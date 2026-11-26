//! Error type of the CLI.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use devspace_ipc::IpcError;
use devspace_ipc::launcher::LaunchError;
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

    /// The daemon could not be started, stopped or probed.
    #[error(transparent)]
    Launch(#[from] LaunchError),

    /// The Tokio runtime could not be created.
    #[error("failed to start the async runtime")]
    Runtime(#[source] io::Error),

    /// The path of the running executable is unknown.
    #[error("failed to locate the devspace executable")]
    CurrentExe(#[source] io::Error),

    /// A command needs the daemon, which is not running.
    #[error("the DevSpace daemon is not running; start it with `devspace daemon start`")]
    DaemonNotRunning,

    /// The daemon refused or failed to process a request.
    #[error("{message}")]
    Refused {
        /// Explanation sent by the daemon.
        message: String,
    },

    /// The current directory is unknown, so a relative path cannot be
    /// resolved.
    #[error("failed to determine the current directory")]
    CurrentDir(#[source] io::Error),

    /// The privileged helper is not installed next to `devspace`.
    #[error("privileged helper `{}` not found next to the devspace executable", path.display())]
    HelperNotFound {
        /// Expected location.
        path: PathBuf,
    },

    /// The privileged helper failed (or the user did not authenticate).
    #[error(
        "{} failed ({status}); the changes listed before it were applied",
        operation.as_deref().unwrap_or("a privileged change")
    )]
    HelperFailed {
        /// Helper command that failed, when known.
        operation: Option<String>,
        /// Exit status of sudo / the helper.
        status: ExitStatus,
    },

    /// The system setup cannot proceed; nothing was changed.
    #[error("{reason}; nothing was changed")]
    SetupBlocked {
        /// Explanation.
        reason: String,
    },

    /// The confirmation prompt failed.
    #[error("failed to read the confirmation")]
    Prompt(#[source] io::Error),

    /// The command given to `devspace exec` could not be started.
    #[error("failed to run `{}`", program.display())]
    Exec {
        /// Program.
        program: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
}
