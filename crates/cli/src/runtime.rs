//! `devspace runtime list|install|remove` and `devspace exec`.
//!
//! Runtimes are installed and selected by the daemon; these commands
//! require it to run.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use devspace_core::AppPaths;
use devspace_ipc::{Endpoint, Request, Response, RuntimeEnv, RuntimeList};

use crate::client::{self, unexpected};
use crate::error::CliError;

/// Time limit of `runtime install` (downloads of up to a few hundred MB).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Time limit of the other runtime requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Runtime subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum RuntimeCommand {
    /// List the versions published for this platform and the installed
    /// ones.
    List,
    /// Download, verify and install a version.
    Install {
        /// Runtime: `php` or `node`.
        name: String,
        /// `MAJOR`, `MAJOR.MINOR` or `MAJOR.MINOR.PATCH`; the newest
        /// published match is installed (the newest version if absent).
        version: Option<String>,
    },
    /// Remove an installed version.
    Remove {
        /// Runtime: `php` or `node`.
        name: String,
        /// Exact installed version, e.g. `8.4.23`.
        version: String,
    },
}

/// Runs a runtime subcommand.
pub fn execute(command: RuntimeCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let (request, timeout) = match command {
        RuntimeCommand::List => (Request::RuntimeList, REQUEST_TIMEOUT),
        RuntimeCommand::Install { name, version } => {
            println!(
                "installing {name} {} (download and verification may take a few minutes)…",
                version.as_deref().unwrap_or("(newest)")
            );
            (Request::RuntimeInstall { name, version }, INSTALL_TIMEOUT)
        }
        RuntimeCommand::Remove { name, version } => {
            (Request::RuntimeRemove { name, version }, REQUEST_TIMEOUT)
        }
    };
    let response = client::block_on(send(&endpoint, request, timeout))??;
    match response {
        Response::Runtimes(list) => print!("{}", format_list(&list)),
        Response::RuntimeInstalled(runtime) if runtime.changed => println!(
            "installed {} {} in {}",
            runtime.name,
            runtime.version,
            runtime.path.display()
        ),
        Response::RuntimeInstalled(runtime) => println!(
            "{} {} is already installed ({})",
            runtime.name,
            runtime.version,
            runtime.path.display()
        ),
        Response::RuntimeRemoved(runtime) => {
            println!("removed {} {}", runtime.name, runtime.version);
        }
        _ => return Err(unexpected("runtime response")),
    }
    Ok(ExitCode::SUCCESS)
}

async fn send(
    endpoint: &Endpoint,
    request: Request,
    timeout: Duration,
) -> Result<Response, CliError> {
    let mut client = client::connect(endpoint).await?;
    client::request_with_timeout(&mut client, request, timeout).await
}

fn format_list(list: &RuntimeList) -> String {
    let mut out = String::new();
    // Writing to a String cannot fail.
    let _ = writeln!(
        out,
        "package index #{} for {}",
        list.index_serial, list.platform
    );
    let width = list
        .versions
        .iter()
        .map(|v| v.name.len() + v.version.len() + 1)
        .max()
        .unwrap_or_default();
    for version in &list.versions {
        let label = format!("{} {}", version.name, version.version);
        let state = match (&version.path, version.available) {
            (Some(path), true) => format!("installed  {}", path.display()),
            (Some(path), false) => {
                format!(
                    "installed  {} (no longer in the package index)",
                    path.display()
                )
            }
            (None, _) => "available".to_owned(),
        };
        let _ = writeln!(out, "  {label:<width$}  {state}");
    }
    out
}

/// `devspace exec`: runs `command` with the runtime versions of the
/// project containing the current directory first in `PATH`, and returns
/// its exit code.
pub fn exec(command: &[OsString], paths: &AppPaths) -> Result<ExitCode, CliError> {
    let Some((program, args)) = command.split_first() else {
        return Err(CliError::Refused {
            message: "no command given; usage: devspace exec -- <command> [args…]".to_owned(),
        });
    };
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let directory = std::env::current_dir().map_err(CliError::CurrentDir)?;
    let env = match client::block_on(send(
        &endpoint,
        Request::RuntimeEnv { directory },
        REQUEST_TIMEOUT,
    ))?? {
        Response::RuntimeEnv(env) => env,
        _ => return Err(unexpected("runtime environment")),
    };
    for message in &env.missing {
        eprintln!("warning: {message}");
    }
    let path = search_path(&env, std::env::var_os("PATH"))?;
    let mut child = std::process::Command::new(program);
    child.args(args).env("PATH", path);
    for entry in &env.runtimes {
        for (name, value) in &entry.variables {
            child.env(name, value);
        }
        tracing::debug!(runtime = %entry.name, version = %entry.version, "selected");
    }
    let status = child.status().map_err(|source| CliError::Exec {
        program: Path::new(program).to_path_buf(),
        source,
    })?;
    // A child killed by a signal has no code; report a failure.
    let code = status.code().unwrap_or(1);
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

/// `PATH` with the directories of the selected runtimes first.
fn search_path(env: &RuntimeEnv, current: Option<OsString>) -> Result<OsString, CliError> {
    let mut dirs: Vec<std::path::PathBuf> = env
        .runtimes
        .iter()
        .flat_map(|entry| entry.bin_dirs.iter().cloned())
        .collect();
    if let Some(current) = current {
        dirs.extend(std::env::split_paths(&current));
    }
    std::env::join_paths(dirs).map_err(|err| CliError::Refused {
        message: format!("cannot build PATH: {err}"),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devspace_ipc::{RuntimeEnvEntry, RuntimeVersionInfo};

    use super::*;

    #[test]
    fn list_shows_installed_and_available_versions() {
        let text = format_list(&RuntimeList {
            index_serial: 3,
            platform: "macos-aarch64".into(),
            versions: vec![
                RuntimeVersionInfo {
                    name: "node".into(),
                    version: "24.22.3".into(),
                    available: true,
                    path: None,
                },
                RuntimeVersionInfo {
                    name: "php".into(),
                    version: "8.4.23".into(),
                    available: true,
                    path: Some(PathBuf::from("/data/runtimes/php/8.4.23")),
                },
                RuntimeVersionInfo {
                    name: "php".into(),
                    version: "8.3.1".into(),
                    available: false,
                    path: Some(PathBuf::from("/data/runtimes/php/8.3.1")),
                },
            ],
        });
        assert!(
            text.starts_with("package index #3 for macos-aarch64\n"),
            "{text}"
        );
        assert!(text.contains("  node 24.22.3  available\n"), "{text}");
        assert!(
            text.contains("  php 8.4.23    installed  /data/runtimes/php/8.4.23\n"),
            "{text}"
        );
        assert!(text.contains("(no longer in the package index)"), "{text}");
    }

    #[test]
    fn runtime_directories_come_first_in_path() {
        let env = RuntimeEnv {
            project: None,
            runtimes: vec![RuntimeEnvEntry {
                name: "php".into(),
                version: "8.4.23".into(),
                bin_dirs: vec![PathBuf::from("/data/php/bin")],
                variables: Vec::new(),
            }],
            missing: Vec::new(),
        };
        let current = std::env::join_paths([PathBuf::from("/usr/bin")]).unwrap();
        let path = search_path(&env, Some(current)).unwrap();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs,
            [PathBuf::from("/data/php/bin"), PathBuf::from("/usr/bin")]
        );
    }
}
