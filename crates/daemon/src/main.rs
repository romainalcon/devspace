//! `devspaced`, the DevSpace background daemon.
//!
//! Startup sequence: refuse elevated execution, resolve and create the
//! application directories, load the configuration, initialise logging
//! (rolling files in the log directory, plus stderr in the foreground), take
//! the single-instance lock, open and migrate the database, stop the web
//! server, PHP-FPM masters and service servers left by a crashed daemon,
//! load the package index, then serve IPC requests (and run the DNS and
//! web servers and the services) until a shutdown request or signal.
//!
//! The daemon normally runs in the background, started by
//! `devspace daemon start`; running `devspaced` directly keeps it in the
//! foreground (stop it with Ctrl+C).

mod dns;
mod error;
mod handlers;
mod php;
mod process;
mod runtimes;
mod server;
mod services;
mod web;

use std::process::ExitCode;

use clap::Parser;
use devspace_core::config::GlobalConfig;
use devspace_core::db::Database;
use devspace_core::logging::{self, LogFile, LogTargets};
use devspace_core::runtime::remote::IndexStore;
use devspace_core::runtime::{self as runtime_packages, RuntimeLayout};
use devspace_core::service::ServiceLayout;
use devspace_core::web::WebLayout;
use devspace_core::{AppPaths, error::report};
use devspace_platform::instance::InstanceLock;

use crate::error::DaemonError;

/// Prefix of the daemon log files in the log directory.
const LOG_FILE_PREFIX: &str = "devspaced";

#[derive(Debug, Parser)]
#[command(
    name = "devspaced",
    version,
    about = "DevSpace daemon (normally started with `devspace daemon start`)"
)]
struct Args {
    /// Background mode, used by `devspace daemon start`: logs go to the log
    /// directory only; stderr only receives fatal errors.
    #[arg(long)]
    background: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let report = report(&err);
            // No-op if logging was not initialised yet; stderr covers that case.
            tracing::error!(error = %report, "daemon stopped with an error");
            eprintln!("devspaced: {report}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), DaemonError> {
    // Refuse to run elevated before touching the filesystem.
    devspace_core::ensure_unprivileged()?;
    let paths = AppPaths::resolve()?;
    paths.ensure_exists()?;
    let config = GlobalConfig::load_or_default(&paths.config_file())?;
    logging::init(
        &config.logging,
        &LogTargets {
            stderr: !args.background,
            file: Some(LogFile {
                dir: paths.log_dir.clone(),
                prefix: LOG_FILE_PREFIX.to_owned(),
            }),
        },
    )?;

    // Held until the end of `run`, after the runtime and the endpoint are gone.
    let lock = InstanceLock::acquire(&paths.daemon_lock_file())?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        background = args.background,
        config_dir = %paths.config_dir.display(),
        data_dir = %paths.data_dir.display(),
        log_dir = %paths.log_dir.display(),
        lock = %lock.path().display(),
        "daemon starting"
    );
    // Opened after taking the lock: only one daemon ever writes to it.
    let db = Database::open(&paths.database_file())?;
    let layout = WebLayout::new(&paths);
    web::stop_orphan(&layout);
    let sites_address = config.web.address.ip();
    let web = web::WebServer::new(layout, config.web);
    let runtime_layout = RuntimeLayout::new(&paths);
    let installed = runtime_packages::list_installed(&runtime_layout);
    let services = services::Services::new(
        ServiceLayout::new(&paths),
        config.services,
        sites_address,
        &installed,
    );
    let runtimes = runtimes::Runtimes::new(runtime_layout, IndexStore::new(&paths));
    let remote_index = config.packages.remote_index;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("devspaced-worker")
        .build()
        .map_err(DaemonError::Runtime)?;
    let result = runtime.block_on(server::run(
        &paths,
        db,
        web,
        runtimes,
        services,
        remote_index,
    ));
    // Cancel any remaining task, and close the database, before releasing
    // the lock.
    drop(runtime);
    drop(lock);
    result?;
    tracing::info!("daemon stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Args::command().debug_assert();
    }
}
