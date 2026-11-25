//! End-to-end tests of the `devspaced` binary in an isolated `DEVSPACE_HOME`.

// Test helpers outside `#[test]` functions are not covered by the
// `allow-*-in-tests` settings of `clippy.toml`.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use devspace_core::AppPaths;
use devspace_ipc::{Client, Endpoint, IpcError, PeerInfo, Request, Response};

const TIMEOUT: Duration = Duration::from_secs(15);

fn peer() -> PeerInfo {
    PeerInfo {
        name: "lifecycle-test".into(),
        version: "0".into(),
    }
}

fn daemon_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devspaced"));
    command
        .arg("--background")
        .env("DEVSPACE_HOME", home)
        .env_remove("DEVSPACE_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

fn endpoint(home: &Path) -> Endpoint {
    Endpoint::new(&AppPaths::from_root(home).runtime_dir).unwrap()
}

async fn wait_until_ready(endpoint: &Endpoint, child: &mut Child) -> Client {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match Client::connect(endpoint, peer()).await {
            Ok(client) => return client,
            Err(IpcError::NotRunning { .. }) => {}
            Err(err) => panic!("unexpected connection error: {err:?}"),
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("daemon exited during startup: {status}");
        }
        assert!(Instant::now() < deadline, "daemon not ready in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("daemon did not exit in time");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn serve_refuse_second_instance_and_shut_down() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = endpoint(home.path());
    let mut child = daemon_command(home.path())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut client = wait_until_ready(&endpoint, &mut child).await;
    assert_eq!(client.daemon().pid, child.id());
    assert_eq!(client.request(Request::Ping).await.unwrap(), Response::Pong);
    let Response::Status(status) = client.request(Request::Status).await.unwrap() else {
        panic!("expected a status response");
    };
    assert_eq!(status.pid, child.id());
    assert_eq!(status.version, env!("CARGO_PKG_VERSION"));

    // A second daemon refuses to start and leaves the first one untouched.
    let second = daemon_command(home.path())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("already running"), "{stderr}");
    let mut other = Client::connect(&endpoint, peer()).await.unwrap();
    assert_eq!(other.request(Request::Ping).await.unwrap(), Response::Pong);

    // Shutdown through IPC closes every connection and removes the endpoint.
    assert_eq!(
        client.request(Request::Shutdown).await.unwrap(),
        Response::ShuttingDown
    );
    assert!(wait_for_exit(&mut child).await.success());
    assert!(matches!(
        other.request(Request::Ping).await,
        Err(IpcError::ConnectionClosed | IpcError::Io(_))
    ));
    assert!(matches!(
        Client::connect(&endpoint, peer()).await,
        Err(IpcError::NotRunning { .. })
    ));

    // The run was logged to a file.
    let logs: Vec<_> = std::fs::read_dir(AppPaths::from_root(home.path()).log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        logs.iter()
            .any(|name| name.starts_with("devspaced.") && name.ends_with(".log")),
        "{logs:?}"
    );

    // The lock is released: the daemon can start again.
    let mut child = daemon_command(home.path())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut client = wait_until_ready(&endpoint, &mut child).await;
    client.request(Request::Shutdown).await.unwrap();
    assert!(wait_for_exit(&mut child).await.success());
}

// Signals are a Unix concept; the Windows console events cannot be sent to a
// detached process from a test.
#[cfg(unix)]
#[tokio::test]
async fn sigterm_triggers_graceful_shutdown() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = endpoint(home.path());
    let mut child = daemon_command(home.path())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let client = wait_until_ready(&endpoint, &mut child).await;
    drop(client);

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    assert!(wait_for_exit(&mut child).await.success());
    let socket = AppPaths::from_root(home.path())
        .runtime_dir
        .join("devspaced.sock");
    assert!(!socket.exists());
}
