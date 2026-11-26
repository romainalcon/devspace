//! Starting, stopping and probing the daemon from a client.
//!
//! Shared by the CLI (`devspace daemon start|stop|status`) and the desktop
//! application, so that both start the daemon the same way: `devspaced
//! --background`, detached, its standard error kept in
//! [`DAEMON_STDERR_FILE_NAME`] to explain an early exit.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use devspace_platform::instance::InstanceLock;
use devspace_platform::{AppPaths, PlatformError};

use crate::protocol::{DaemonStatus, PeerInfo, Request, Response};
use crate::{Client, Endpoint, IpcError};

/// File of the log directory receiving the daemon's stderr (fatal errors
/// only in background mode). Truncated at every start.
pub const DAEMON_STDERR_FILE_NAME: &str = "daemon-stderr.log";

/// Maximum time between spawning the daemon and its first accepted
/// connection.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time between the shutdown acknowledgement and the exit of the
/// daemon.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Error while starting, stopping or probing the daemon.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// IPC error while talking to the daemon.
    #[error(transparent)]
    Ipc(#[from] IpcError),

    /// Platform error (directories, instance lock).
    #[error(transparent)]
    Platform(#[from] PlatformError),

    /// The path of the running executable is unknown.
    #[error("failed to locate the running executable")]
    CurrentExe(#[source] io::Error),

    /// `devspaced` is not installed next to the client.
    #[error("daemon executable `{}` not found", path.display())]
    DaemonExecutableNotFound {
        /// Expected location.
        path: PathBuf,
    },

    /// The file receiving the daemon's stderr could not be created.
    #[error("failed to create `{}`", path.display())]
    DaemonStderrFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The daemon process could not be spawned.
    #[error("failed to start `{}`", path.display())]
    SpawnDaemon {
        /// Daemon executable.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The state of the spawned daemon could not be queried.
    #[error("failed to check the daemon process")]
    DaemonWait(#[source] io::Error),

    /// The daemon exited before accepting connections.
    #[error(
        "the daemon exited during startup ({status}){}\nlogs: {}",
        if output.is_empty() { String::new() } else { format!(":\n{output}") },
        log_dir.display()
    )]
    DaemonExited {
        /// Exit status of the daemon.
        status: ExitStatus,
        /// What the daemon wrote on stderr.
        output: String,
        /// Log directory.
        log_dir: PathBuf,
    },

    /// The daemon did not accept connections in time.
    #[error(
        "the daemon (pid {pid}) did not become ready within {secs}s; see the logs in `{}`",
        log_dir.display()
    )]
    StartTimeout {
        /// Process id of the spawned daemon.
        pid: u32,
        /// Elapsed limit in seconds.
        secs: u64,
        /// Log directory.
        log_dir: PathBuf,
    },

    /// The daemon accepted the shutdown request but did not exit in time.
    #[error("the daemon (pid {pid}) did not stop within {secs}s")]
    StopTimeout {
        /// Process id of the daemon.
        pid: u32,
        /// Elapsed limit in seconds.
        secs: u64,
    },

    /// A process holds the instance lock but does not accept connections.
    #[error(
        "a daemon process holds `{}` but does not accept connections; it may still be \
         starting or be unresponsive",
        lock.display()
    )]
    Unresponsive {
        /// Lock file.
        lock: PathBuf,
    },
}

/// Outcome of [`start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started {
    /// Process id of the daemon.
    pub pid: u32,
    /// `true` when the daemon was already running.
    pub already_running: bool,
}

