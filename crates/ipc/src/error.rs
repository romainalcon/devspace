//! Errors of the IPC layer.

use std::io;
use std::time::Duration;

use devspace_platform::PlatformError;

/// Convenience alias for results of the IPC layer.
pub type Result<T, E = IpcError> = std::result::Result<T, E>;

/// Error returned by the IPC layer.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Nothing listens on the daemon endpoint.
    #[error("the DevSpace daemon is not running (nothing listens on `{endpoint}`)")]
    NotRunning {
        /// Endpoint description.
        endpoint: String,
    },

    /// Transport error (connection, accept, peer verification).
    #[error(transparent)]
    Transport(#[from] PlatformError),

    /// Read or write failure on an established connection.
    #[error("I/O error on the IPC connection")]
    Io(#[source] io::Error),

    /// The peer closed the connection.
    #[error("the IPC connection was closed by the peer")]
    ConnectionClosed,

    /// A frame exceeds [`crate::framing::MAX_FRAME_LEN`].
    #[error("IPC frame of {len} bytes exceeds the limit of {max} bytes")]
    FrameTooLarge {
        /// Announced or actual payload length.
        len: usize,
        /// Maximum accepted length.
        max: usize,
    },

    /// A frame does not contain a valid message.
    #[error("failed to decode IPC message")]
    Decode(#[source] serde_json::Error),

    /// A message could not be serialized.
    #[error("failed to encode IPC message")]
    Encode(#[source] serde_json::Error),

    /// An operation did not complete in time.
    #[error("timed out after {}s while {operation}", timeout.as_secs())]
    Timeout {
        /// What was being waited for.
        operation: &'static str,
        /// Elapsed limit.
        timeout: Duration,
    },

    /// The peer sent a message that is not valid at this point.
    #[error("unexpected IPC message: expected {expected}")]
    UnexpectedMessage {
        /// Expected message.
        expected: &'static str,
    },

    /// The peers speak different protocol versions.
    #[error(
        "incompatible IPC protocol: this program speaks version {local}, the peer speaks \
         version {peer}; restart the daemon after upgrading DevSpace"
    )]
    IncompatibleProtocol {
        /// Version of this program.
        local: u32,
        /// Version of the peer.
        peer: u32,
    },
}
