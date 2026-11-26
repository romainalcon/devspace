//! Request handlers of the daemon.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use devspace_core::db::Database;
use devspace_core::error::report;
use devspace_core::project::{Project, ProjectName};
use devspace_core::runtime::InstalledRuntime;
use devspace_core::service::{InstanceKey, ServiceMode};
use devspace_core::web::site::{self, Runtimes as SiteRuntimes, Site, SiteTarget};
use devspace_core::web::{ca, release};
use devspace_ipc::{
    CertificateAuthorityInfo, DaemonInfo, DaemonStatus, DnsServerState, ErrorCode, ErrorResponse,
    IntegrationStatus, ProjectInfo, Request, Response, SiteInfo, SiteTargetInfo,
    SystemIntegrations, WebServerState, WebStatus,
};
use devspace_platform::PlatformError;
use devspace_platform::system::port_forwarding::Forwarding;
use devspace_platform::system::{IntegrationState, port_forwarding, resolver, trust_store};

use crate::runtimes::{self, Runtimes};
use crate::services::{self, Services};
use crate::web::{WebError, WebServer};

/// State shared by all connections.
#[derive(Debug)]
pub(crate) struct DaemonState {
    started: Instant,
    pid: u32,
    /// The connection is synchronous; it is only used from blocking tasks.
    db: Mutex<Database>,
    pub(crate) web: WebServer,
    pub(crate) runtimes: Runtimes,
    pub(crate) services: Services,
    dns: Mutex<DnsServerState>,
}

impl DaemonState {
    pub(crate) fn new(
        db: Database,
        web: WebServer,
        runtimes: Runtimes,
        services: Services,
    ) -> Self {
        Self {
            started: Instant::now(),
            pid: std::process::id(),
            db: Mutex::new(db),
            web,
            runtimes,
            services,
            dns: Mutex::new(DnsServerState::Failed {
                message: "not started".to_owned(),
            }),
        }
    }

    pub(crate) fn info(&self) -> DaemonInfo {
        DaemonInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: self.pid,
        }
    }

    pub(crate) fn set_dns_state(&self, state: DnsServerState) {
        *self.dns.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }

    fn status(&self) -> DaemonStatus {
        DaemonStatus {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: self.pid,
            uptime_secs: self.started.elapsed().as_secs(),
        }
    }

    /// Locks the database. A poisoned lock means that a previous holder
    /// panicked; SQLite rolled back its unfinished transaction, but the
    /// daemon should be restarted.
    fn db(&self) -> Result<MutexGuard<'_, Database>, Response> {
        self.db.lock().map_err(|_| {
            tracing::error!("database lock poisoned");
            internal_error("the daemon database is unavailable; restart the daemon")
        })
    }

    /// Registered projects (blocking).
    pub(crate) fn projects(&self) -> Result<Vec<Project>, Response> {
        self.db()?
            .list_projects()
            .map_err(|err| error_response(&err))
    }

    /// Choices of the user for the service instances (blocking).
    pub(crate) fn service_modes(&self) -> Result<Vec<(InstanceKey, ServiceMode)>, Response> {
        self.db()?
            .service_modes()
            .map_err(|err| error_response(&err))
    }

    /// Records a choice of the user for a service instance (blocking).
    pub(crate) fn set_service_mode(
        &self,
        key: InstanceKey,
        mode: Option<ServiceMode>,
    ) -> Result<(), Response> {
        self.db()?
            .set_service_mode(key, mode)
            .map_err(|err| error_response(&err))
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
            let modifies = !matches!(request, Request::ProjectList);
            let response =
                blocking(state, move |state| handle_project_request(request, state)).await;
            if modifies && !matches!(response, Response::Error(_)) {
                reload_in_background(state);
                services::apply_in_background(state);
            }
            response
        }
        Request::WebInstall => match state.web.install().await {
            Ok(_) => web_status(state).await,
            Err(err) => web_error(&err),
        },
        Request::WebStart | Request::WebRestart => {
            match apply_sites(state, matches!(request, Request::WebRestart)).await {
                Ok(()) => web_status(state).await,
                Err(response) => response,
            }
        }
        Request::WebStop => {
            state.web.stop().await;
            state.runtimes.php.stop_all().await;
            web_status(state).await
        }
        Request::WebStatus => web_status(state).await,
        Request::RuntimeList => runtimes::list(state).await,
        Request::IndexRefresh => runtimes::refresh_index(state).await,
        Request::RuntimeInstall { name, version } => {
            runtimes::install(state, &name, version.as_deref()).await
        }
        Request::RuntimeRemove { name, version } => runtimes::remove(state, &name, &version).await,
        Request::RuntimeEnv { directory } => runtimes::env(state, directory).await,
        Request::ServiceList => services::list(state).await,
        Request::ServiceStart { name, series } => {
            services::set(state, &name, series.as_deref(), true).await
        }
        Request::ServiceStop { name, series } => {
            services::set(state, &name, series.as_deref(), false).await
        }
    }
}

