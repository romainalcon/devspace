//! End-to-end tests of the `devspaced` binary in an isolated `DEVSPACE_HOME`.

// Test helpers outside `#[test]` functions are not covered by the
// `allow-*-in-tests` settings of `clippy.toml`.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use devspace_core::AppPaths;
use devspace_ipc::{Client, Endpoint, ErrorCode, IpcError, PeerInfo, Request, Response};

const TIMEOUT: Duration = Duration::from_secs(15);

/// Kills a daemon if the test panics, so that a failed test does not leave
/// it running.
struct KillOnPanic(u32);

impl Drop for KillOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = devspace_platform::process::kill(self.0);
        }
    }
}

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
    let _guard = KillOnPanic(child.id());

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
    let _guard = KillOnPanic(child.id());
    let mut client = wait_until_ready(&endpoint, &mut child).await;
    client.request(Request::Shutdown).await.unwrap();
    assert!(wait_for_exit(&mut child).await.success());
}

async fn start_daemon(home: &Path) -> (Child, Client) {
    let mut child = daemon_command(home).stderr(Stdio::null()).spawn().unwrap();
    let client = wait_until_ready(&endpoint(home), &mut child).await;
    (child, client)
}

async fn stop_daemon(mut child: Child, mut client: Client) {
    client.request(Request::Shutdown).await.unwrap();
    assert!(wait_for_exit(&mut child).await.success());
}

fn error_code(response: Response) -> ErrorCode {
    match response {
        Response::Error(error) => error.code,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test]
async fn project_registry_is_served_and_persisted() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let blog = workspace.path().join("Blog");
    std::fs::create_dir(&blog).unwrap();
    let blog = devspace_platform::fs::canonicalize(&blog).unwrap();

    let (child, mut client) = start_daemon(home.path()).await;
    let _guard = KillOnPanic(child.id());
    let Response::ProjectAdded(added) = client
        .request(Request::ProjectAdd {
            path: blog.clone(),
            name: None,
        })
        .await
        .unwrap()
    else {
        panic!("expected project_added");
    };
    assert_eq!(added.name, "blog");
    assert_eq!(added.path, blog);

    // Refusals are reported as typed errors and keep the connection usable.
    let duplicate = Request::ProjectAdd {
        path: blog.clone(),
        name: Some("other".into()),
    };
    assert_eq!(
        error_code(client.request(duplicate).await.unwrap()),
        ErrorCode::AlreadyExists
    );
    let relative = Request::ProjectAdd {
        path: "relative".into(),
        name: None,
    };
    assert_eq!(
        error_code(client.request(relative).await.unwrap()),
        ErrorCode::InvalidRequest
    );
    let bad_name = Request::ProjectRemove {
        name: "Not Valid".into(),
    };
    assert_eq!(
        error_code(client.request(bad_name).await.unwrap()),
        ErrorCode::InvalidRequest
    );
    stop_daemon(child, client).await;

    // The registry survives a restart.
    let (child, mut client) = start_daemon(home.path()).await;
    let _guard = KillOnPanic(child.id());
    assert_eq!(
        client.request(Request::ProjectList).await.unwrap(),
        Response::Projects {
            projects: vec![added.clone()]
        }
    );
    let removed = Request::ProjectRemove {
        name: "blog".into(),
    };
    assert_eq!(
        client.request(removed.clone()).await.unwrap(),
        Response::ProjectRemoved(added)
    );
    assert_eq!(
        error_code(client.request(removed).await.unwrap()),
        ErrorCode::NotFound
    );
    assert!(blog.is_dir());
    stop_daemon(child, client).await;
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
    let _guard = KillOnPanic(child.id());
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

/// Unprivileged port currently free on the loopback interface (UDP and TCP).
fn free_port() -> u16 {
    loop {
        let udp = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let port = udp.local_addr().unwrap().port();
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

#[tokio::test]
async fn dns_and_web_status_are_served() {
    use devspace_ipc::{DnsServerState, IntegrationStatus, SiteTargetInfo, WebServerState};

    let home = tempfile::tempdir().unwrap();
    let paths = AppPaths::from_root(home.path());
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    let (http, https, dns) = (free_port(), free_port(), free_port());
    std::fs::write(
        paths.config_file(),
        format!("[web]\nhttp_port = {http}\nhttps_port = {https}\ndns_port = {dns}\n"),
    )
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let blog = workspace.path().join("blog");
    std::fs::create_dir_all(blog.join("public")).unwrap();
    let blog = devspace_platform::fs::canonicalize(&blog).unwrap();

    let (child, mut client) = start_daemon(home.path()).await;
    let _guard = KillOnPanic(child.id());
    client
        .request(Request::ProjectAdd {
            path: blog.clone(),
            name: None,
        })
        .await
        .unwrap();

    // The DNS server answers for *.internal on the configured port.
    let socket = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
    let mut query = vec![0x42, 0x42, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in ["www", "blog", "internal"] {
        query.push(u8::try_from(label.len()).unwrap());
        query.extend_from_slice(label.as_bytes());
    }
    query.extend_from_slice(&[0, 0, 1, 0, 1]);
    socket.send_to(&query, ("127.0.0.1", dns)).await.unwrap();
    let mut response = [0u8; 512];
    let (len, _) = tokio::time::timeout(TIMEOUT, socket.recv_from(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response[len - 4..len], &[127, 0, 0, 2]);

    // Status without Caddy installed.
    let Response::WebStatus(status) = client.request(Request::WebStatus).await.unwrap() else {
        panic!("expected a web status");
    };
    assert!(!status.caddy_installed);
    assert_eq!(status.server, WebServerState::Stopped);
    assert_eq!(status.dns, DnsServerState::Listening);
    assert_eq!(
        (status.http_port, status.https_port, status.dns_port),
        (http, https, dns)
    );
    assert!(status.certificate_authority.is_none());
    assert_ne!(
        status.system.certificate_trust,
        IntegrationStatus::Installed
    );
    assert_eq!(status.sites.len(), 1);
    assert_eq!(
        (status.tld.as_str(), status.address.as_str()),
        ("internal", "127.0.0.2")
    );
    assert_eq!(status.sites[0].domain, "blog.internal");
    assert_eq!(
        status.sites[0].target,
        SiteTargetInfo::Static {
            root: blog.join("public")
        }
    );

    // Starting requires the installation, which tests never download.
    assert_eq!(
        error_code(client.request(Request::WebStart).await.unwrap()),
        ErrorCode::Unavailable
    );
    stop_daemon(child, client).await;
}
