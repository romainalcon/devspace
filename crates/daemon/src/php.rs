//! Supervision of the PHP FastCGI process managers.
//!
//! One PHP-FPM master runs per PHP version used by a site, as the current
//! user, with the configuration of [`devspace_core::runtime::php`]. The
//! set of masters follows the sites: [`PhpPools::sync`] starts the missing
//! ones and stops those no site uses any more, before the web server is
//! (re)started.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devspace_core::runtime::php::{FpmPaths, PHP_INI};
use devspace_core::runtime::{self, InstalledRuntime, RuntimeLayout, RuntimeName, Version};
use devspace_ipc::{PhpPoolInfo, WebServerState};
use devspace_platform::fs::{FileMode, write_atomic};

use crate::process::{self, ChildInfo, POLL_INTERVAL, SharedState, StopPolicy, Supervised};

/// Time given to PHP-FPM to create its socket.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Number of log lines quoted when PHP-FPM fails to start.
const LOG_TAIL_LINES: usize = 10;

/// A PHP-FPM master.
#[derive(Debug)]
struct Pool {
    state: SharedState,
    running: Option<Supervised>,
}

/// Supervisor of the PHP-FPM masters.
#[derive(Debug)]
pub(crate) struct PhpPools {
    layout: RuntimeLayout,
    /// Serialises the changes; one entry per PHP version in use.
    pools: tokio::sync::Mutex<BTreeMap<Version, Pool>>,
    /// States for status requests, without waiting for a change in
    /// progress.
    states: Mutex<BTreeMap<Version, SharedState>>,
}

impl PhpPools {
    pub(crate) fn new(layout: RuntimeLayout) -> Self {
        Self {
            layout,
            pools: tokio::sync::Mutex::new(BTreeMap::new()),
            states: Mutex::new(BTreeMap::new()),
        }
    }

    /// State of every master, sorted by version.
    pub(crate) fn status(&self) -> Vec<PhpPoolInfo> {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(version, state)| PhpPoolInfo {
                version: version.to_string(),
                state: process::get_state(state),
            })
            .collect()
    }

    /// Runs exactly the masters of `needed`: stops the others, starts the
    /// missing or failed ones. Start failures are logged and kept in the
    /// state of the pool (the site then answers 502); they never prevent
    /// the other pools or the web server from running.
    pub(crate) async fn sync(&self, needed: &BTreeSet<Version>, installed: &[InstalledRuntime]) {
        let mut pools = self.pools.lock().await;
        let unused: Vec<Version> = pools
            .keys()
            .filter(|version| !needed.contains(version))
            .copied()
            .collect();
        for version in unused {
            if let Some(pool) = pools.remove(&version)
                && let Some(running) = pool.running
            {
                running.stop().await;
            }
        }
        for &version in needed {
            let pool = pools.entry(version).or_insert_with(|| Pool {
                state: Arc::new(Mutex::new(WebServerState::Stopped)),
                running: None,
            });
            if pool
                .running
                .as_ref()
                .is_some_and(|running| !running.is_finished())
            {
                continue;
            }
            process::set_state(&pool.state, WebServerState::Starting);
            match self.spawn(version, installed, &pool.state).await {
                Ok(running) => pool.running = Some(running),
                Err(message) => {
                    tracing::error!(%version, error = %message, "PHP-FPM failed to start");
                    pool.running = None;
                    process::set_state(&pool.state, WebServerState::Failed { message });
                }
            }
        }
        self.publish(&pools);
    }

    /// Stops every master.
    pub(crate) async fn stop_all(&self) {
        self.sync(&BTreeSet::new(), &[]).await;
    }

    /// Stops the master of `version`, if it runs (before removing it).
    pub(crate) async fn stop(&self, version: Version) {
        let mut pools = self.pools.lock().await;
        if let Some(pool) = pools.remove(&version)
            && let Some(running) = pool.running
        {
            running.stop().await;
        }
        self.publish(&pools);
    }

    fn publish(&self, pools: &BTreeMap<Version, Pool>) {
        *self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pools
            .iter()
            .map(|(version, pool)| (*version, Arc::clone(&pool.state)))
            .collect();
    }

    async fn spawn(
        &self,
        version: Version,
        installed: &[InstalledRuntime],
        state: &SharedState,
    ) -> Result<Supervised, String> {
        let program = installed
            .iter()
            .find(|runtime| runtime.name == RuntimeName::Php && runtime.version == version)
            .and_then(|runtime| runtime.program("php-fpm"))
            .ok_or_else(|| format!("PHP {version} is not installed with php-fpm"))?;
        let fpm = self.layout.fpm(version);
        prepare(&fpm).map_err(|err| devspace_core::error::report(&*err))?;

        let log = File::create(&fpm.log_file)
            .map_err(|err| format!("cannot create `{}`: {err}", fpm.log_file.display()))?;
        let log_err = log
            .try_clone()
            .map_err(|err| format!("cannot open `{}`: {err}", fpm.log_file.display()))?;
        let mut command = std::process::Command::new(&program);
        command
            .args(fpm.arguments())
            .current_dir(&fpm.ini_dir)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err);
        devspace_platform::process::detach(&mut command);
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| format!("cannot start `{}`: {err}", program.display()))?;
        let pid = child.id().unwrap_or_default();
        if let Err(err) = fs::write(&fpm.pid_file, pid.to_string()) {
            tracing::warn!(error = %err, path = %fpm.pid_file.display(), "failed to write the PHP-FPM pid file");
        }

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let _ = fs::remove_file(&fpm.pid_file);
                return Err(format!(
                    "PHP-FPM {version} exited during startup ({status}); last lines of `{}`:\n{}",
                    fpm.log_file.display(),
                    process::log_tail(&fpm.log_file, LOG_TAIL_LINES)
                ));
            }
            if fpm.socket.exists() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill().await;
                let _ = fs::remove_file(&fpm.pid_file);
                return Err(format!(
                    "PHP-FPM {version} did not create its socket within {}s; see `{}`",
                    START_TIMEOUT.as_secs(),
                    fpm.log_file.display()
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        tracing::info!(pid, %version, socket = %fpm.socket.display(), "PHP-FPM started");
        Ok(Supervised::start(
            child,
            ChildInfo {
                name: format!("PHP-FPM {version}"),
                pid,
                pid_file: fpm.pid_file.clone(),
                log: fpm.log_file.clone(),
                stop: StopPolicy::DEFAULT,
            },
            Arc::clone(state),
        ))
    }
}

