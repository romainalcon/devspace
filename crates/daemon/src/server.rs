//! IPC server loop of the daemon.
//!
//! Startup binds the IPC endpoint, then starts the DNS server and, in the
//! background, the web server (when installed) and the service instances
//! to run. The accept loop runs until
//! a client sends [`Request::Shutdown`] or the process receives a shutdown
//! signal. Shutdown then proceeds in order: stop accepting and remove the
//! endpoint, notify open connections, wait up to [`DRAIN_TIMEOUT`] for them
//! to finish, abort the rest, stop the web server, the PHP-FPM masters and
//! the service servers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use devspace_core::AppPaths;
use devspace_core::db::Database;
use devspace_core::error::report;
use devspace_ipc::{DnsServerState, Endpoint, Listener, Request, ServerConnection, ServerStream};
use devspace_platform::PlatformError;
use devspace_platform::signals::ShutdownSignals;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tracing::Instrument as _;

use crate::dns;
use crate::error::DaemonError;
use crate::handlers::{self, DaemonState};
use crate::runtimes::Runtimes;
use crate::services::{self, Services};
use crate::web::WebServer;

/// Maximum time given to open connections once shutdown has started.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause after an accept failure, so that a persistent error (e.g. file
/// descriptor exhaustion) does not turn into a busy loop.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Serves IPC requests until shutdown.
pub(crate) async fn run(
    paths: &AppPaths,
    db: Database,
    web: WebServer,
    runtimes: Runtimes,
    services: Services,
    remote_index: bool,
) -> Result<(), DaemonError> {
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

    let state = Arc::new(DaemonState::new(db, web, runtimes, services));
    let dns_port = state.web.config().dns_port;
    let dns_task = match dns::bind(dns_port).await {
        Ok(socket) => {
            tracing::info!(port = dns_port, "DNS server listening on 127.0.0.1");
            state.set_dns_state(DnsServerState::Listening);
            Some(tokio::spawn(dns::serve(socket, state.web.config().clone())))
        }
        Err(err) => {
            let message = format!(
                "cannot listen on 127.0.0.1:{dns_port} (UDP): {err}; free the port or change \
                 `web.dns_port` in the configuration"
            );
            tracing::error!(error = %message, "DNS server unavailable");
            state.set_dns_state(DnsServerState::Failed { message });
            None
        }
    };
    let index_task = remote_index
        .then(|| tokio::spawn(crate::runtimes::refresh_periodically(Arc::clone(&state))));
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move { handlers::autostart_web(&state).await });
    }
    services::apply_in_background(&state);

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
    state.web.stop().await;
    state.runtimes.php.stop_all().await;
    state.services.stop_all().await;
    for task in [dns_task, index_task].into_iter().flatten() {
        task.abort();
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
        let is_shutdown = request == Request::Shutdown;
        let response = handlers::handle(request, &state).await;
        if let Err(err) = conn.respond(id, response).await {
            tracing::warn!(error = %report(&err), "failed to send response");
            break;
        }
        if is_shutdown {
            shutdown.send_replace(true);
            break;
        }
    }
    tracing::debug!("client disconnected");
}
