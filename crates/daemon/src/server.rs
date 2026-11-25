//! IPC server loop of the daemon.
//!
//! The accept loop runs until a client sends [`Request::Shutdown`] or the
//! process receives a shutdown signal. Shutdown then proceeds in order:
//! stop accepting and remove the endpoint, notify open connections, wait up
//! to [`DRAIN_TIMEOUT`] for them to finish, abort the rest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use devspace_core::AppPaths;
use devspace_core::error::report;
use devspace_ipc::{
    DaemonInfo, DaemonStatus, Endpoint, Listener, Request, Response, ServerConnection, ServerStream,
};
use devspace_platform::PlatformError;
use devspace_platform::signals::ShutdownSignals;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tracing::Instrument as _;

use crate::error::DaemonError;

/// Maximum time given to open connections once shutdown has started.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause after an accept failure, so that a persistent error (e.g. file
/// descriptor exhaustion) does not turn into a busy loop.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// State shared by all connections.
#[derive(Debug)]
struct DaemonState {
    started: Instant,
    pid: u32,
}

impl DaemonState {
    fn info(&self) -> DaemonInfo {
        DaemonInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: self.pid,
        }
    }

    fn status(&self) -> DaemonStatus {
        DaemonStatus {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: self.pid,
            uptime_secs: self.started.elapsed().as_secs(),
        }
    }
}

/// Serves IPC requests until shutdown.
pub(crate) async fn run(paths: &AppPaths) -> Result<(), DaemonError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let mut listener = Listener::bind(&endpoint)?;
    let mut signals = match ShutdownSignals::install() {
        Ok(signals) => Some(signals),
        Err(err) => {
            tracing::warn!(
                error = %report(&err),
                "signal handlers unavailable; the daemon can only be stopped through IPC"
            );
            None
        }
    };

    let state = Arc::new(DaemonState {
        started: Instant::now(),
        pid: std::process::id(),
    });
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);
    let mut connections = JoinSet::new();
    let next_connection_id = AtomicU64::new(1);

    tracing::info!(endpoint = %endpoint, "daemon ready");

    loop {
        tokio::select! {
            signal = next_signal(&mut signals) => {
                tracing::info!(signal, "shutdown requested by signal");
                break;
            }
            _ = shutdown_rx.wait_for(|stop| *stop) => {
                tracing::info!("shutdown requested by a client");
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let id = next_connection_id.fetch_add(1, Ordering::Relaxed);
                    let span = tracing::info_span!("connection", id);
                    connections.spawn(
                        handle_connection(stream, Arc::clone(&state), Arc::clone(&shutdown_tx))
                            .instrument(span),
                    );
                }
                Err(err) => {
                    tracing::warn!(error = %report(&err), "incoming connection rejected");
                    if matches!(err, PlatformError::Accept { .. }) {
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                    }
                }
            },
            Some(joined) = connections.join_next() => log_connection_end(joined),
        }
    }

    // Stop accepting first: dropping the listener removes the endpoint, so
    // clients see "not running" from now on.
    drop(listener);
    shutdown_tx.send_replace(true);
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while let Some(joined) = connections.join_next().await {
            log_connection_end(joined);
        }
    })
    .await;
    if drained.is_err() {
        tracing::warn!(
            remaining = connections.len(),
            timeout_secs = DRAIN_TIMEOUT.as_secs(),
            "connections still open after the drain timeout; aborting them"
        );
        connections.abort_all();
    }
    Ok(())
}

async fn next_signal(signals: &mut Option<ShutdownSignals>) -> &'static str {
    match signals {
        Some(signals) => signals.recv().await,
        None => std::future::pending().await,
    }
}

fn log_connection_end(joined: Result<(), JoinError>) {
    if let Err(err) = joined
        && err.is_panic()
    {
        tracing::error!(error = %err, "connection task panicked");
    }
}

async fn handle_connection(
    stream: ServerStream,
    state: Arc<DaemonState>,
    shutdown: Arc<watch::Sender<bool>>,
) {
    let mut conn = match ServerConnection::handshake(stream, state.info()).await {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!(error = %report(&err), "handshake failed");
            return;
        }
    };
    tracing::debug!(
        client = %conn.client().name,
        client_version = %conn.client().version,
        "client connected"
    );

    let mut shutdown_rx = shutdown.subscribe();
    loop {
        let next = tokio::select! {
            _ = shutdown_rx.wait_for(|stop| *stop) => break,
            next = conn.next_request() => next,
        };
        let (id, request) = match next {
            Ok(Some(next)) => next,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %report(&err), "dropping connection after protocol error");
                break;
            }
        };
        tracing::debug!(id, ?request, "request received");
        let response = match request {
            Request::Ping => Response::Pong,
            Request::Status => Response::Status(state.status()),
            Request::Shutdown => Response::ShuttingDown,
        };
        if let Err(err) = conn.respond(id, response).await {
            tracing::warn!(error = %report(&err), "failed to send response");
            break;
        }
        if request == Request::Shutdown {
            shutdown.send_replace(true);
            break;
        }
    }
    tracing::debug!("client disconnected");
}
