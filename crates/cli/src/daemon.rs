//! `devspace daemon start|stop|status`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use devspace_core::AppPaths;
use devspace_core::instance::InstanceLock;
use devspace_ipc::{Client, Endpoint, IpcError, PeerInfo, Request, Response};

use crate::error::CliError;

/// Exit code of `devspace daemon status` when the daemon is not running
/// (same convention as LSB init scripts).
pub const NOT_RUNNING_EXIT_CODE: u8 = 3;

/// File of the log directory receiving the daemon's stderr (fatal errors
/// only in background mode). Truncated at every start.
pub const DAEMON_STDERR_FILE_NAME: &str = "daemon-stderr.log";

const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Daemon subcommands.
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon in the background.
    Start,
    /// Stop the running daemon gracefully.
    Stop,
    /// Show whether the daemon is running (exit code 3 when it is not).
    Status,
}

/// Runs a daemon subcommand.
pub fn execute(command: DaemonCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(CliError::Runtime)?;
    runtime.block_on(async {
        match command {
            DaemonCommand::Start => start(paths, &endpoint).await,
            DaemonCommand::Stop => stop(paths, &endpoint).await,
            DaemonCommand::Status => status(paths, &endpoint).await,
        }
    })
}

fn peer_info() -> PeerInfo {
    PeerInfo {
        name: "devspace-cli".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Connects to the daemon; `Ok(None)` means that it is not running.
async fn try_connect(endpoint: &Endpoint) -> Result<Option<Client>, CliError> {
    match Client::connect(endpoint, peer_info()).await {
        Ok(client) => Ok(Some(client)),
        Err(IpcError::NotRunning { .. }) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn status(paths: &AppPaths, endpoint: &Endpoint) -> Result<ExitCode, CliError> {
    let Some(mut client) = try_connect(endpoint).await? else {
        let lock = paths.daemon_lock_file();
        if InstanceLock::is_held(&lock)? {
            return Err(CliError::Unresponsive { lock });
        }
        println!("status:   not running");
        return Ok(ExitCode::from(NOT_RUNNING_EXIT_CODE));
    };
    let Response::Status(status) = client.request(Request::Status).await? else {
        return Err(IpcError::UnexpectedMessage {
            expected: "status response",
        }
        .into());
    };
    println!("status:   running");
    println!("pid:      {}", status.pid);
    println!("version:  {}", status.version);
    println!("uptime:   {}", format_uptime(status.uptime_secs));
    println!("endpoint: {endpoint}");
    println!("logs:     {}", paths.log_dir.display());
    Ok(ExitCode::SUCCESS)
}

async fn start(paths: &AppPaths, endpoint: &Endpoint) -> Result<ExitCode, CliError> {
    if let Some(client) = try_connect(endpoint).await? {
        println!("daemon already running (pid {})", client.daemon().pid);
        return Ok(ExitCode::SUCCESS);
    }
    paths.ensure_exists()?;
    let lock = paths.daemon_lock_file();
    if InstanceLock::is_held(&lock)? {
        return Err(CliError::Unresponsive { lock });
    }

    let executable = daemon_executable()?;
    let stderr_path = paths.log_dir.join(DAEMON_STDERR_FILE_NAME);
    let stderr = File::create(&stderr_path).map_err(|source| CliError::DaemonStderrFile {
        path: stderr_path.clone(),
        source,
    })?;
    let mut command = Command::new(&executable);
    command
        .arg("--background")
        // Do not keep the caller's working directory busy (e.g. a volume
        // that the user wants to unmount).
        .current_dir(&paths.data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    devspace_platform::process::detach(&mut command);
    let mut child = command.spawn().map_err(|source| CliError::SpawnDaemon {
        path: executable,
        source,
    })?;
    tracing::debug!(pid = child.id(), "daemon process spawned");

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(client) = try_connect(endpoint).await? {
            println!("daemon started (pid {})", client.daemon().pid);
            return Ok(ExitCode::SUCCESS);
        }
        if let Some(status) = child.try_wait().map_err(CliError::DaemonWait)? {
            return Err(CliError::DaemonExited {
                status,
                output: read_stderr_file(&stderr_path),
                log_dir: paths.log_dir.clone(),
            });
        }
        if Instant::now() >= deadline {
            return Err(CliError::StartTimeout {
                pid: child.id(),
                secs: START_TIMEOUT.as_secs(),
                log_dir: paths.log_dir.clone(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn stop(paths: &AppPaths, endpoint: &Endpoint) -> Result<ExitCode, CliError> {
    let Some(mut client) = try_connect(endpoint).await? else {
        println!("daemon is not running");
        return Ok(ExitCode::SUCCESS);
    };
    let pid = client.daemon().pid;
    match client.request(Request::Shutdown).await? {
        Response::ShuttingDown => {}
        _ => {
            return Err(IpcError::UnexpectedMessage {
                expected: "shutdown acknowledgement",
            }
            .into());
        }
    }
    drop(client);

    // The daemon releases the lock as the very last step of its shutdown.
    let lock = paths.daemon_lock_file();
    let deadline = Instant::now() + STOP_TIMEOUT;
    while InstanceLock::is_held(&lock)? {
        if Instant::now() >= deadline {
            return Err(CliError::StopTimeout {
                pid,
                secs: STOP_TIMEOUT.as_secs(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    println!("daemon stopped (pid {pid})");
    Ok(ExitCode::SUCCESS)
}

/// `devspaced` is installed next to `devspace`.
fn daemon_executable() -> Result<PathBuf, CliError> {
    let cli = std::env::current_exe().map_err(CliError::CurrentExe)?;
    let path = cli.with_file_name(format!("devspaced{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(CliError::DaemonExecutableNotFound { path })
    }
}

fn read_stderr_file(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => text.trim().to_owned(),
        Err(err) => format!("(could not read `{}`: {err})", path.display()),
    }
}

fn format_uptime(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (hours, rest) = (rest / 3_600, rest % 3_600);
    let (minutes, seconds) = (rest / 60, rest % 60);
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m {seconds:02}s")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_formatting() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(59), "59s");
        assert_eq!(format_uptime(61), "1m 01s");
        assert_eq!(format_uptime(3_600 + 120 + 3), "1h 02m 03s");
        assert_eq!(format_uptime(2 * 86_400 + 5), "2d 00h 00m 05s");
    }
}
