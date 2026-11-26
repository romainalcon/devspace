//! Supervision of the child processes of the daemon (Caddy, PHP-FPM, the
//! service servers).
//!
//! A monitor task owns each child: it records an unexpected exit as a
//! failure in the shared state, and performs graceful stops (a signal,
//! then a kill after the timeout of its [`StopPolicy`]). The pid of every child is kept in
//! a file of the runtime directory, so that a daemon restarted after a
//! crash can stop what the previous one left (only if that pid still runs
//! the expected executable).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use devspace_ipc::WebServerState;
use devspace_platform::process::StopRequest;
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// How a child is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StopPolicy {
    /// Graceful stop request.
    pub(crate) request: StopRequest,
    /// Time given to stop before the child is killed.
    pub(crate) timeout: Duration,
}

impl StopPolicy {
    /// `SIGTERM`, then a kill after 5 s (Caddy, PHP-FPM).
    pub(crate) const DEFAULT: Self = Self {
        request: StopRequest::Terminate,
        timeout: Duration::from_secs(5),
    };
}

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
    /// How it is stopped.
    pub(crate) stop: StopPolicy,
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
        stop: policy,
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
            if let Err(err) = devspace_platform::process::request_stop_with(pid, policy.request) {
                tracing::warn!(pid, error = %err, "failed to ask the {name} to stop");
            }
            if tokio::time::timeout(policy.timeout, child.wait()).await.is_err() {
                tracing::warn!(pid, timeout_secs = policy.timeout.as_secs(), "{name} did not stop in time; killing it");
                let _ = child.kill().await;
            }
            tracing::info!(pid, "{name} stopped");
            set_state(&state, WebServerState::Stopped);
        }
    }
    let _ = fs::remove_file(&pid_file);
}

/// Stops a process left running by a previous daemon (crash or kill), if
/// `pid_file` designates a process that still runs one of `executables`,
/// then removes the pid file. Blocking.
pub(crate) fn stop_orphan(
    pid_file: &Path,
    executables: &[PathBuf],
    name: &str,
    policy: StopPolicy,
) {
    let Ok(text) = fs::read_to_string(pid_file) else {
        return;
    };
    let running = |pid: u32| {
        executables
            .iter()
            .any(|executable| runs_executable(pid, executable))
    };
    if let Ok(pid) = text.trim().parse::<u32>()
        && running(pid)
    {
        tracing::warn!(pid, "stopping the {name} left by a previous daemon");
        if let Err(err) = devspace_platform::process::request_stop_with(pid, policy.request) {
            tracing::warn!(pid, error = %err, "failed to stop the previous {name}");
        }
        let deadline = Instant::now() + policy.timeout;
        while running(pid) && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        if running(pid) {
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
        let policy = StopPolicy::DEFAULT;
        fs::write(&pid_file, std::process::id().to_string()).unwrap();
        stop_orphan(
            &pid_file,
            std::slice::from_ref(&executable),
            "web server",
            policy,
        );
        assert!(!pid_file.exists());

        fs::write(&pid_file, "garbage").unwrap();
        stop_orphan(&pid_file, &[executable], "web server", policy);
        assert!(!pid_file.exists());

        fs::write(&pid_file, "1").unwrap();
        stop_orphan(&pid_file, &[], "web server", policy);
        assert!(!pid_file.exists());
    }
}
