//! Connection to the daemon, shared by the commands that use it.

use std::future::Future;
use std::time::Duration;

use devspace_ipc::{Client, Endpoint, ErrorResponse, IpcError, PeerInfo, Request, Response};

use crate::error::CliError;

/// Runs `future` to completion on a single-threaded Tokio runtime.
pub fn block_on<F: Future>(future: F) -> Result<F::Output, CliError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(CliError::Runtime)?;
    Ok(runtime.block_on(future))
}

fn peer_info() -> PeerInfo {
    PeerInfo {
        name: "devspace-cli".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Connects to the daemon; `Ok(None)` means that it is not running.
pub async fn try_connect(endpoint: &Endpoint) -> Result<Option<Client>, CliError> {
    match Client::connect(endpoint, peer_info()).await {
        Ok(client) => Ok(Some(client)),
        Err(IpcError::NotRunning { .. }) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Connects to the daemon, which must be running.
pub async fn connect(endpoint: &Endpoint) -> Result<Client, CliError> {
    try_connect(endpoint)
        .await?
        .ok_or(CliError::DaemonNotRunning)
}

/// Sends `request`, turning an error response into [`CliError::Refused`].
pub async fn request(client: &mut Client, request: Request) -> Result<Response, CliError> {
    refused_as_error(client.request(request).await?)
}

/// Like [`request`], with a custom time limit.
pub async fn request_with_timeout(
    client: &mut Client,
    request: Request,
    timeout: Duration,
) -> Result<Response, CliError> {
    refused_as_error(client.request_with_timeout(request, timeout).await?)
}

fn refused_as_error(response: Response) -> Result<Response, CliError> {
    match response {
        Response::Error(ErrorResponse { message, .. }) => Err(CliError::Refused { message }),
        response => Ok(response),
    }
}

/// Error for a response that does not answer the request.
pub fn unexpected(expected: &'static str) -> CliError {
    IpcError::UnexpectedMessage { expected }.into()
}
