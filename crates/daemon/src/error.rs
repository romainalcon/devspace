//! Error type of the daemon binary.

use std::io;

use devspace_platform::PlatformError;

/// Fatal error stopping the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Core engine error (configuration, logging, instance lock).
    #[error(transparent)]
    Core(#[from] devspace_core::Error),

    /// Platform error (directories, IPC endpoint).
    #[error(transparent)]
    Platform(#[from] PlatformError),

    /// The Tokio runtime could not be created.
    #[error("failed to start the async runtime")]
    Runtime(#[source] io::Error),
}
