//! Daemon side of a connection.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::framing::{read_frame, write_frame};
use crate::protocol::{
    ClientMessage, DaemonInfo, PROTOCOL_VERSION, PeerInfo, Request, Response, ServerMessage,
};
use crate::{IpcError, Result, ServerStream};

/// Time a client has to complete the handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection whose handshake succeeded.
#[derive(Debug)]
pub struct ServerConnection<S = ServerStream> {
    stream: S,
    client: PeerInfo,
}

impl<S: AsyncRead + AsyncWrite + Unpin> ServerConnection<S> {
    /// Runs the server side of the handshake on a freshly accepted stream.
    ///
    /// A client speaking another protocol version receives
    /// [`ServerMessage::Rejected`] before the error is returned.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::Timeout`] if no `hello` arrives within
    /// [`HANDSHAKE_TIMEOUT`], [`IpcError::IncompatibleProtocol`] on version
    /// mismatch, [`IpcError::UnexpectedMessage`] if the first message is not
    /// `hello`, or a framing error.
    pub async fn handshake(mut stream: S, daemon: DaemonInfo) -> Result<Self> {
        let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut stream))
            .await
            .map_err(|_| IpcError::Timeout {
                operation: "waiting for the client handshake",
                timeout: HANDSHAKE_TIMEOUT,
            })??;
        match first {
            None => Err(IpcError::ConnectionClosed),
            Some(ClientMessage::Hello {
                protocol_version,
                client,
            }) => {
                if protocol_version != PROTOCOL_VERSION {
                    let rejection = ServerMessage::Rejected {
                        protocol_version: PROTOCOL_VERSION,
                        message: format!(
                            "daemon speaks protocol version {PROTOCOL_VERSION}, \
                             client speaks version {protocol_version}"
                        ),
                    };
                    write_frame(&mut stream, &rejection).await?;
                    return Err(IpcError::IncompatibleProtocol {
                        local: PROTOCOL_VERSION,
                        peer: protocol_version,
                    });
                }
                let welcome = ServerMessage::Welcome {
                    protocol_version: PROTOCOL_VERSION,
                    daemon,
                };
                write_frame(&mut stream, &welcome).await?;
                Ok(Self { stream, client })
            }
            Some(ClientMessage::Request { .. }) => {
                Err(IpcError::UnexpectedMessage { expected: "hello" })
            }
        }
    }

    /// Identity announced by the client.
    #[must_use]
    pub fn client(&self) -> &PeerInfo {
        &self.client
    }

    /// Waits for the next request. Returns `Ok(None)` when the client closed
    /// the connection.
    ///
    /// Not cancellation-safe: if the future is dropped mid-frame, the
    /// connection must be discarded.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::UnexpectedMessage`] for a second `hello`, or a
    /// framing error.
    pub async fn next_request(&mut self) -> Result<Option<(u64, Request)>> {
        match read_frame(&mut self.stream).await? {
            None => Ok(None),
            Some(ClientMessage::Request { id, request }) => Ok(Some((id, request))),
            Some(ClientMessage::Hello { .. }) => Err(IpcError::UnexpectedMessage {
                expected: "request",
            }),
        }
    }

    /// Sends the response to request `id`.
    ///
    /// # Errors
    ///
    /// Returns a framing error.
    pub async fn respond(&mut self, id: u64, response: Response) -> Result<()> {
        write_frame(&mut self.stream, &ServerMessage::Response { id, response }).await
    }
}
