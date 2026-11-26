//! Supervision of the service instances (MySQL, PostgreSQL) and the
//! service requests.
//!
//! [`devspace_core::service::plan`] decides which instances run, from the
//! manifests of the registered projects, the installed versions and the
//! choices of the user stored in the database. [`Services::sync`] then
//! stops what must not run (or runs an outdated version) and starts what
//! is missing: the data directory is initialised at the first start, then
//! the server runs in the foreground as the current user, on the sites
//! address. The plan is applied at daemon startup and after every change
//! of the projects, of the installed versions and of the choices.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use devspace_core::config::ServicesConfig;
use devspace_core::error::report;
use devspace_core::project::manifest::Manifest;
use devspace_core::runtime::{InstalledRuntime, RuntimeName, Version};
use devspace_core::service::process::{Instance, open_log};
use devspace_core::service::{
    self, InstanceKey, Plan, PlannedInstance, Requirement, Series, ServiceLayout, ServiceMode,
    ServiceName,
};
use devspace_ipc::{
    ErrorCode, ErrorResponse, Response, ServiceInstanceInfo, ServiceList, ServiceMode as ModeInfo,
    WebServerState,
};
use devspace_platform::paths::OsFamily;

use crate::handlers::{self, DaemonState};
use crate::process::{self, ChildInfo, POLL_INTERVAL, SharedState, StopPolicy, Supervised};

/// Time given to a server to listen on its port (the first start of MySQL
/// creates its system tables first).
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// Time given to a server to stop (writing its buffers to disk).
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// Number of log lines quoted when a server fails.
const LOG_TAIL_LINES: usize = 15;

/// A running server.
#[derive(Debug)]
struct Running {
    version: Version,
    supervised: Supervised,
}

/// Supervisor of the service instances.
#[derive(Debug)]
pub(crate) struct Services {
    layout: ServiceLayout,
    config: ServicesConfig,
    address: Ipv4Addr,
    /// Serialises the changes; one entry per running server.
    running: tokio::sync::Mutex<BTreeMap<InstanceKey, Running>>,
    /// States for status requests, without waiting for a change in
    /// progress.
    states: Mutex<BTreeMap<InstanceKey, SharedState>>,
}

impl Services {
    /// Stops the servers left by a previous daemon. Blocking.
    pub(crate) fn new(
        layout: ServiceLayout,
        config: ServicesConfig,
        address: Ipv4Addr,
        installed: &[InstalledRuntime],
    ) -> Self {
        stop_orphans(&layout, installed);
        Self {
            layout,
            config,
            address,
            running: tokio::sync::Mutex::new(BTreeMap::new()),
            states: Mutex::new(BTreeMap::new()),
        }
    }

    /// Loopback address and port of `service`.
    pub(crate) fn endpoint(&self, service: ServiceName) -> (Ipv4Addr, u16) {
        (self.address, service.port(&self.config))
    }

    fn state(&self, key: InstanceKey) -> SharedState {
        Arc::clone(
            self.states
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(key)
                .or_insert_with(|| Arc::new(Mutex::new(WebServerState::Stopped))),
        )
    }

    fn current_state(&self, key: InstanceKey) -> WebServerState {
        self.states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .map_or(WebServerState::Stopped, |state| process::get_state(state))
    }

