//! `devspace` command-line interface.
//!
//! Local commands (`paths`, `config`) work without the daemon; `daemon`
//! commands manage the daemon lifecycle, `project`, `web`, `runtime` and
//! `exec` commands use the registry, web server and runtimes owned by the
//! daemon, through the IPC protocol. `system` commands install the machine-wide integrations
//! through the privileged helper, after approval.

mod client;
mod daemon;
mod error;
mod project;
mod runtime;
mod system;
mod web;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use devspace_core::config::GlobalConfig;
use devspace_core::logging::{self, LogTargets};
use devspace_core::{AppPaths, error::report};

use crate::daemon::DaemonCommand;
use crate::error::CliError;
use crate::project::ProjectCommand;
use crate::runtime::RuntimeCommand;
use crate::system::SystemCommand;
use crate::web::WebCommand;

#[derive(Debug, Parser)]
#[command(
    name = "devspace",
    version,
    about = "DevSpace local development environment manager"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the directories used by DevSpace.
    Paths,
    /// Inspect or initialise the global configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Manage the DevSpace daemon.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Manage registered projects (requires the daemon).
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Manage the web server serving https://<project>.internal (requires the
    /// daemon).
    #[command(subcommand)]
    Web(WebCommand),
    /// Install, list or remove runtime versions (PHP, Node.js; requires the
    /// daemon).
    #[command(subcommand)]
    Runtime(RuntimeCommand),
    /// Run a command with the runtime versions of the current project
    /// first in PATH, e.g. `devspace exec -- php artisan migrate`.
    Exec {
        /// Command and its arguments.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<std::ffi::OsString>,
    },
    /// Install or remove the machine-wide integrations (DNS resolver, port
    /// forwarding, certificate trust) with administrator rights.
    #[command(subcommand)]
    System(SystemCommand),
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the path of the global configuration file.
    Path,
    /// Print the effective configuration (file merged with defaults).
    Show,
    /// Create the configuration file with default values. Never overwrites.
    Init,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {}", report(&err));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, CliError> {
    devspace_core::ensure_unprivileged()?;
    let paths = AppPaths::resolve()?;
    // `paths` and `config path` must keep working with a broken config file,
    // so the load error is only raised by commands that need the config.
    let config = GlobalConfig::load_or_default(&paths.config_file());
    let logging_config = config
        .as_ref()
        .map(|c| c.logging.clone())
        .unwrap_or_default();
    logging::init(&logging_config, &LogTargets::stderr_only())?;

    match cli.command {
        Command::Paths => {
            println!("config: {}", paths.config_dir.display());
            println!("data:   {}", paths.data_dir.display());
            println!("cache:  {}", paths.cache_dir.display());
            println!("logs:   {}", paths.log_dir.display());
            println!("run:    {}", paths.runtime_dir.display());
        }
        Command::Config(ConfigCommand::Path) => println!("{}", paths.config_file().display()),
        Command::Config(ConfigCommand::Show) => print!("{}", config?.to_toml()?),
        Command::Config(ConfigCommand::Init) => {
            paths.ensure_exists()?;
            let file = paths.config_file();
            GlobalConfig::default().write_new(&file)?;
            tracing::info!(path = %file.display(), "configuration file created");
            println!("created {}", file.display());
        }
        Command::Daemon(command) => return daemon::execute(command, &paths),
        Command::Project(command) => return project::execute(command, &paths),
        Command::Web(command) => return web::execute(command, &paths),
        Command::System(command) => return system::execute(command, &paths),
        Command::Runtime(command) => return runtime::execute(command, &paths),
        Command::Exec { command } => return runtime::exec(&command, &paths),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