/// Starts (or, with `restart`, restarts) the web server with the current
/// sites, after starting the PHP-FPM masters they need and stopping the
/// others.
async fn apply_sites(state: &Arc<DaemonState>, restart: bool) -> Result<(), Response> {
    let (sites, installed) = sites(state).await?;
    let php: BTreeSet<_> = sites
        .iter()
        .filter_map(|site| match site.target {
            SiteTarget::Php { version, .. } => Some(version),
            _ => None,
        })
        .collect();
    state.runtimes.php.sync(&php, &installed).await;
    let result = if restart {
        state.web.restart(&sites).await
    } else {
        state.web.start(&sites).await
    };
    result.map_err(|err| web_error(&err))
}

/// Runs `work` on a blocking thread.
pub(crate) async fn blocking<F>(state: &Arc<DaemonState>, work: F) -> Response
where
    F: FnOnce(&DaemonState) -> Response + Send + 'static,
{
    let state = Arc::clone(state);
    tokio::task::spawn_blocking(move || work(&state))
        .await
        .unwrap_or_else(|err| {
            tracing::error!(error = %err, "request task failed");
            internal_error("the request handler failed; see the daemon logs")
        })
}

/// Runs a registry request on the database (blocking).
fn handle_project_request(request: Request, state: &DaemonState) -> Response {
    let mut db = match state.db() {
        Ok(db) => db,
        Err(response) => return response,
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
        _ => return internal_error("not a project request"),
    };
    result.unwrap_or_else(|err| error_response(&err))
}

/// Sites of the registered projects, read from their manifests, and the
/// installed runtimes they were resolved with.
fn resolve_sites(state: &DaemonState) -> Result<(Vec<Site>, Vec<InstalledRuntime>), Response> {
    let projects = state.projects()?;
    let installed = state.runtimes.installed();
    let runtimes = SiteRuntimes {
        installed: &installed,
        layout: &state.runtimes.layout,
    };
    let tld = &state.web.config().tld;
    let sites = projects
        .iter()
        .map(|project| site::resolve(project, tld, runtimes))
        .collect();
    Ok((sites, installed))
}

async fn sites(state: &Arc<DaemonState>) -> Result<(Vec<Site>, Vec<InstalledRuntime>), Response> {
    let state = Arc::clone(state);
    tokio::task::spawn_blocking(move || resolve_sites(&state))
        .await
        .unwrap_or_else(|err| {
            tracing::error!(error = %err, "site resolution task failed");
            Err(internal_error(
                "the request handler failed; see the daemon logs",
            ))
        })
}

/// Starts the web server at daemon startup when it is installed; failures
/// are logged and reported by the web status.
pub(crate) async fn autostart_web(state: &Arc<DaemonState>) {
    if !release::is_installed(state.web.layout()) {
        tracing::info!("web server not installed; run `devspace web install`");
        return;
    }
    // Errors are logged and kept in the state by `start`.
    let _ = apply_sites(state, false).await;
}

/// Applies a change of the registry or of the installed runtimes to the
/// running web server.
pub(crate) fn reload_in_background(state: &Arc<DaemonState>) {
    if !state.web.is_running() {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        if let Err(response) = apply_sites(&state, true).await {
            tracing::error!(?response, "failed to reload the web server");
        }
    });
}

async fn web_status(state: &Arc<DaemonState>) -> Response {
    let server = state.web.state();
    let forwarding_active = if matches!(server, WebServerState::Running { .. }) {
        Some(state.web.probe_forwarding().await)
    } else {
        None
    };
    blocking(state, move |state| {
        collect_web_status(state, server, forwarding_active).unwrap_or_else(|response| response)
    })
    .await
}

