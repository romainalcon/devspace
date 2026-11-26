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

    /// The metadata of a directory could not be read.
    #[error("failed to inspect directory `{}`", path.display())]
    InspectDirectory {
        /// Directory that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A directory that must be private is owned by another user or is
    /// accessible to other users.
    #[error(
        "directory `{}` must be owned by the current user (uid {expected_owner}) and not be \
         accessible to other users, but it is owned by uid {owner} with mode {mode:o}; \
         fix its ownership and permissions (e.g. `chmod 700`)",
        path.display()
    )]
    InsecureDirectory {
        /// Offending directory.
        path: PathBuf,
        /// Owner found on disk.
        owner: u32,
        /// Expected owner (current effective user).
        expected_owner: u32,
        /// Permission bits found on disk.
        mode: u32,
    },

    /// The privilege level of the current process could not be determined.
    #[error("failed to determine the privilege level of the current process")]
    PrivilegeQuery {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// The Unix socket path exceeds the OS limit (`sun_path`).
    #[error(
        "socket path `{}` is longer than the {max} bytes allowed by the operating system; \
         use a shorter DEVSPACE_HOME",
        path.display()
    )]
    SocketPathTooLong {
        /// Socket path.
        path: PathBuf,
        /// Maximum length in bytes.
        max: usize,
    },

    /// A non-socket file occupies the IPC endpoint path.
    #[error("`{}` exists and is not a socket; refusing to replace it", path.display())]
    EndpointOccupied {
        /// Endpoint path.
        path: PathBuf,
    },

    /// A stale endpoint left by a previous daemon could not be removed.
    #[error("failed to remove stale IPC endpoint `{}`", path.display())]
    RemoveStaleEndpoint {
        /// Endpoint path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The IPC endpoint could not be created.
    #[error("failed to listen on IPC endpoint `{endpoint}`")]
    Bind {
        /// Endpoint description (socket path or pipe name).
        endpoint: String,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The permissions of the IPC endpoint could not be restricted.
    #[error("failed to restrict permissions of IPC endpoint `{}`", path.display())]
    SecureEndpoint {
        /// Endpoint path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Accepting an incoming IPC connection failed.
    #[error("failed to accept an IPC connection")]
    Accept {
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Connecting to the IPC endpoint failed.
    #[error("failed to connect to IPC endpoint `{endpoint}`")]
    Connect {
        /// Endpoint description (socket path or pipe name).
        endpoint: String,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The identity of the IPC peer could not be determined.
    #[error("failed to determine the identity of the IPC peer")]
    PeerIdentity {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// The IPC peer runs as a different user.
    #[error("IPC peer rejected: {detail}")]
    PeerRejected {
        /// Human-readable reason.
        detail: String,
    },

    /// Signal handlers could not be installed.
    #[error("failed to install shutdown signal handlers")]
    Signals {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// The feature is not implemented on this platform yet.
    #[error("{feature} is not supported on this platform yet")]
    Unsupported {
        /// Description of the missing feature.
        feature: &'static str,
    },

    /// An argument of a system integration is invalid.
    #[error("invalid argument: {detail}")]
    InvalidArgument {
        /// Human-readable explanation.
        detail: String,
    },

    /// A system file could not be read, written or removed.
    #[error("failed to access system file `{}`", path.display())]
    SystemFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A system file exists but was not created by DevSpace; it is never
    /// modified or removed.
    #[error(
        "`{}` exists and is not managed by DevSpace (another tool may use it); \
         refusing to modify it",
        path.display()
    )]
    ForeignSystemFile {
        /// File path.
        path: PathBuf,
    },

    /// A system directory that must only be writable by the administrator
    /// is not.
    #[error(
        "`{}` must be a directory owned by the administrator and writable by it only; \
         refusing to install the helper there",
        path.display()
    )]
    InsecureSystemDirectory {
        /// Directory path.
        path: PathBuf,
    },

    /// A system command could not be run or failed.
    #[error("`{command}` failed: {detail}")]
    SystemCommand {
        /// Command line, for the message.
        command: String,
        /// Exit status and error output, or the reason it could not run.
        detail: String,
    },

    /// Another daemon instance holds the single-instance lock.
    #[error("another DevSpace daemon is already running (lock `{}` is held)", path.display())]
    AlreadyRunning {
        /// Lock file.
        path: PathBuf,
    },

    /// The single-instance lock file could not be opened or locked.
    #[error("failed to lock `{}`", path.display())]
    InstanceLock {
        /// Lock file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
}

impl PlatformError {
    /// Returns `true` for `Connect` errors meaning that nothing listens on
    /// the endpoint (missing socket or pipe, or stale socket).
    #[must_use]
    pub fn is_endpoint_absent(&self) -> bool {
        matches!(
            self,
            Self::Connect { source, .. }
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                )
        )
    }
}