    /// Runs exactly the instances `plan` wants, with the version it chose.
    /// Start failures are logged and kept in the state of the instance.
    pub(crate) async fn sync(&self, plan: &Plan) {
        let mut running = self.running.lock().await;
        let wanted: BTreeMap<InstanceKey, &PlannedInstance> = plan
            .running()
            .map(|instance| (instance.key, instance))
            .collect();
        let outdated: Vec<InstanceKey> = running
            .iter()
            .filter(|(key, server)| {
                server.supervised.is_finished()
                    || wanted
                        .get(key)
                        .and_then(|i| i.runtime.as_ref())
                        .map(|r| r.version)
                        != Some(server.version)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in outdated {
            if let Some(server) = running.remove(&key) {
                let finished = server.supervised.is_finished();
                server.supervised.stop().await;
                if !finished {
                    // A server that exited by itself keeps its failure.
                    process::set_state(&self.state(key), WebServerState::Stopped);
                }
            }
        }
        for (key, instance) in wanted {
            if running.contains_key(&key) {
                continue;
            }
            let Some(runtime) = &instance.runtime else {
                continue;
            };
            // A failed instance is retried: every sync follows a change
            // (projects, installations, choices of the user).
            let state = self.state(key);
            process::set_state(&state, WebServerState::Starting);
            match self.spawn(key, runtime, &state).await {
                Ok(supervised) => {
                    running.insert(
                        key,
                        Running {
                            version: runtime.version,
                            supervised,
                        },
                    );
                }
                Err(message) => {
                    tracing::error!(instance = %key, error = %message, "service failed to start");
                    process::set_state(&state, WebServerState::Failed { message });
                }
            }
        }
    }

    /// Stops every server running a version of `name` matching `version`
    /// (before its files are removed).
    pub(crate) async fn stop_version(&self, name: RuntimeName, version: Version) {
        let Some(service) = ServiceName::of_package(name) else {
            return;
        };
        let mut running = self.running.lock().await;
        let key = InstanceKey::of(service, version);
        if running
            .get(&key)
            .is_some_and(|server| server.version == version)
            && let Some(server) = running.remove(&key)
        {
            server.supervised.stop().await;
            process::set_state(&self.state(key), WebServerState::Stopped);
        }
    }

    /// Stops every server.
    pub(crate) async fn stop_all(&self) {
        let mut running = self.running.lock().await;
        for (key, server) in std::mem::take(&mut *running) {
            server.supervised.stop().await;
            process::set_state(&self.state(key), WebServerState::Stopped);
        }
    }

    async fn spawn(
        &self,
        key: InstanceKey,
        runtime: &InstalledRuntime,
        state: &SharedState,
    ) -> Result<Supervised, String> {
        let port = key.service.port(&self.config);
        check_port(self.address, port, key.service)?;

        let layout = self.layout.clone();
        let owned_runtime = runtime.clone();
        let address = self.address;
        let log_file = self.layout.log_file(key);
        // The first start creates the data directory (seconds).
        tokio::task::spawn_blocking(move || {
            Instance {
                key,
                runtime: &owned_runtime,
                address,
                port,
                layout: &layout,
            }
            .initialize()
        })
        .await
        .map_err(|err| format!("the initialisation task failed: {err}"))?
        .map_err(|err| {
            format!(
                "{}; last lines of `{}`:\n{}",
                report(&err),
                log_file.display(),
                process::log_tail(&log_file, LOG_TAIL_LINES)
            )
        })?;

        let instance = Instance {
            key,
            runtime,
            address: self.address,
            port,
            layout: &self.layout,
        };
        let invocation = instance
            .server(OsFamily::current())
            .map_err(|err| report(&err))?;
        let log = open_log(&log_file)
            .map_err(|err| format!("cannot open `{}`: {err}", log_file.display()))?;
        let log_err = log
            .try_clone()
            .map_err(|err| format!("cannot open `{}`: {err}", log_file.display()))?;
        let mut command = std::process::Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .current_dir(&self.layout.data_root)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err);
        devspace_platform::process::detach(&mut command);
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| format!("cannot start `{}`: {err}", invocation.program.display()))?;
        let pid = child.id().unwrap_or_default();
        let pid_file = self.layout.pid_file(key);
        if let Err(err) = std::fs::write(&pid_file, pid.to_string()) {
            tracing::warn!(error = %err, path = %pid_file.display(), "failed to write a service pid file");
        }

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let _ = std::fs::remove_file(&pid_file);
                return Err(format!(
                    "{key} exited during startup ({status}); last lines of `{}`:\n{}",
                    log_file.display(),
                    process::log_tail(&log_file, LOG_TAIL_LINES)
                ));
            }
            if is_listening(self.address, port) {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill().await;
                let _ = std::fs::remove_file(&pid_file);
                return Err(format!(
                    "{key} did not listen on {}:{port} within {}s; see `{}`",
                    self.address,
                    START_TIMEOUT.as_secs(),
                    log_file.display()
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        tracing::info!(pid, instance = %key, version = %runtime.version, %port, "service started");
        Ok(Supervised::start(
            child,
            ChildInfo {
                name: format!("{key} server"),
                pid,
                pid_file,
                log: log_file,
                stop: stop_policy(key.service),
            },
            Arc::clone(state),
        ))
    }
}

fn stop_policy(service: ServiceName) -> StopPolicy {
    StopPolicy {
        request: service.stop_request(),
        timeout: STOP_TIMEOUT,
    }
}

/// Fails if the port of `service` cannot be used on `address`.
fn check_port(address: Ipv4Addr, port: u16, service: ServiceName) -> Result<(), String> {
    match TcpListener::bind((address, port)) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AddrNotAvailable => Err(format!(
            "the sites address {address} is not configured on the loopback interface; run \
             `devspace system setup`"
        )),
        Err(err) => Err(format!(
            "port {port} of {address} is not available ({err}); another server may use it; \
             stop it or change `services.{service}_port` in the \
             configuration"
        )),
    }
}

/// Whether a server listens on `address:port`, checked by a failing bind
/// (the port was free before the server started), as for the web server.
fn is_listening(address: Ipv4Addr, port: u16) -> bool {
    matches!(
        TcpListener::bind((address, port)),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse
    )
}

/// Stops the servers left by a previous daemon. Blocking.
fn stop_orphans(layout: &ServiceLayout, installed: &[InstalledRuntime]) {
    let Ok(entries) = std::fs::read_dir(&layout.run_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(key) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".pid"))
            .and_then(parse_file_stem)
        else {
            continue;
        };
        let executables: Vec<_> = installed
            .iter()
            .filter(|runtime| {
                runtime.name == key.service.package() && key.series.contains(runtime.version)
            })
            .filter_map(|runtime| runtime.program(key.service.server_program()))
            .collect();
        process::stop_orphan(
            &path,
            &executables,
            &key.to_string(),
            stop_policy(key.service),
        );
    }
}