/// Writes the configuration of `fpm` and removes a socket left by a
/// previous master.
fn prepare(fpm: &FpmPaths) -> Result<(), Box<devspace_core::Error>> {
    let file_error = |path: &PathBuf| {
        let path = path.clone();
        move |source: io::Error| Box::new(devspace_core::Error::InstallFile { path, source })
    };
    let config = fpm.render_config().map_err(Box::new)?;
    for dir in [&fpm.ini_dir, &parent(&fpm.socket), &parent(&fpm.log_file)] {
        fs::create_dir_all(dir).map_err(file_error(dir))?;
    }
    write_ini(fpm).map_err(Box::new)?;
    write_atomic(&fpm.config_file, config.as_bytes(), FileMode::Private)
        .map_err(file_error(&fpm.config_file))?;
    match fs::remove_file(&fpm.socket) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(file_error(&fpm.socket)(err)),
    }
}

/// Writes the `php.ini` of `fpm` (also used by `devspace exec`).
pub(crate) fn write_ini(fpm: &FpmPaths) -> Result<(), devspace_core::Error> {
    fs::create_dir_all(&fpm.ini_dir)
        .and_then(|()| write_atomic(&fpm.ini_file, PHP_INI.as_bytes(), FileMode::Readable))
        .map_err(|source| devspace_core::Error::InstallFile {
            path: fpm.ini_file.clone(),
            source,
        })
}

fn parent(path: &std::path::Path) -> PathBuf {
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// Stops the PHP-FPM masters left by a previous daemon. Blocking.
pub(crate) fn stop_orphans(layout: &RuntimeLayout) {
    let Ok(entries) = fs::read_dir(&layout.php_run_dir) else {
        return;
    };
    let installed = runtime::list_installed(layout);
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(version) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".pid"))
            .and_then(|version| Version::parse(version).ok())
        else {
            continue;
        };
        let executable = installed
            .iter()
            .find(|runtime| runtime.name == RuntimeName::Php && runtime.version == version)
            .and_then(|runtime| runtime.program("php-fpm"));
        process::stop_orphan(&path, executable.as_slice(), "PHP-FPM", StopPolicy::DEFAULT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_php_is_reported_as_a_failed_pool() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(&devspace_core::AppPaths::from_root(tmp.path()));
        let pools = PhpPools::new(layout);
        let version = Version::parse("8.4.23").unwrap();
        pools.sync(&BTreeSet::from([version]), &[]).await;
        let status = pools.status();
        assert_eq!(status.len(), 1);
        assert!(matches!(
            &status[0].state,
            WebServerState::Failed { message } if message.contains("not installed")
        ));
        pools.stop_all().await;
        assert!(pools.status().is_empty());
    }
}