/// Gathers the web status (blocking: database, files, system tools).
fn collect_web_status(
    state: &DaemonState,
    server: WebServerState,
    forwarding_active: Option<bool>,
) -> Result<Response, Response> {
    let web = &state.web;
    let config = web.config();
    let sites = resolve_sites(state)?.0.into_iter().map(site_info).collect();
    let local_ca = ca::load(web.layout()).unwrap_or_else(|err| {
        tracing::warn!(error = %report(&err), "cannot read the local certificate authority");
        None
    });

    let resolver = integration(resolver::state(config.tld.as_str(), config.dns_port));
    let port_forwarding = integration(
        Forwarding::new(config.address.ip(), config.http_port, config.https_port)
            .and_then(port_forwarding::state),
    );
    let certificate_trust = match &local_ca {
        Some(local_ca) => integration(trust_store::is_trusted(&local_ca.path).map(|trusted| {
            if trusted {
                IntegrationState::Installed
            } else {
                IntegrationState::Missing
            }
        })),
        None if trust_store::is_supported() => IntegrationStatus::Missing,
        None => IntegrationStatus::Unsupported,
    };

    let dns = state
        .dns
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    Ok(Response::WebStatus(Box::new(WebStatus {
        caddy_version: release::CADDY_VERSION.to_owned(),
        caddy_installed: release::is_installed(web.layout()),
        server,
        tld: config.tld.to_string(),
        address: config.address.to_string(),
        http_port: config.http_port,
        https_port: config.https_port,
        dns_port: config.dns_port,
        dns,
        certificate_authority: local_ca.map(|local_ca| CertificateAuthorityInfo {
            certificate_pem: local_ca.certificate.to_pem(),
            sha256: local_ca.certificate.fingerprint().to_string(),
        }),
        system: SystemIntegrations {
            resolver,
            port_forwarding,
            certificate_trust,
            forwarding_active,
        },
        sites,
        php: state.runtimes.php.status(),
    })))
}

fn integration(state: Result<IntegrationState, PlatformError>) -> IntegrationStatus {
    match state {
        Ok(IntegrationState::Installed) => IntegrationStatus::Installed,
        Ok(IntegrationState::Missing) => IntegrationStatus::Missing,
        Ok(IntegrationState::Outdated) => IntegrationStatus::Outdated,
        Ok(IntegrationState::Conflict) => IntegrationStatus::Conflict,
        Err(PlatformError::Unsupported { .. }) => IntegrationStatus::Unsupported,
        Err(err) => {
            tracing::warn!(error = %report(&err), "cannot determine a system integration state");
            IntegrationStatus::Unknown
        }
    }
}

fn site_info(site: Site) -> SiteInfo {
    SiteInfo {
        project: site.project.as_str().to_owned(),
        domain: site.domain,
        target: match site.target {
            SiteTarget::Static { root } => SiteTargetInfo::Static { root },
            SiteTarget::Php { root, version, .. } => SiteTargetInfo::Php {
                root,
                version: version.to_string(),
            },
            SiteTarget::Proxy { upstream } => SiteTargetInfo::Proxy {
                upstream: upstream.as_str().to_owned(),
            },
            SiteTarget::Unavailable { reason } => SiteTargetInfo::Unavailable { reason },
        },
    }
}

fn project_info(project: Project) -> ProjectInfo {
    ProjectInfo {
        name: project.name.as_str().to_owned(),
        path: project.path,
        added_at: project.added_at,
    }
}

fn web_error(err: &WebError) -> Response {
    let code = if err.is_precondition() {
        ErrorCode::Unavailable
    } else {
        ErrorCode::Failed
    };
    let message = report(err);
    tracing::warn!(error = %message, ?code, "web request failed");
    Response::Error(ErrorResponse { code, message })
}

pub(crate) fn error_response(err: &devspace_core::Error) -> Response {
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
        | E::InvalidManifest { .. }
        | E::UnsupportedManifestVersion { .. } => ErrorCode::InvalidRequest,
        E::ProjectNameTaken { .. } | E::ProjectPathTaken { .. } => ErrorCode::AlreadyExists,
        E::ProjectNotFound { .. }
        | E::RuntimeUnavailable { .. }
        | E::RuntimeNotInstalled { .. } => ErrorCode::NotFound,
        E::Download { .. }
        | E::DownloadRead { .. }
        | E::ChecksumMismatch { .. }
        | E::Archive { .. }
        | E::InstallFile { .. }
        | E::RuntimeCheck { .. }
        | E::ServiceInit { .. } => ErrorCode::Failed,
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
