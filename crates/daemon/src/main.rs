//! `devspaced`, the DevSpace background daemon.
//!
//! **Status (M0):** only the startup sequence is implemented: privilege check,
//! directory resolution and creation, configuration loading and logging. The
//! IPC server and the supervision loop arrive in milestone M1, so the binary
//! currently exits right after a successful startup check instead of
//! pretending to serve requests.

use std::process::ExitCode;

use devspace_core::config::GlobalConfig;
use devspace_core::{AppPaths, Result, error, logging};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("devspaced: {}", error::report(&err));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // Refuse to run elevated before touching the filesystem.
    devspace_core::ensure_unprivileged()?;
    let paths = AppPaths::resolve()?;
    paths.ensure_exists()?;
    let config = GlobalConfig::load_or_default(&paths.config_file())?;
    logging::init(&config.logging)?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config_dir = %paths.config_dir.display(),
        data_dir = %paths.data_dir.display(),
        log_dir = %paths.log_dir.display(),
        "startup checks passed"
    );
    tracing::warn!("IPC server not implemented yet (milestone M1); exiting");
    Ok(())
}
