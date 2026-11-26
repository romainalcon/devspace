//! Client side of a connection.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::framing::{read_frame, write_frame};
use crate::protocol::{
    ClientMessage, DaemonInfo, PROTOCOL_VERSION, PeerInfo, Request, Response, ServerMessage,
};
use crate::{ClientStream, Endpoint, IpcError, Result};

/// Maximum time to reach the endpoint.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum time for the handshake once connected.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection to the daemon whose handshake succeeded.
#[derive(Debug)]
pub struct Client<S = ClientStream> {
    stream: S,
    daemon: DaemonInfo,
    next_id: u64,
}

impl Client<ClientStream> {
    /// Connects to the daemon at `endpoint` and performs the handshake.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::NotRunning`] when nothing listens on the endpoint,
    /// [`IpcError::Transport`] for other connection failures (including a
    /// server running as another user), and the errors of
    /// [`Client::handshake`].
    pub async fn connect(endpoint: &Endpoint, client: PeerInfo) -> Result<Self> {
        let stream = tokio::time::timeout(
            CONNECT_TIMEOUT,
            devspace_platform::local_socket::connect(endpoint),
        )
        .await
        .map_err(|_| IpcError::Timeout {
            operation: "connecting to the daemon",
            timeout: CONNECT_TIMEOUT,
        })?
        .map_err(|err| {
            if err.is_endpoint_absent() {
                IpcError::NotRunning {
                    endpoint: endpoint.to_string(),
                }
            } else {
                IpcError::Transport(err)
            }
        })?;
        Self::handshake(stream, client).await
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    /// Runs the client side of the handshake on an established stream.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::IncompatibleProtocol`] when the daemon speaks
    /// another version, [`IpcError::Timeout`], [`IpcError::ConnectionClosed`],
    /// [`IpcError::UnexpectedMessage`] or a framing error.
    pub async fn handshake(mut stream: S, client: PeerInfo) -> Result<Self> {
        let hello = ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            client,
        };
        let answer = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            write_frame(&mut stream, &hello).await?;
            read_frame(&mut stream).await
        })
        .await
        .map_err(|_| IpcError::Timeout {
            operation: "waiting for the daemon handshake",
            timeout: HANDSHAKE_TIMEOUT,
        })??;
        match answer {
            Some(ServerMessage::Welcome {
                protocol_version,
                daemon,
            }) if protocol_version == PROTOCOL_VERSION => Ok(Self {
                stream,
                daemon,
                next_id: 1,
            }),
            Some(
                ServerMessage::Welcome {
                    protocol_version, ..
                }
                | ServerMessage::Rejected {
                    protocol_version, ..
                },
            ) => Err(IpcError::IncompatibleProtocol {
                local: PROTOCOL_VERSION,
                peer: protocol_version,
            }),
            Some(ServerMessage::Response { .. }) => Err(IpcError::UnexpectedMessage {
                expected: "welcome",
            }),
            None => Err(IpcError::ConnectionClosed),
        }
    }

    /// Identity announced by the daemon during the handshake.
    #[must_use]
    pub fn daemon(&self) -> &DaemonInfo {
        &self.daemon
    }

    /// Sends `request` and waits for its response, at most
    /// [`Request::time_limit`].
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::Timeout`] after the time limit,
    /// [`IpcError::ConnectionClosed`], [`IpcError::UnexpectedMessage`] for a
    /// response to another request, or a framing error.
    pub async fn request(&mut self, request: Request) -> Result<Response> {
        let timeout = request.time_limit();
        self.request_with_timeout(request, timeout).await
    }

    /// Like [`Client::request`], with a custom time limit for requests that
    /// must not wait as long as usual. After a timeout the
    /// connection must be dropped: a late response would be mistaken for
    /// the answer to the next request.
    ///
    /// # Errors
    ///
    /// Same as [`Client::request`], the timeout being `timeout`.
    pub async fn request_with_timeout(
        &mut self,
        request: Request,
        timeout: Duration,
    ) -> Result<Response> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let stream = &mut self.stream;
        let answer = tokio::time::timeout(timeout, async {
            write_frame(stream, &ClientMessage::Request { id, request }).await?;
            read_frame(stream).await
        })
        .await
        .map_err(|_| IpcError::Timeout {
            operation: "waiting for the daemon response",
            timeout,
        })??;
        match answer {
            Some(ServerMessage::Response {
                id: answered,
                response,
            }) if answered == id => Ok(response),
            Some(_) => Err(IpcError::UnexpectedMessage {
                expected: "response to the pending request",
            }),
            None => Err(IpcError::ConnectionClosed),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::DuplexStream;

    use super::*;
    use crate::ServerConnection;

    fn cli() -> PeerInfo {
        PeerInfo {
            name: "test-client".into(),
            version: "1.2.3".into(),
        }
    }

    fn daemon() -> DaemonInfo {
        DaemonInfo {
            version: "0.1.0".into(),
            pid: 4242,
        }
    }

    #[tokio::test]
    async fn handshake_and_requests_roundtrip() {
        let (client_side, server_side) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut conn = ServerConnection::handshake(server_side, daemon())
                .await
                .unwrap();
            assert_eq!(conn.client(), &cli());
            while let Some((id, request)) = conn.next_request().await.unwrap() {
                let response = match request {
                    Request::Ping => Response::Pong,
                    _ => Response::ShuttingDown,
                };
                conn.respond(id, response).await.unwrap();
            }
        });

        let mut client = Client::handshake(client_side, cli()).await.unwrap();
        assert_eq!(client.daemon(), &daemon());
        assert_eq!(client.request(Request::Ping).await.unwrap(), Response::Pong);
        assert_eq!(
            client.request(Request::Shutdown).await.unwrap(),
            Response::ShuttingDown
        );
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn server_rejects_other_protocol_versions() {
        let (mut client_side, server_side) = tokio::io::duplex(1024);
        let server = tokio::spawn(ServerConnection::handshake(server_side, daemon()));
        let hello = ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
            client: cli(),
        };
        write_frame(&mut client_side, &hello).await.unwrap();
        let answer: ServerMessage = read_frame(&mut client_side).await.unwrap().unwrap();
        assert!(
            matches!(answer, ServerMessage::Rejected { protocol_version, .. }
            if protocol_version == PROTOCOL_VERSION)
        );
        assert!(matches!(
            server.await.unwrap(),
            Err(IpcError::IncompatibleProtocol { .. })
        ));
    }

    #[tokio::test]
    async fn client_detects_rejection() {
        let (client_side, mut server_side) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let _: Option<ClientMessage> = read_frame(&mut server_side).await.unwrap();
            let rejection = ServerMessage::Rejected {
                protocol_version: 99,
                message: "too new".into(),
            };
            write_frame(&mut server_side, &rejection).await.unwrap();
        });
        let err = Client::handshake(client_side, cli()).await.unwrap_err();
        assert!(matches!(
            err,
            IpcError::IncompatibleProtocol {
                local: PROTOCOL_VERSION,
                peer: 99
            }
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_before_hello_is_refused() {
        let (mut client_side, server_side) = tokio::io::duplex(1024);
        let server = tokio::spawn(ServerConnection::handshake(server_side, daemon()));
        let request = ClientMessage::Request {
            id: 1,
            request: Request::Shutdown,
        };
        write_frame(&mut client_side, &request).await.unwrap();
        assert!(matches!(
            server.await.unwrap(),
            Err(IpcError::UnexpectedMessage { .. })
        ));
    }

    #[tokio::test]
    async fn mismatched_response_id_is_refused() {
        let (client_side, server_side) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut conn: ServerConnection<DuplexStream> =
                ServerConnection::handshake(server_side, daemon())
                    .await
                    .unwrap();
            let (id, _) = conn.next_request().await.unwrap().unwrap();
            conn.respond(id + 1, Response::Pong).await.unwrap();
        });
        let mut client = Client::handshake(client_side, cli()).await.unwrap();
        assert!(matches!(
            client.request(Request::Ping).await,
            Err(IpcError::UnexpectedMessage { .. })
        ));
        server.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn silent_client_times_out() {
        let (_client_side, server_side) = tokio::io::duplex(1024);
        let err = ServerConnection::handshake(server_side, daemon())
            .await
            .unwrap_err();
        assert!(matches!(err, IpcError::Timeout { .. }));
    }
}