/// Inverse of [`InstanceKey::file_stem`].
fn parse_file_stem(stem: &str) -> Option<InstanceKey> {
    let (name, series) = stem.split_once('-')?;
    let service = ServiceName::parse(name).ok()?;
    Some(InstanceKey {
        service,
        series: Series::parse(service, series).ok()?,
    })
}

/// The plan for the current projects, installations and choices
/// (blocking).
pub(crate) fn current_plan(state: &DaemonState) -> Result<Plan, Response> {
    let projects = state.projects()?;
    let modes = state.service_modes()?;
    let mut requirements = Vec::new();
    for project in &projects {
        let manifest = match Manifest::load(&project.path) {
            Ok(manifest) => manifest.unwrap_or_default(),
            Err(err) => {
                tracing::warn!(project = %project.name.as_str(), error = %report(&err), "ignoring the services of a project with an invalid manifest");
                continue;
            }
        };
        for service in ServiceName::ALL {
            if let Some(versions) = manifest.services.get(service.package()) {
                requirements.push(Requirement {
                    project: project.name.clone(),
                    service,
                    versions,
                });
            }
        }
    }
    Ok(service::plan(
        &requirements,
        &state.runtimes.installed(),
        &modes,
    ))
}

/// Applies the current plan.
pub(crate) async fn apply(state: &Arc<DaemonState>) -> Result<Plan, Response> {
    let plan = {
        let state = Arc::clone(state);
        tokio::task::spawn_blocking(move || current_plan(&state))
            .await
            .unwrap_or_else(|err| {
                tracing::error!(error = %err, "service plan task failed");
                Err(internal("the request handler failed; see the daemon logs"))
            })?
    };
    state.services.sync(&plan).await;
    Ok(plan)
}

/// Applies the current plan without waiting (after a change).
pub(crate) fn apply_in_background(state: &Arc<DaemonState>) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        if let Err(response) = apply(&state).await {
            tracing::error!(?response, "failed to apply the service plan");
        }
    });
}

/// `service_list`.
pub(crate) async fn list(state: &Arc<DaemonState>) -> Response {
    handlers::blocking(state, |state| match current_plan(state) {
        Ok(plan) => Response::Services(service_list(state, &plan)),
        Err(response) => response,
    })
    .await
}

/// `service_start` and `service_stop`: records the choice of the user,
/// applies the plan and answers with the state of the instances.
pub(crate) async fn set(
    state: &Arc<DaemonState>,
    name: &str,
    series: Option<&str>,
    start: bool,
) -> Response {
    let service = match ServiceName::parse(name) {
        Ok(service) => service,
        Err(message) => return invalid(message),
    };
    let series = match series.map(|text| Series::parse(service, text)).transpose() {
        Ok(series) => series,
        Err(message) => return invalid(message),
    };
    let recorded = handlers::blocking(state, move |state| {
        match record_choice(state, service, series, start) {
            Ok(()) => Response::Pong,
            Err(response) => response,
        }
    })
    .await;
    if matches!(recorded, Response::Error(_)) {
        return recorded;
    }
    match apply(state).await {
        Ok(plan) => {
            let state = Arc::clone(state);
            tokio::task::spawn_blocking(move || Response::Services(service_list(&state, &plan)))
                .await
                .unwrap_or_else(|_| internal("the request handler failed; see the daemon logs"))
        }
        Err(response) => response,
    }
}

