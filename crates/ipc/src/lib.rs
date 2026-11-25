//! IPC protocol between the DevSpace daemon and its clients.
//!
//! The daemon is the single source of truth; the CLI and the desktop
//! application talk to it exclusively through this crate.
//!
//! Layers:
//! - transport ([`Endpoint`], [`Listener`]): user-restricted local sockets
//!   with peer identity checks, provided by `devspace-platform`;
//! - [`framing`]: length-prefixed JSON frames;
//! - [`protocol`]: versioned messages and handshake;
//! - [`Client`] / [`ServerConnection`]: typed endpoints of a connection.
//!
//! A connection starts with a handshake: the client sends
//! [`ClientMessage::Hello`] with its [`PROTOCOL_VERSION`]; the daemon answers
//! [`ServerMessage::Welcome`] or [`ServerMessage::Rejected`] and closes the
//! connection when versions differ. Requests then follow, each answered by a
//! response carrying the same `id`.

pub mod client;
pub mod error;
pub mod framing;
pub mod protocol;
pub mod server;

pub use client::Client;
pub use devspace_platform::local_socket::{ClientStream, Endpoint, Listener, ServerStream};
pub use error::{IpcError, Result};
pub use protocol::{
    ClientMessage, DaemonInfo, DaemonStatus, ErrorCode, ErrorResponse, PROTOCOL_VERSION, PeerInfo,
    ProjectInfo, Request, Response, ServerMessage,
};
pub use server::ServerConnection;
