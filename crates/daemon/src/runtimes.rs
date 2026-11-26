//! Runtime requests: list, install, remove, and the environment of a
//! directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use devspace_core::error::report;
use devspace_core::project::manifest::Manifest;
use devspace_core::runtime::index::{Index, platform_key};
use devspace_core::runtime::{
    self, InstalledRuntime, RuntimeLayout, RuntimeName, Selected, Selection, Version, VersionReq,
    install,
};
use devspace_core::service::ServiceName;
use devspace_ipc::{
    ErrorCode, ErrorResponse, Response, RuntimeEnv, RuntimeEnvEntry, RuntimeInfo, RuntimeList,
    RuntimeVersionInfo,
};
use devspace_platform::target::Target;

use crate::handlers::{self, DaemonState};
use crate::php::{self, PhpPools};
use crate::services;

/// Runtime state of the daemon.
#[derive(Debug)]
pub(crate) struct Runtimes {
    pub(crate) layout: RuntimeLayout,
    /// The embedded package index, or why it cannot be used.
    index: Result<Arc<Index>, String>,
    /// Serialises installations and removals.
    changes: tokio::sync::Mutex<()>,
    pub(crate) php: PhpPools,
}

impl Runtimes {
    /// Loads the embedded index and cleans what interrupted installations
    /// left. Blocking.
    pub(crate) fn new(layout: RuntimeLayout) -> Self {
        install::clean_staging(&layout);
        php::stop_orphans(&layout);
        let index = match Index::embedded() {
            Ok(index) => {
                tracing::info!(serial = index.serial, "package index loaded");
                Ok(Arc::new(index))
            }
            Err(err) => {
                let message = report(&err);
                tracing::error!(error = %message, "package index unusable");
                Err(message)
            }
        };
        Self {
            php: PhpPools::new(layout.clone()),
            layout,
            index,
            changes: tokio::sync::Mutex::new(()),
        }
    }

    fn index(&self) -> Result<Arc<Index>, Response> {
        self.index.clone().map_err(|message| {
            error(
                ErrorCode::Internal,
                format!("the package index of this build is unusable: {message}"),
            )
        })
    }

    pub(crate) fn installed(&self) -> Vec<InstalledRuntime> {
        runtime::list_installed(&self.layout)
    }
}

fn error(code: ErrorCode, message: String) -> Response {
    Response::Error(ErrorResponse { code, message })
}

fn invalid(message: String) -> Response {
    error(ErrorCode::InvalidRequest, message)
}

fn target() -> Result<Target, Response> {
    Target::current().ok_or_else(|| {
        error(
            ErrorCode::Unavailable,
            "runtimes are not available for this processor architecture".to_owned(),
        )
    })
}

/// `runtime_list`.
pub(crate) async fn list(state: &Arc<DaemonState>) -> Response {
    handlers::blocking(state, |state| {
        let runtimes = &state.runtimes;
        let (index, target) = match runtimes.index().and_then(|index| Ok((index, target()?))) {
            Ok(pair) => pair,
            Err(response) => return response,
        };
        let mut versions: BTreeMap<(RuntimeName, Version), RuntimeVersionInfo> = BTreeMap::new();
        for name in RuntimeName::ALL {
            for version in index.versions(name, target) {
                versions.insert(
                    (name, version),
                    RuntimeVersionInfo {
                        name: name.to_string(),
                        version: version.to_string(),
                        available: true,
                        path: None,
                    },
                );
            }
        }
        for installed in runtimes.installed() {
            versions
                .entry((installed.name, installed.version))
                .or_insert_with(|| RuntimeVersionInfo {
                    name: installed.name.to_string(),
                    version: installed.version.to_string(),
                    available: false,
                    path: None,
                })
                .path = Some(installed.dir);
        }
        Response::Runtimes(RuntimeList {
            index_serial: index.serial,
            platform: platform_key(target).to_owned(),
            versions: versions.into_values().collect(),
        })
    })
    .await
}

/// `runtime_install`.
pub(crate) async fn install(
    state: &Arc<DaemonState>,
    name: &str,
    version: Option<&str>,
) -> Response {
    let request = (|| {
        let name = RuntimeName::parse(name).map_err(invalid)?;
        let req = version
            .map(VersionReq::parse)
            .transpose()
            .map_err(invalid)?;
        let index = state.runtimes.index()?;
        let version = install::resolve(&index, name, req, target()?)
            .map_err(|err| handlers::error_response(&err))?;
        Ok((name, version, index))
    })();
    let (name, version, index) = match request {
        Ok(request) => request,
        Err(response) => return response,
    };
    let response = {
        let _changes = state.runtimes.changes.lock().await;
        handlers::blocking(state, move |state| {
            match install::install(&state.runtimes.layout, &index, name, version) {
                Ok((runtime, changed)) => {
                    Response::RuntimeInstalled(runtime_info(&runtime, changed))
                }
                Err(err) => handlers::error_response(&err),
            }
        })
        .await
    };
    if matches!(
        response,
        Response::RuntimeInstalled(RuntimeInfo { changed: true, .. })
    ) {
        if name.is_service() {
            // A required instance may now run, or run a newer version.
            services::apply_in_background(state);
        } else {
            // A PHP site may now be served.
            handlers::reload_in_background(state);
        }
    }
    response
}

