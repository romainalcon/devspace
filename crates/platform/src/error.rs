//! Errors raised by platform primitives.

use std::io;
use std::path::PathBuf;

/// Error returned by the platform layer.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The user home directory could not be determined.
    #[error("unable to determine the home directory of the current user")]
    HomeDirectoryNotFound,

    /// The `DEVSPACE_HOME` override is not an absolute path.
    #[error("{variable} must be an absolute path, got `{}`", value.display())]
    RelativeHomeOverride {
        /// Name of the environment variable.
        variable: &'static str,
        /// Rejected value.
        value: PathBuf,
    },

    /// A directory could not be created or secured.
    #[error("failed to create directory `{}`", path.display())]
    CreateDirectory {
        /// Directory that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The privilege level of the current process could not be determined.
    #[error("failed to determine the privilege level of the current process")]
    PrivilegeQuery {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },
}
