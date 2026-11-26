//! Supervision of the Caddy web server.
//!
//! The daemon starts Caddy as a child process running as the current user,
//! in its own session, with a configuration generated from the registered
//! projects. Changes are applied by restarting it (its admin API is
//! disabled). The child is supervised by [`crate::process`] (monitor task,
//! graceful stop, pid file for crash recovery).

use std::fs::{self, File};
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devspace_core::config::WebConfig;
use devspace_core::web::WebLayout;
use devspace_core::web::caddy_config::{self, PROBE_HEADER};
use devspace_core::web::release;
use devspace_core::web::site::Site;
use devspace_ipc::WebServerState;
use devspace_platform::fs::{FileMode, write_atomic};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::Child;

use crate::process::{self, ChildInfo, POLL_INTERVAL, SharedState, StopPolicy, Supervised};

/// Time given to Caddy to start accepting connections.
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// Time limit of the port forwarding probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Number of lines of the Caddy log quoted when it fails to start.
const LOG_TAIL_LINES: usize = 15;

/// Failure of a web server operation.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WebError {
    #[error(
        "Caddy {} is not installed; run `devspace web install`",
        release::CADDY_VERSION
    )]
    NotInstalled,

    #[error(transparent)]
    Core(#[from] devspace_core::Error),

    #[error(
        "port {port} of {address} is already in use; free it or change `{setting}` in the \
         configuration"
    )]
    PortInUse {
        address: String,
        port: u16,
        setting: &'static str,
    },

    #[error(
        "the sites address {address} is not configured on the loopback interface; run \
         `devspace system setup`"
    )]
    AddressUnavailable { address: String },

    #[error("failed to prepare `{}`", path.display())]
    Prepare {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to serialize the Caddy configuration")]
    Serialize(#[source] serde_json::Error),

    #[error("failed to start `{}`", path.display())]
    Spawn {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("the web server exited during startup ({status}); last lines of `{}`:\n{tail}", log.display())]
    Exited {
        status: ExitStatus,
        log: PathBuf,
        tail: String,
    },

    #[error("the web server did not accept connections within {}s; see `{}`", START_TIMEOUT.as_secs(), log.display())]
    NotReady { log: PathBuf },

    #[error("the installation task failed; see the daemon logs")]
    Task,
}

impl WebError {
    /// Whether the error comes from the state of the installation rather
    /// than from a failed operation.
    pub(crate) fn is_precondition(&self) -> bool {
        matches!(
            self,
            Self::NotInstalled
                | Self::AddressUnavailable { .. }
                | Self::Core(devspace_core::Error::CaddyUnavailable)
        )
    }
}

/// Supervisor of the web server.
#[derive(Debug)]
pub(crate) struct WebServer {
    layout: WebLayout,
    config: WebConfig,
    /// Serialises start, stop and install.
    control: tokio::sync::Mutex<Option<Supervised>>,
    /// Current state, also updated by the monitor task.
    state: SharedState,
}

impl WebServer {
    pub(crate) fn new(layout: WebLayout, config: WebConfig) -> Self {
        Self {
            layout,
            config,
            control: tokio::sync::Mutex::new(None),
            state: Arc::new(Mutex::new(WebServerState::Stopped)),
        }
    }

    pub(crate) fn layout(&self) -> &WebLayout {
        &self.layout
    }

    pub(crate) fn config(&self) -> &WebConfig {
        &self.config
    }

    pub(crate) fn state(&self) -> WebServerState {
        process::get_state(&self.state)
    }

    fn set_state(&self, state: WebServerState) {
        process::set_state(&self.state, state);
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(
            self.state(),
            WebServerState::Running { .. } | WebServerState::Starting
        )
    }

    /// Installs the pinned Caddy if needed (blocking work on a dedicated
    /// thread). Returns `true` if it was installed by this call.
    pub(crate) async fn install(&self) -> Result<bool, WebError> {
        let _control = self.control.lock().await;
        let layout = self.layout.clone();
        tokio::task::spawn_blocking(move || release::install(&layout))
            .await
            .map_err(|err| {
                tracing::error!(error = %err, "installation task failed");
                WebError::Task
            })?
            .map_err(WebError::from)
    }

    /// Starts Caddy serving `sites`, unless it already runs.
    pub(crate) async fn start(&self, sites: &[Site]) -> Result<(), WebError> {
        let mut control = self.control.lock().await;
        if control
            .as_ref()
            .is_some_and(|running| !running.is_finished())
        {
            return Ok(());
        }
        self.set_state(WebServerState::Starting);
        match self.spawn(sites).await {
            Ok(running) => {
                *control = Some(running);
                Ok(())
            }
            Err(err) if err.is_precondition() => {
                self.set_state(WebServerState::Stopped);
                Err(err)
            }
            Err(err) => {
                let message = devspace_core::error::report(&err);
                tracing::error!(error = %message, "web server failed to start");
                self.set_state(WebServerState::Failed { message });
                Err(err)
            }
        }
    }

    /// Stops Caddy if it runs, waiting for it to exit.
    pub(crate) async fn stop(&self) {
        let mut control = self.control.lock().await;
        if let Some(running) = control.take() {
            running.stop().await;
        }
        self.set_state(WebServerState::Stopped);
    }

    /// Restarts Caddy with a configuration serving `sites`.
    pub(crate) async fn restart(&self, sites: &[Site]) -> Result<(), WebError> {
        self.stop().await;
        self.start(sites).await
    }

    async fn spawn(&self, sites: &[Site]) -> Result<Supervised, WebError> {
        let layout = &self.layout;
        if !release::is_installed(layout) {
            return Err(WebError::NotInstalled);
        }
        let address = self.config.address.ip();
        for (port, setting) in [
            (self.config.http_port, "web.http_port"),
            (self.config.https_port, "web.https_port"),
        ] {
            match TcpListener::bind((address, port)) {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::AddrNotAvailable => {
                    return Err(WebError::AddressUnavailable {
                        address: address.to_string(),
                    });
                }
                Err(_) => {
                    return Err(WebError::PortInUse {
                        address: address.to_string(),
                        port,
                        setting,
                    });
                }
            }
        }

        let home = &layout.caddy_home;
        for dir in [home.as_path(), layout.caddy_storage.as_path()] {
            fs::create_dir_all(dir).map_err(prepare_error(dir))?;
        }
        let config = caddy_config::render(sites, &self.config, &layout.caddy_storage);
        let text = serde_json::to_vec_pretty(&config).map_err(WebError::Serialize)?;
        write_atomic(&layout.caddy_config, &text, FileMode::Private)
            .map_err(prepare_error(&layout.caddy_config))?;
        let log = File::create(&layout.caddy_log).map_err(prepare_error(&layout.caddy_log))?;
        let log_err = log.try_clone().map_err(prepare_error(&layout.caddy_log))?;

        let mut command = std::process::Command::new(&layout.caddy_binary);
        command
            .arg("run")
            .arg("--config")
            .arg(&layout.caddy_config)
            // Keep everything Caddy writes inside the DevSpace directories.
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .current_dir(home)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err);
        // Own session / process group: a Ctrl+C on the daemon's terminal
        // must not reach Caddy, which the daemon stops itself.
        devspace_platform::process::detach(&mut command);
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command.spawn().map_err(|source| WebError::Spawn {
            path: layout.caddy_binary.clone(),
            source,
        })?;
        let pid = child.id().unwrap_or_default();
        if let Err(err) = fs::write(&layout.caddy_pid_file, pid.to_string()) {
            tracing::warn!(error = %err, path = %layout.caddy_pid_file.display(), "failed to write the web server pid file");
        }

        if let Err(err) = self.wait_until_ready(&mut child).await {
            let _ = child.kill().await;
            let _ = fs::remove_file(&layout.caddy_pid_file);
            return Err(err);
        }
        tracing::info!(
            pid,
            sites = sites.len(),
            http_port = self.config.http_port,
            https_port = self.config.https_port,
            "web server started"
        );
        Ok(Supervised::start(
            child,
            ChildInfo {
                name: "web server".to_owned(),
                pid,
                pid_file: layout.caddy_pid_file.clone(),
                log: layout.caddy_log.clone(),
                stop: StopPolicy::DEFAULT,
            },
            Arc::clone(&self.state),
        ))
    }

    /// Whether Caddy listens on the HTTPS port, checked by trying to bind
    /// it (the port was free before Caddy started). Connecting is not
    /// reliable: with the `pf` redirection active on the loopback
    /// interface, direct connections to the redirection targets can stall.
    fn is_listening(&self) -> bool {
        matches!(
            TcpListener::bind((self.config.address.ip(), self.config.https_port)),
            Err(err) if err.kind() == io::ErrorKind::AddrInUse
        )
    }

    async fn wait_until_ready(&self, child: &mut Child) -> Result<(), WebError> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(WebError::Exited {
                    status,
                    log: self.layout.caddy_log.clone(),
                    tail: process::log_tail(&self.layout.caddy_log, LOG_TAIL_LINES),
                });
            }
            if self.is_listening() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(WebError::NotReady {
                    log: self.layout.caddy_log.clone(),
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Whether a request to port 80 of the sites address reaches this web
    /// server, i.e. whether the port forwarding is active.
    pub(crate) async fn probe_forwarding(&self) -> bool {
        let request = format!(
            "HEAD / HTTP/1.0\r\nHost: devspace-probe.{}\r\n\r\n",
            self.config.tld
        );
        let probe = async {
            let mut stream = tokio::net::TcpStream::connect((self.config.address.ip(), 80)).await?;
            stream.write_all(request.as_bytes()).await?;
            let mut buffer = vec![0u8; 4096];
            let mut read = 0;
            while read < buffer.len() {
                let n = stream.read(&mut buffer[read..]).await?;
                if n == 0 {
                    break;
                }
                read += n;
            }
            io::Result::Ok(String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase())
        };
        let header = format!("\r\n{}: 1", PROBE_HEADER.to_ascii_lowercase());
        matches!(
            tokio::time::timeout(PROBE_TIMEOUT, probe).await,
            Ok(Ok(response)) if response.contains(&header)
        )
    }
}

/// Stops a Caddy left running by a previous daemon (crash or kill).
pub(crate) fn stop_orphan(layout: &WebLayout) {
    process::stop_orphan(
        &layout.caddy_pid_file,
        std::slice::from_ref(&layout.caddy_binary),
        "web server",
        StopPolicy::DEFAULT,
    );
}

fn prepare_error(path: &Path) -> impl FnOnce(io::Error) -> WebError + '_ {
    |source| WebError::Prepare {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_requires_installation() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = WebLayout::new(&devspace_core::AppPaths::from_root(tmp.path()));
        let web = WebServer::new(layout, WebConfig::default());
        let err = web.start(&[]).await.unwrap_err();
        assert!(matches!(err, WebError::NotInstalled));
        assert!(err.is_precondition());
        assert_eq!(web.state(), WebServerState::Stopped);
        web.stop().await;
        assert_eq!(web.state(), WebServerState::Stopped);
    }
}