/// Stores the choice for the designated instances (blocking). A choice
/// equal to what the projects decide is stored as "automatic", so that
/// the projects keep deciding later.
fn record_choice(
    state: &DaemonState,
    service: ServiceName,
    series: Option<Series>,
    start: bool,
) -> Result<(), Response> {
    let plan = current_plan(state)?;
    let of_service = || {
        plan.instances
            .iter()
            .filter(move |instance| instance.key.service == service)
    };
    let keys: Vec<InstanceKey> = match (series, start) {
        (Some(series), _) => vec![InstanceKey { service, series }],
        (None, true) => {
            // The newest installed series.
            let newest = of_service()
                .filter(|instance| instance.runtime.is_some())
                .map(|instance| instance.key)
                .max()
                .ok_or_else(|| {
                    error(
                        ErrorCode::NotFound,
                        format!("no {service} version is installed; run `devspace service install {service}`"),
                    )
                })?;
            vec![newest]
        }
        (None, false) => of_service()
            .filter(|instance| {
                instance.run
                    || instance.mode == Some(ServiceMode::Started)
                    || !instance.required_by.is_empty()
            })
            .map(|instance| instance.key)
            .collect(),
    };
    if start
        && let Some(key) = keys.first()
        && key.newest_installed(&state.runtimes.installed()).is_none()
    {
        return Err(error(
            ErrorCode::NotFound,
            format!(
                "no {key} version is installed; run `devspace service install {} {}`",
                key.service, key.series
            ),
        ));
    }
    for key in keys {
        let required = plan
            .instances
            .iter()
            .any(|instance| instance.key == key && !instance.required_by.is_empty());
        let mode = match (start, required) {
            (true, true) | (false, false) => None,
            (true, false) => Some(ServiceMode::Started),
            (false, true) => Some(ServiceMode::Stopped),
        };
        state.set_service_mode(key, mode)?;
        if start {
            // Another series of the service started explicitly would keep
            // the port.
            for other in plan.instances.iter().filter(|instance| {
                instance.key.service == service
                    && instance.key != key
                    && instance.mode == Some(ServiceMode::Started)
            }) {
                state.set_service_mode(other.key, None)?;
            }
        }
    }
    Ok(())
}

fn service_list(state: &DaemonState, plan: &Plan) -> ServiceList {
    let services = &state.services;
    ServiceList {
        address: services.address.to_string(),
        instances: plan
            .instances
            .iter()
            .map(|instance| ServiceInstanceInfo {
                name: instance.key.service.to_string(),
                series: instance.key.series.to_string(),
                version: instance
                    .runtime
                    .as_ref()
                    .map(|runtime| runtime.version.to_string()),
                port: instance.key.service.port(&services.config),
                user: instance.key.service.user().to_owned(),
                mode: match instance.mode {
                    None => ModeInfo::Automatic,
                    Some(ServiceMode::Started) => ModeInfo::Started,
                    Some(ServiceMode::Stopped) => ModeInfo::Stopped,
                },
                required_by: instance
                    .required_by
                    .iter()
                    .map(|name| name.as_str().to_owned())
                    .collect(),
                state: services.current_state(instance.key),
                blocked: instance.blocked.clone(),
                data_dir: services.layout.data_dir(instance.key),
                log_file: services.layout.log_file(instance.key),
            })
            .collect(),
        missing: plan.missing.clone(),
    }
}

fn error(code: ErrorCode, message: String) -> Response {
    Response::Error(ErrorResponse { code, message })
}

fn invalid(message: String) -> Response {
    error(ErrorCode::InvalidRequest, message)
}

fn internal(message: &str) -> Response {
    error(ErrorCode::Internal, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_file_names_round_trip() {
        for stem in ["postgresql-18", "mysql-8.4"] {
            assert_eq!(parse_file_stem(stem).unwrap().file_stem(), stem);
        }
        for bad in ["postgresql", "redis-7", "mysql-8", "postgresql-18.1"] {
            assert!(parse_file_stem(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn unconfigured_address_is_explained() {
        // 127.0.0.254 is not configured on a usual loopback interface
        // (Linux accepts the whole 127.0.0.0/8 range).
        if let Err(message) =
            check_port(Ipv4Addr::new(127, 0, 0, 254), 5432, ServiceName::Postgresql)
        {
            assert!(
                message.contains("system setup") || message.contains("not available"),
                "{message}"
            );
        }
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = busy.local_addr().unwrap().port();
        let message = check_port(Ipv4Addr::LOCALHOST, port, ServiceName::Mysql).unwrap_err();
        assert!(message.contains("services.mysql_port"), "{message}");
        assert!(is_listening(Ipv4Addr::LOCALHOST, port));
    }

    #[tokio::test]
    async fn uninstalled_instances_are_not_started() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = ServiceLayout::new(&devspace_core::AppPaths::from_root(tmp.path()));
        let services = Services::new(layout, ServicesConfig::default(), Ipv4Addr::LOCALHOST, &[]);
        let plan = service::plan(
            &[],
            &[],
            &[(
                InstanceKey {
                    service: ServiceName::Postgresql,
                    series: Series::parse(ServiceName::Postgresql, "18").unwrap(),
                },
                ServiceMode::Started,
            )],
        );
        services.sync(&plan).await;
        assert!(services.running.lock().await.is_empty());
        services.stop_all().await;
    }
}