/// `runtime_remove`.
pub(crate) async fn remove(state: &Arc<DaemonState>, name: &str, version: &str) -> Response {
    let (name, version) = match RuntimeName::parse(name)
        .and_then(|name| Version::parse(version).map(|version| (name, version)))
    {
        Ok(pair) => pair,
        Err(message) => return invalid(message),
    };
    let response = {
        let _changes = state.runtimes.changes.lock().await;
        // Its files are about to disappear.
        if name == RuntimeName::Php {
            state.runtimes.php.stop(version).await;
        }
        state.services.stop_version(name, version).await;
        handlers::blocking(state, move |state| {
            match install::remove(&state.runtimes.layout, name, version) {
                Ok(runtime) => Response::RuntimeRemoved(runtime_info(&runtime, true)),
                Err(err) => handlers::error_response(&err),
            }
        })
        .await
    };
    // The sites and instances of that version move to another one or
    // become unavailable.
    if name.is_service() {
        services::apply_in_background(state);
    } else {
        handlers::reload_in_background(state);
    }
    response
}

/// `runtime_env`.
pub(crate) async fn env(state: &Arc<DaemonState>, directory: PathBuf) -> Response {
    handlers::blocking(state, move |state| {
        environment(state, &directory).unwrap_or_else(|response| response)
    })
    .await
}

fn environment(state: &DaemonState, directory: &Path) -> Result<Response, Response> {
    if !directory.is_absolute() {
        return Err(invalid(format!(
            "directory `{}` must be absolute",
            directory.display()
        )));
    }
    let directory = devspace_platform::fs::canonicalize(directory).map_err(|err| {
        invalid(format!(
            "cannot access directory `{}`: {err}",
            directory.display()
        ))
    })?;
    // The innermost registered project containing the directory.
    let project = state
        .projects()?
        .into_iter()
        .filter(|project| directory.starts_with(&project.path))
        .max_by_key(|project| project.path.components().count());
    let manifest = match &project {
        Some(project) => Manifest::load(&project.path)
            .map_err(|err| handlers::error_response(&err))?
            .unwrap_or_default(),
        None => Manifest::default(),
    };

    let installed = state.runtimes.installed();
    let mut runtimes = Vec::new();
    let mut missing = Vec::new();
    for name in RuntimeName::ALL {
        let selection = manifest
            .requirement(name)
            .map_or(Selection::Newest, Selection::Manifest);
        match runtime::select(&installed, name, selection) {
            Selected::Installed(runtime) => {
                let mut variables = Vec::new();
                if name == RuntimeName::Php {
                    let fpm = state.runtimes.layout.fpm(runtime.version);
                    php::write_ini(&fpm).map_err(|err| handlers::error_response(&err))?;
                    variables.push(("PHPRC".to_owned(), fpm.ini_dir.display().to_string()));
                }
                if let Some(service) = ServiceName::of_package(name) {
                    let (address, port) = state.services.endpoint(service);
                    variables.extend(service.client_variables(address, port));
                }
                runtimes.push(RuntimeEnvEntry {
                    name: name.to_string(),
                    version: runtime.version.to_string(),
                    bin_dirs: runtime.bin_dirs,
                    variables,
                });
            }
            Selected::Missing => {
                if let Selection::Manifest(req) = selection {
                    let command = if name.is_service() {
                        "service"
                    } else {
                        "runtime"
                    };
                    missing.push(format!(
                        "{name} {req} (required by devspace.toml) is not installed; run \
                         `devspace {command} install {name} {req}`"
                    ));
                }
            }
        }
    }
    Ok(Response::RuntimeEnv(RuntimeEnv {
        project: project.map(|project| project.name.as_str().to_owned()),
        runtimes,
        missing,
    }))
}

fn runtime_info(runtime: &InstalledRuntime, changed: bool) -> RuntimeInfo {
    RuntimeInfo {
        name: runtime.name.to_string(),
        version: runtime.version.to_string(),
        path: runtime.dir.clone(),
        changed,
    }
}