/// Connects to the daemon; `Ok(None)` means that it is not running.
///
/// # Errors
///
/// Returns the connection and handshake errors of [`Client::connect`]
/// other than [`IpcError::NotRunning`].
pub async fn try_connect(endpoint: &Endpoint, peer: &PeerInfo) -> Result<Option<Client>, IpcError> {
    match Client::connect(endpoint, peer.clone()).await {
        Ok(client) => Ok(Some(client)),
        Err(IpcError::NotRunning { .. }) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Status of the daemon; `Ok(None)` means that it is not running.
///
/// # Errors
///
/// Returns [`LaunchError::Unresponsive`] when a process holds the instance
/// lock without accepting connections, and IPC or lock errors.
pub async fn status(
    paths: &AppPaths,
    endpoint: &Endpoint,
    peer: &PeerInfo,
) -> Result<Option<DaemonStatus>, LaunchError> {
    let Some(mut client) = try_connect(endpoint, peer).await? else {
        let lock = paths.daemon_lock_file();
        if InstanceLock::is_held(&lock)? {
            return Err(LaunchError::Unresponsive { lock });
        }
        return Ok(None);
    };
    match client.request(Request::Status).await? {
        Response::Status(status) => Ok(Some(status)),
        _ => Err(IpcError::UnexpectedMessage {
            expected: "status response",
        }
        .into()),
    }
}

/// Starts `executable` (`devspaced`) in the background unless the daemon
/// already runs, and returns once it accepts connections
/// ([`START_TIMEOUT`] at most).
///
/// # Errors
///
/// Returns [`LaunchError::DaemonExited`] with its error output when the
/// daemon exits during startup, [`LaunchError::StartTimeout`],
/// [`LaunchError::Unresponsive`], and spawn, IPC or lock errors.
pub async fn start(
    paths: &AppPaths,
    endpoint: &Endpoint,
    peer: &PeerInfo,
    executable: &Path,
) -> Result<Started, LaunchError> {
    if let Some(client) = try_connect(endpoint, peer).await? {
        return Ok(Started {
            pid: client.daemon().pid,
            already_running: true,
        });
    }
    paths.ensure_exists()?;
    let lock = paths.daemon_lock_file();
    if InstanceLock::is_held(&lock)? {
        return Err(LaunchError::Unresponsive { lock });
    }

    let stderr_path = paths.log_dir.join(DAEMON_STDERR_FILE_NAME);
    let stderr = File::create(&stderr_path).map_err(|source| LaunchError::DaemonStderrFile {
        path: stderr_path.clone(),
        source,
    })?;
    let mut command = Command::new(executable);
    command
        .arg("--background")
        // Do not keep the caller's working directory busy (e.g. a volume
        // that the user wants to unmount).
        .current_dir(&paths.data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    devspace_platform::process::detach(&mut command);
    let mut child = command.spawn().map_err(|source| LaunchError::SpawnDaemon {
        path: executable.to_path_buf(),
        source,
    })?;
    tracing::debug!(pid = child.id(), "daemon process spawned");

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(client) = try_connect(endpoint, peer).await? {
            return Ok(Started {
                pid: client.daemon().pid,
                already_running: false,
            });
        }
        if let Some(status) = child.try_wait().map_err(LaunchError::DaemonWait)? {
            return Err(LaunchError::DaemonExited {
                status,
                output: read_stderr_file(&stderr_path),
                log_dir: paths.log_dir.clone(),
            });
        }
        if Instant::now() >= deadline {
            return Err(LaunchError::StartTimeout {
                pid: child.id(),
                secs: START_TIMEOUT.as_secs(),
                log_dir: paths.log_dir.clone(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Asks the daemon to shut down and returns its process id once it has
/// exited ([`STOP_TIMEOUT`] at most); `Ok(None)` when it was not running.
///
/// # Errors
///
/// Returns [`LaunchError::StopTimeout`], and IPC or lock errors.
pub async fn stop(
    paths: &AppPaths,
    endpoint: &Endpoint,
    peer: &PeerInfo,
) -> Result<Option<u32>, LaunchError> {
    let Some(mut client) = try_connect(endpoint, peer).await? else {
        return Ok(None);
    };
    let pid = client.daemon().pid;
    if client.request(Request::Shutdown).await? != Response::ShuttingDown {
        return Err(IpcError::UnexpectedMessage {
            expected: "shutdown acknowledgement",
        }
        .into());
    }
    drop(client);

    // The daemon releases the lock as the very last step of its shutdown.
    let lock = paths.daemon_lock_file();
    let deadline = Instant::now() + STOP_TIMEOUT;
    while InstanceLock::is_held(&lock)? {
        if Instant::now() >= deadline {
            return Err(LaunchError::StopTimeout {
                pid,
                secs: STOP_TIMEOUT.as_secs(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(Some(pid))
}

/// `devspaced` next to the running executable: DevSpace installs the
/// daemon, the CLI and the desktop application in the same directory (and
/// Cargo builds them in the same target directory).
///
/// # Errors
///
/// Returns [`LaunchError::CurrentExe`] or
/// [`LaunchError::DaemonExecutableNotFound`].
pub fn daemon_executable() -> Result<PathBuf, LaunchError> {
    let current = std::env::current_exe().map_err(LaunchError::CurrentExe)?;
    let path = current.with_file_name(format!("devspaced{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(LaunchError::DaemonExecutableNotFound { path })
    }
}

fn read_stderr_file(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => text.trim().to_owned(),
        Err(err) => format!("(could not read `{}`: {err})", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use devspace_platform::instance::InstanceLock;

    use super::*;

    fn peer() -> PeerInfo {
        PeerInfo {
            name: "launcher-test".into(),
            version: "0".into(),
        }
    }

    fn setup() -> (tempfile::TempDir, AppPaths, Endpoint) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_root(tmp.path());
        paths.ensure_exists().unwrap();
        let endpoint = Endpoint::new(&paths.runtime_dir).unwrap();
        (tmp, paths, endpoint)
    }

    #[tokio::test]
    async fn absent_daemon_is_reported_as_not_running() {
        let (_tmp, paths, endpoint) = setup();
        assert!(try_connect(&endpoint, &peer()).await.unwrap().is_none());
        assert!(status(&paths, &endpoint, &peer()).await.unwrap().is_none());
        assert!(stop(&paths, &endpoint, &peer()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn held_lock_without_endpoint_is_unresponsive() {
        let (_tmp, paths, endpoint) = setup();
        let _lock = InstanceLock::acquire(&paths.daemon_lock_file()).unwrap();
        assert!(matches!(
            status(&paths, &endpoint, &peer()).await,
            Err(LaunchError::Unresponsive { .. })
        ));
        let missing = paths.data_dir.join("devspaced");
        assert!(matches!(
            start(&paths, &endpoint, &peer(), &missing).await,
            Err(LaunchError::Unresponsive { .. })
        ));
    }

    #[tokio::test]
    async fn missing_executable_cannot_be_spawned() {
        let (_tmp, paths, endpoint) = setup();
        let missing = paths.data_dir.join("devspaced");
        assert!(matches!(
            start(&paths, &endpoint, &peer(), &missing).await,
            Err(LaunchError::SpawnDaemon { .. })
        ));
    }

    // A shell script stands for a daemon failing at startup.
    #[cfg(unix)]
    #[tokio::test]
    async fn early_exit_reports_the_error_output() {
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, paths, endpoint) = setup();
        let script = paths.data_dir.join("fake-devspaced");
        std::fs::write(&script, "#!/bin/sh\necho \"boom: $1\" >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let err = start(&paths, &endpoint, &peer(), &script)
            .await
            .unwrap_err();
        let LaunchError::DaemonExited { status, output, .. } = &err else {
            panic!("unexpected error: {err}");
        };
        assert_eq!(status.code(), Some(3));
        assert_eq!(output, "boom: --background");
        assert!(err.to_string().contains("boom: --background"));
    }
}
