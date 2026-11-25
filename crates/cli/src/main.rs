//! `devspace` command-line interface.
//!
//! In milestone M0 the CLI only exposes local, read-mostly commands working on
//! the global configuration and the application directories. Commands driving
//! the daemon arrive with the IPC protocol (M1).

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use devspace_core::config::GlobalConfig;
use devspace_core::{AppPaths, Result, error, logging};

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
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}", error::report(&err));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    devspace_core::ensure_unprivileged()?;
    let paths = AppPaths::resolve()?;
    // `paths` and `config path` must keep working with a broken config file,
    // so the load error is only raised by commands that need the config.
    let config = GlobalConfig::load_or_default(&paths.config_file());
    let logging_config = config
        .as_ref()
        .map(|c| c.logging.clone())
        .unwrap_or_default();
    logging::init(&logging_config)?;

    match cli.command {
        Command::Paths => {
            println!("config: {}", paths.config_dir.display());
            println!("data:   {}", paths.data_dir.display());
            println!("cache:  {}", paths.cache_dir.display());
            println!("logs:   {}", paths.log_dir.display());
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
    }
    Ok(())
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
