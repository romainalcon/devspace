//! Request handlers of the daemon.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use devspace_core::db::Database;
use devspace_core::error::report;
use devspace_core::project::{Project, ProjectName};
use devspace_ipc::{
    DaemonInfo, DaemonStatus, ErrorCode, ErrorResponse, ProjectInfo, Request, Response,
};

/// State shared by all connections.
#[derive(Debug)]
pub(crate) struct DaemonState {
    started: Instant,
    pid: u32,
    /// The connection is synchronous; it is only used from blocking tasks.
    db: Mutex<Database>,
}

impl DaemonState {
    pub(crate) fn new(db: Database) -> Self {
        Self {
            started: Instant::now(),
            pid: std::process::id(),
            db: Mutex::new(db),
        }
    }

    pub(crate) fn info(&self) -> DaemonInfo {
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

/// Computes the response to `request`. [`Request::Shutdown`] is only
/// acknowledged here; the caller performs the shutdown.
pub(crate) async fn handle(request: Request, state: &Arc<DaemonState>) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Status => Response::Status(state.status()),
        Request::Shutdown => Response::ShuttingDown,
        Request::ProjectAdd { .. } | Request::ProjectList | Request::ProjectRemove { .. } => {
            let state = Arc::clone(state);
            tokio::task::spawn_blocking(move || handle_project_request(request, &state))
                .await
                .unwrap_or_else(|err| {
                    tracing::error!(error = %err, "project request task failed");
                    internal_error("the request handler failed; see the daemon logs")
                })
        }
    }
}

/// Runs a registry request on the database (blocking).
fn handle_project_request(request: Request, state: &DaemonState) -> Response {
    let Ok(mut db) = state.db.lock() else {
        // A previous holder panicked; the connection itself is still
        // consistent (SQLite rolls back unfinished transactions) but the
        // daemon should be restarted.
        tracing::error!("database lock poisoned");
        return internal_error("the daemon database is unavailable; restart the daemon");
    };
    let result = match request {
        Request::ProjectAdd { path, name } => name
            .as_deref()
            .map(ProjectName::parse)
            .transpose()
            .and_then(|name| db.add_project(&path, name))
            .map(|project| Response::ProjectAdded(project_info(project))),
        Request::ProjectList => db.list_projects().map(|projects| Response::Projects {
            projects: projects.into_iter().map(project_info).collect(),
        }),
        Request::ProjectRemove { name } => ProjectName::parse(&name)
            .and_then(|name| db.remove_project(&name))
            .map(|project| Response::ProjectRemoved(project_info(project))),
        Request::Ping | Request::Status | Request::Shutdown => {
            return internal_error("not a project request");
        }
    };
    result.unwrap_or_else(|err| error_response(&err))
}

fn project_info(project: Project) -> ProjectInfo {
    ProjectInfo {
        name: project.name.as_str().to_owned(),
        path: project.path,
        added_at: project.added_at,
    }
}

fn error_response(err: &devspace_core::Error) -> Response {
    use devspace_core::Error as E;

    let code = match err {
        E::InvalidProjectName { .. }
        | E::ProjectNameUnavailable { .. }
        | E::RelativeProjectPath { .. }
        | E::ProjectPath { .. }
        | E::ProjectNotDirectory { .. }
        | E::NonUnicodeProjectPath { .. }
        | E::ManifestRead { .. }
        | E::ManifestTooLarge { .. }
        | E::ManifestParse { .. }
        | E::UnsupportedManifestVersion { .. } => ErrorCode::InvalidRequest,
        E::ProjectNameTaken { .. } | E::ProjectPathTaken { .. } => ErrorCode::AlreadyExists,
        E::ProjectNotFound { .. } => ErrorCode::NotFound,
        _ => ErrorCode::Internal,
    };
    let message = report(err);
    if code == ErrorCode::Internal {
        tracing::error!(error = %message, "request failed");
    } else {
        tracing::debug!(error = %message, ?code, "request refused");
    }
    Response::Error(ErrorResponse { code, message })
}

fn internal_error(message: &str) -> Response {
    Response::Error(ErrorResponse {
        code: ErrorCode::Internal,
        message: message.to_owned(),
    })
}
