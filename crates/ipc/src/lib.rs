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
    CertificateAuthorityInfo, ClientMessage, DaemonInfo, DaemonStatus, DnsServerState, ErrorCode,
    ErrorResponse, IntegrationStatus, PROTOCOL_VERSION, PeerInfo, PhpPoolInfo, ProjectInfo,
    Request, Response, RuntimeEnv, RuntimeEnvEntry, RuntimeInfo, RuntimeList, RuntimeVersionInfo,
    ServerMessage, ServiceInstanceInfo, ServiceList, ServiceMode, SiteInfo, SiteTargetInfo,
    SystemIntegrations, WebServerState, WebStatus,
};
pub use server::ServerConnection;
