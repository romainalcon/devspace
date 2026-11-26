//! Connection of the desktop application to the daemon.
//!
//! Every operation opens its own IPC connection: the daemon serves
//! connections concurrently, so a download running on one connection never
//! delays the periodic refresh of the interface.

use std::sync::{Mutex, PoisonError};

use devspace_ipc::launcher::{self, LaunchError};
use devspace_ipc::{
    DaemonStatus, Endpoint, IpcError, PeerInfo, ProjectInfo, Request, Response, WebStatus,
};
use devspace_platform::{AppPaths, PlatformError};
use serde::Serialize;

/// State of the daemon as shown by the interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DaemonView {
    /// Accepting requests.
    Running {
        /// Status reported by the daemon.
        #[serde(flatten)]
        status: DaemonStatus,
    },
    /// Not running.
    Stopped {
        /// Why the last start requested by the application failed.
        last_error: Option<String>,
    },
    /// The application is starting it.
    Starting,
    /// The application is stopping it.
    Stopping,
    /// Its state could not be determined (e.g. a process holds the lock
    /// without answering).
    Unknown {
        /// What happened.
        message: String,
    },
}

/// Start or stop requested by the application and not finished yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    Starting,
    Stopping,
}

/// Error of a desktop command, shown to the user.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    /// The daemon could not be reached, started or stopped.
    #[error(transparent)]
    Launch(#[from] LaunchError),

    /// IPC error.
    #[error(transparent)]
    Ipc(#[from] IpcError),

    /// Platform error (directories, endpoint).
    #[error(transparent)]
    Platform(#[from] PlatformError),

    /// The daemon is not running.
    #[error("the DevSpace daemon is not running")]
    NotRunning,

    /// Another start or stop is in progress.
    #[error("the daemon is already being started or stopped")]
    Busy,

    /// The request is not available through the generic command.
    #[error("`{method}` cannot be sent directly; use the daemon controls")]
    Forbidden {
        /// Method of the request.
        method: &'static str,
    },

    /// The daemon answered a request with something else.
    #[error("unexpected answer from the daemon: expected {expected}")]
    Unexpected {
        /// Expected response.
        expected: &'static str,
    },

    /// The daemon refused or failed the request.
    #[error("{message}")]
    Refused {
        /// Explanation sent by the daemon.
        message: String,
    },

    /// Nothing is known under that name.
    #[error("no project named `{name}`")]
    UnknownProject {
        /// Project name.
        name: String,
    },

    /// A desktop integration (window, opener) failed.
    #[error("{0}")]
    Shell(String),
}

impl DesktopError {
    /// The error and its causes, one per line, for the interface.
    #[must_use]
    pub fn report(&self) -> String {
        let mut out = self.to_string();
        let mut source = std::error::Error::source(self);
        while let Some(cause) = source {
            out.push_str("\n  caused by: ");
            out.push_str(&cause.to_string());
            source = cause.source();
        }
        out
    }
}

/// Everything needed to talk to the daemon.
#[derive(Debug)]
pub struct Daemon {
    paths: AppPaths,
    endpoint: Endpoint,
    peer: PeerInfo,
    transition: Mutex<Option<Transition>>,
    last_error: Mutex<Option<String>>,
    /// Held while probing the daemon and during a start or stop: the probe
    /// briefly takes the instance lock when it is free, which would make a
    /// concurrent start fail (`InstanceLock::is_held`).
    probe: tokio::sync::Mutex<()>,
}

impl Daemon {
    /// Resolves the DevSpace directories (honouring `DEVSPACE_HOME`) and
    /// the daemon endpoint.
    ///
    /// # Errors
    ///
    /// Returns the platform errors of [`AppPaths::resolve`] and
    /// [`Endpoint::new`].
    pub fn new() -> Result<Self, DesktopError> {
        let paths = AppPaths::resolve()?;
        let endpoint = Endpoint::new(&paths.runtime_dir)?;
        Ok(Self {
            paths,
            endpoint,
            peer: PeerInfo {
                name: "devspace-desktop".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            transition: Mutex::new(None),
            last_error: Mutex::new(None),
            probe: tokio::sync::Mutex::new(()),
        })
    }

    /// Current state of the daemon.
    pub async fn view(&self) -> DaemonView {
        if let Some(view) = self.transition_view() {
            return view;
        }
        let _probe = self.probe.lock().await;
        // A start or stop may have begun while waiting.
        if let Some(view) = self.transition_view() {
            return view;
        }
        match launcher::status(&self.paths, &self.endpoint, &self.peer).await {
            Ok(Some(status)) => DaemonView::Running { status },
            Ok(None) => DaemonView::Stopped {
                last_error: self
                    .last_error
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
            },
            Err(err) => DaemonView::Unknown {
                message: DesktopError::from(err).report(),
            },
        }
    }

    /// Starts the daemon (`devspaced` next to the application) unless it
    /// runs, and returns its state.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopError::Busy`] during another start or stop, and the
    /// errors of [`launcher::start`]; the failure is also kept for
    /// [`DaemonView::Stopped`].
    pub async fn start(&self) -> Result<DaemonView, DesktopError> {
        let guard = self.begin(Transition::Starting)?;
        let probe = self.probe.lock().await;
        let result = async {
            let executable = launcher::daemon_executable()?;
            launcher::start(&self.paths, &self.endpoint, &self.peer, &executable).await
        }
        .await;
        drop(probe);
        let outcome = match result {
            Ok(started) => {
                tracing::info!(
                    pid = started.pid,
                    already_running = started.already_running,
                    "daemon ready"
                );
                Ok(())
            }
            Err(err) => {
                let err = DesktopError::from(err);
                tracing::warn!(error = %err.report(), "daemon start failed");
                Err(err)
            }
        };
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            outcome.as_ref().err().map(DesktopError::report);
        outcome?;
        drop(guard);
        Ok(self.view().await)
    }

    /// Stops the daemon and returns its state.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopError::Busy`] during another start or stop, and the
    /// errors of [`launcher::stop`].
    pub async fn stop(&self) -> Result<DaemonView, DesktopError> {
        let guard = self.begin(Transition::Stopping)?;
        let probe = self.probe.lock().await;
        let pid = launcher::stop(&self.paths, &self.endpoint, &self.peer).await?;
        drop(probe);
        tracing::info!(?pid, "daemon stopped");
        drop(guard);
        Ok(self.view().await)
    }

    /// Sends `request` to the daemon on a new connection.
    ///
    /// Error responses are returned as [`Response::Error`]; the daemon
    /// lifecycle requests go through [`Daemon::start`] and
    /// [`Daemon::stop`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopError::Forbidden`] for `shutdown`,
    /// [`DesktopError::NotRunning`], and IPC errors.
    pub async fn request(&self, request: Request) -> Result<Response, DesktopError> {
        if request == Request::Shutdown {
            return Err(DesktopError::Forbidden { method: "shutdown" });
        }
        let mut client = launcher::try_connect(&self.endpoint, &self.peer)
            .await?
            .ok_or(DesktopError::NotRunning)?;
        Ok(client.request(request).await?)
    }

    /// URL of the site of a registered project.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopError::UnknownProject`] when no site has that name,
    /// and the errors of [`Daemon::request`].
    pub async fn site_url(&self, project: &str) -> Result<String, DesktopError> {
        let status = match self.request(Request::WebStatus).await? {
            Response::WebStatus(status) => status,
            response => return Err(refusal(response, "web status")),
        };
        site_url(&status, project).ok_or_else(|| DesktopError::UnknownProject {
            name: project.to_owned(),
        })
    }

    /// A registered project.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopError::UnknownProject`] and the errors of
    /// [`Daemon::request`].
    pub async fn project(&self, name: &str) -> Result<ProjectInfo, DesktopError> {
        let projects = match self.request(Request::ProjectList).await? {
            Response::Projects { projects } => projects,
            response => return Err(refusal(response, "project list")),
        };
        projects
            .into_iter()
            .find(|project| project.name == name)
            .ok_or_else(|| DesktopError::UnknownProject {
                name: name.to_owned(),
            })
    }

    fn transition_view(&self) -> Option<DaemonView> {
        self.current_transition()
            .map(|transition| match transition {
                Transition::Starting => DaemonView::Starting,
                Transition::Stopping => DaemonView::Stopping,
            })
    }

    fn current_transition(&self) -> Option<Transition> {
        *self
            .transition
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn begin(&self, transition: Transition) -> Result<TransitionGuard<'_>, DesktopError> {
        let mut current = self
            .transition
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if current.is_some() {
            return Err(DesktopError::Busy);
        }
        *current = Some(transition);
        Ok(TransitionGuard(&self.transition))
    }
}

/// Clears the transition when the start or stop ends, even on error.
struct TransitionGuard<'a>(&'a Mutex<Option<Transition>>);

impl Drop for TransitionGuard<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

fn refusal(response: Response, expected: &'static str) -> DesktopError {
    match response {
        Response::Error(error) => DesktopError::Refused {
            message: error.message,
        },
        _ => DesktopError::Unexpected { expected },
    }
}

/// `https://<domain>` of the site of `project`, as printed by
/// `devspace web status`.
fn site_url(status: &WebStatus, project: &str) -> Option<String> {
    status
        .sites
        .iter()
        .find(|site| site.project == project)
        .map(|site| format!("https://{}", site.domain))
}

#[cfg(test)]
mod tests {
    use devspace_ipc::{
        DnsServerState, ErrorCode, ErrorResponse, IntegrationStatus, SiteInfo, SiteTargetInfo,
        SystemIntegrations, WebServerState,
    };

    use super::*;

    fn web_status() -> WebStatus {
        WebStatus {
            caddy_version: "2.11.9".into(),
            caddy_installed: true,
            server: WebServerState::Stopped,
            tld: "internal".into(),
            address: "127.0.0.2".into(),
            http_port: 47080,
            https_port: 47443,
            dns_port: 47053,
            dns: DnsServerState::Listening,
            certificate_authority: None,
            system: SystemIntegrations {
                resolver: IntegrationStatus::Missing,
                port_forwarding: IntegrationStatus::Missing,
                certificate_trust: IntegrationStatus::Missing,
                forwarding_active: None,
            },
            sites: vec![SiteInfo {
                project: "blog".into(),
                domain: "blog.internal".into(),
                target: SiteTargetInfo::Proxy {
                    upstream: "127.0.0.1:5173".into(),
                },
            }],
            php: Vec::new(),
        }
    }

    #[test]
    fn site_urls_come_from_the_daemon() {
        let status = web_status();
        assert_eq!(
            site_url(&status, "blog").as_deref(),
            Some("https://blog.internal")
        );
        assert_eq!(site_url(&status, "shop"), None);
    }

    #[test]
    fn view_wire_format() {
        let running = DaemonView::Running {
            status: DaemonStatus {
                version: "0.1.0".into(),
                pid: 42,
                uptime_secs: 7,
            },
        };
        assert_eq!(
            serde_json::to_value(&running).unwrap(),
            serde_json::json!({"state": "running", "version": "0.1.0", "pid": 42, "uptime_secs": 7})
        );
        assert_eq!(
            serde_json::to_value(DaemonView::Stopped { last_error: None }).unwrap(),
            serde_json::json!({"state": "stopped", "last_error": null})
        );
    }

    #[test]
    fn errors_are_reported_with_their_causes() {
        let err = DesktopError::Launch(LaunchError::DaemonExecutableNotFound {
            path: "/apps/devspaced".into(),
        });
        assert_eq!(
            err.report(),
            "daemon executable `/apps/devspaced` not found"
        );
        let refused = refusal(
            Response::Error(ErrorResponse {
                code: ErrorCode::NotFound,
                message: "no such thing".into(),
            }),
            "web status",
        );
        assert_eq!(refused.report(), "no such thing");
        assert!(matches!(
            refusal(Response::Pong, "web status"),
            DesktopError::Unexpected { .. }
        ));
    }

    #[test]
    fn one_transition_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_root(tmp.path());
        let daemon = Daemon {
            endpoint: Endpoint::new(&paths.runtime_dir).unwrap(),
            paths,
            peer: PeerInfo {
                name: "test".into(),
                version: "0".into(),
            },
            transition: Mutex::new(None),
            last_error: Mutex::new(None),
            probe: tokio::sync::Mutex::new(()),
        };
        let guard = daemon.begin(Transition::Starting).unwrap();
        assert!(matches!(
            daemon.begin(Transition::Stopping),
            Err(DesktopError::Busy)
        ));
        assert_eq!(daemon.current_transition(), Some(Transition::Starting));
        drop(guard);
        assert_eq!(daemon.current_transition(), None);
    }
}
