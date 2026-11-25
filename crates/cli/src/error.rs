//! Error type of the CLI.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use devspace_ipc::IpcError;
use devspace_platform::PlatformError;

/// Error reported by a CLI command.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Core engine error.
    #[error(transparent)]
    Core(#[from] devspace_core::Error),

    /// Platform error.
    #[error(transparent)]
    Platform(#[from] PlatformError),

    /// IPC error while talking to the daemon.
    #[error(transparent)]
    Ipc(#[from] IpcError),

    /// The Tokio runtime could not be created.
    #[error("failed to start the async runtime")]
    Runtime(#[source] io::Error),

    /// The path of the running executable is unknown.
    #[error("failed to locate the devspace executable")]
    CurrentExe(#[source] io::Error),

    /// `devspaced` is not installed next to `devspace`.
    #[error("daemon executable `{}` not found next to the devspace executable", path.display())]
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
