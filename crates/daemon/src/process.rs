//! Supervision of the child processes of the daemon (Caddy, PHP-FPM).
//!
//! A monitor task owns each child: it records an unexpected exit as a
//! failure in the shared state, and performs graceful stops (`SIGTERM`,
//! then a kill after [`STOP_TIMEOUT`]). The pid of every child is kept in
//! a file of the runtime directory, so that a daemon restarted after a
//! crash can stop what the previous one left (only if that pid still runs
//! the expected executable).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use devspace_ipc::WebServerState;
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Time given to a child to stop gracefully before it is killed.
pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between checks while waiting for a process.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// State of a supervised process, shared with its monitor task.
pub(crate) type SharedState = Arc<Mutex<WebServerState>>;

/// Replaces the state.
pub(crate) fn set_state(state: &Mutex<WebServerState>, value: WebServerState) {
    *state.lock().unwrap_or_else(PoisonError::into_inner) = value;
}

/// Reads the state.
pub(crate) fn get_state(state: &Mutex<WebServerState>) -> WebServerState {
    state.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A running child and its monitor task.
#[derive(Debug)]
pub(crate) struct Supervised {
    stop: oneshot::Sender<()>,
    monitor: JoinHandle<()>,
}

/// What a monitor needs to report on its child.
#[derive(Debug, Clone)]
pub(crate) struct ChildInfo {
    /// Human-readable name, e.g. `web server`.
    pub(crate) name: String,
    /// Process id.
    pub(crate) pid: u32,
    /// Pid file, removed when the child exits.
    pub(crate) pid_file: PathBuf,
    /// Output of the child, quoted in failures.
    pub(crate) log: PathBuf,
}

impl Supervised {
    /// Hands `child` to a monitor task and marks it running.
    pub(crate) fn start(child: Child, info: ChildInfo, state: SharedState) -> Self {
        set_state(&state, WebServerState::Running { pid: info.pid });
        let (stop, stop_rx) = oneshot::channel();
        let monitor = tokio::spawn(monitor(child, info, stop_rx, state));
        Self { stop, monitor }
    }

    /// Whether the child has exited by itself.
    pub(crate) fn is_finished(&self) -> bool {
        self.monitor.is_finished()
    }

    /// Stops the child and waits for it to exit.
    pub(crate) async fn stop(self) {
        // Fails if the monitor already ended (the child exited by itself).
        let _ = self.stop.send(());
        if let Err(err) = self.monitor.await {
            tracing::error!(error = %err, "process monitor task failed");
        }
    }
}

/// Owns `child` until it exits.
async fn monitor(
    mut child: Child,
    info: ChildInfo,
    stop: oneshot::Receiver<()>,
    state: SharedState,
) {
    let ChildInfo {
        name,
        pid,
        pid_file,
        log,
    } = info;
    tokio::select! {
        status = child.wait() => {
            let message = match status {
                Ok(status) => format!(
                    "the {name} exited unexpectedly ({status}); see `{}`",
                    log.display()
                ),
                Err(err) => format!("lost track of the {name}: {err}"),
            };
            tracing::error!(pid, error = %message, "{name} stopped");
            set_state(&state, WebServerState::Failed { message });
        }
        _ = stop => {
            if let Err(err) = devspace_platform::process::request_stop(pid) {
                tracing::warn!(pid, error = %err, "failed to ask the {name} to stop");
            }
            if tokio::time::timeout(STOP_TIMEOUT, child.wait()).await.is_err() {
                tracing::warn!(pid, timeout_secs = STOP_TIMEOUT.as_secs(), "{name} did not stop in time; killing it");
                let _ = child.kill().await;
            }
            tracing::info!(pid, "{name} stopped");
            set_state(&state, WebServerState::Stopped);
        }
    }
    let _ = fs::remove_file(&pid_file);
}

/// Stops a process left running by a previous daemon (crash or kill), if
/// `pid_file` designates a process that still runs `executable`, then
/// removes the pid file. Blocking.
pub(crate) fn stop_orphan(pid_file: &Path, executable: Option<&Path>, name: &str) {
    let Ok(text) = fs::read_to_string(pid_file) else {
        return;
    };
    if let (Ok(pid), Some(executable)) = (text.trim().parse::<u32>(), executable)
        && runs_executable(pid, executable)
    {
        tracing::warn!(pid, "stopping the {name} left by a previous daemon");
        if let Err(err) = devspace_platform::process::request_stop(pid) {
            tracing::warn!(pid, error = %err, "failed to stop the previous {name}");
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        while runs_executable(pid, executable) && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        if runs_executable(pid, executable) {
            let _ = devspace_platform::process::kill(pid);
        }
    }
    let _ = fs::remove_file(pid_file);
}

fn runs_executable(pid: u32, executable: &Path) -> bool {
    let canonical = |path: &Path| devspace_platform::fs::canonicalize(path).ok();
    match devspace_platform::process::executable_path(pid) {
        Ok(path) => canonical(&path).is_some() && canonical(&path) == canonical(executable),
        Err(_) => false,
    }
}

/// Last `lines` lines of a log file, for startup errors.
pub(crate) fn log_tail(path: &Path, lines: usize) -> String {
    match fs::read_to_string(path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(err) => format!("(could not read the log: {err})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_tail_keeps_the_last_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("caddy.log");
        let text: String = (0..40).map(|i| format!("line {i}\n")).collect();
        fs::write(&log, text).unwrap();
        let tail = log_tail(&log, 15);
        assert_eq!(tail.lines().count(), 15);
        assert!(tail.ends_with("line 39"));
        assert!(log_tail(&tmp.path().join("absent"), 15).starts_with("(could not read"));
    }

    #[test]
    fn orphan_cleanup_ignores_unrelated_processes() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("caddy.pid");
        let executable = tmp.path().join("caddy");
        fs::write(&executable, "").unwrap();
        // The test process itself does not run that executable.
        fs::write(&pid_file, std::process::id().to_string()).unwrap();
        stop_orphan(&pid_file, Some(&executable), "web server");
        assert!(!pid_file.exists());

        fs::write(&pid_file, "garbage").unwrap();
        stop_orphan(&pid_file, Some(&executable), "web server");
        assert!(!pid_file.exists());

        fs::write(&pid_file, "1").unwrap();
        stop_orphan(&pid_file, None, "web server");
        assert!(!pid_file.exists());
    }
}
