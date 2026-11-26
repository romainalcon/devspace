//! Versioned IPC messages.
//!
//! Messages are JSON objects tagged by a `type` field. Example session:
//!
//! ```text
//! → {"type":"hello","protocol_version":2,"client":{"name":"devspace-cli","version":"0.1.0"}}
//! ← {"type":"welcome","protocol_version":2,"daemon":{"version":"0.1.0","pid":4242}}
//! → {"type":"request","id":1,"request":{"method":"status"}}
//! ← {"type":"response","id":1,"response":{"kind":"status","version":"0.1.0","pid":4242,"uptime_secs":12}}
//! ```
//!
//! Compatibility rules:
//! - the handshake messages (`hello`, `welcome`, `rejected`) and their
//!   `protocol_version` field never change shape, so that any two versions
//!   of DevSpace can detect a mismatch;
//! - any other change to the messages increments [`PROTOCOL_VERSION`];
//! - peers only talk when their versions are equal.
//!
//! History: version 1 (M1) had `ping`, `status` and `shutdown`; version 2
//! (M2) adds the project registry and the `error` response; version 3 (M3)
//! adds the web server requests and the `unavailable` and `failed` error
//! codes; version 4 (M4) adds the runtime requests, PHP sites and the PHP
//! process managers in the web status; version 5 (M5) adds the service
//! requests.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Version of the protocol implemented by this build.
pub const PROTOCOL_VERSION: u32 = 6;

/// Name and version of a program taking part in a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Program name, e.g. `devspace-cli`.
    pub name: String,
    /// Program version.
    pub version: String,
}

/// Identity of the daemon, sent in the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// Daemon version.
    pub version: String,
    /// Process id of the daemon.
    pub pid: u32,
}

/// Message sent by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// First message of every connection.
    Hello {
        /// Protocol version spoken by the client.
        protocol_version: u32,
        /// Client identity, for logs.
        client: PeerInfo,
    },
    /// A request, answered by a [`ServerMessage::Response`] with the same id.
    Request {
        /// Client-chosen identifier, echoed in the response.
        id: u64,
        /// The request itself.
        request: Request,
    },
}

/// Message sent by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Handshake accepted.
    Welcome {
        /// Protocol version spoken by the daemon.
        protocol_version: u32,
        /// Daemon identity.
        daemon: DaemonInfo,
    },
    /// Handshake refused because of a protocol mismatch; the daemon closes
    /// the connection afterwards.
    Rejected {
        /// Protocol version spoken by the daemon.
        protocol_version: u32,
        /// Human-readable explanation.
        message: String,
    },
    /// Answer to a [`ClientMessage::Request`].
    Response {
        /// Identifier of the request.
        id: u64,
        /// The response itself.
        response: Response,
    },
}

/// Operations a client can ask the daemon to perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    /// Liveness check, answered by [`Response::Pong`].
    Ping,
    /// Daemon status, answered by [`Response::Status`].
    Status,
    /// Graceful shutdown, answered by [`Response::ShuttingDown`] before the
    /// daemon stops.
    Shutdown,
    /// Registers a project directory, answered by
    /// [`Response::ProjectAdded`].
    ProjectAdd {
        /// Absolute path of the project directory, as seen by the client.
        path: PathBuf,
        /// Explicit project name; when absent the daemon uses the project
        /// manifest or the directory name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Lists registered projects, answered by [`Response::Projects`].
    ProjectList,
    /// Unregisters a project, answered by [`Response::ProjectRemoved`]. The
    /// project directory is not modified.
    ProjectRemove {
        /// Name of the project.
        name: String,
    },
    /// Downloads and installs the pinned Caddy release if needed, answered
    /// by [`Response::WebStatus`]. May take minutes.
    WebInstall,
    /// Starts the web server if it is not running, answered by
    /// [`Response::WebStatus`].
    WebStart,
    /// Stops the web server, answered by [`Response::WebStatus`].
    WebStop,
    /// Restarts the web server with a configuration regenerated from the
    /// projects and their manifests, answered by [`Response::WebStatus`].
    WebRestart,
    /// State of the web server, DNS server, local CA and system
    /// integrations, answered by [`Response::WebStatus`].
    WebStatus,
    /// Runtime versions published for this platform and installed,
    /// answered by [`Response::Runtimes`].
    RuntimeList,
    /// Downloads the package index published with the DevSpace releases and
    /// uses it if it is signed and newer, answered by
    /// [`Response::Runtimes`].
    IndexRefresh,
    /// Downloads, verifies and installs a runtime version, answered by
    /// [`Response::RuntimeInstalled`]. May take minutes.
    RuntimeInstall {
        /// Runtime name (`php`, `node`).
        name: String,
        /// `MAJOR`, `MAJOR.MINOR`, `MAJOR.MINOR.PATCH` or
        /// `MAJOR.MINOR.PATCH.BUILD`; the newest published match is
        /// installed. Absent: the newest version.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// Removes an installed runtime version, answered by
    /// [`Response::RuntimeRemoved`].
    RuntimeRemove {
        /// Runtime name.
        name: String,
        /// Exact version.
        version: String,
    },
    /// Runtime versions to use in a directory (the registered project
    /// containing it, if any), answered by [`Response::RuntimeEnv`].
    RuntimeEnv {
        /// Absolute directory, as seen by the client.
        directory: PathBuf,
    },
    /// Service instances, answered by [`Response::Services`].
    ServiceList,
    /// Starts a service instance and keeps it running across daemon
    /// restarts, answered by [`Response::Services`].
    ServiceStart {
        /// Service name (`mysql`, `postgresql`, `valkey`).
        name: String,
        /// Series (`18`, `8.4`); absent: the newest installed one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        series: Option<String>,
    },
    /// Stops a service instance and keeps it stopped, even when projects
    /// require it, until it is started again; answered by
    /// [`Response::Services`].
    ServiceStop {
        /// Service name.
        name: String,
        /// Series; absent: every series of the service.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        series: Option<String>,
    },
}

impl Request {
    /// Time a client waits for the response before giving up.
    ///
    /// Downloads and service initialisations legitimately take minutes; the
    /// other requests answer within seconds. Shared by every client so that
    /// the CLI and the desktop application wait the same time.
    #[must_use]
    pub fn time_limit(&self) -> Duration {
        const MINUTE: u64 = 60;
        let secs = match self {
            Self::Ping
            | Self::Status
            | Self::Shutdown
            | Self::ProjectAdd { .. }
            | Self::ProjectList
            | Self::ProjectRemove { .. } => 10,
            // Stop, then start, of Caddy and of the PHP process managers.
            Self::WebStart | Self::WebStop | Self::WebRestart | Self::WebStatus => 45,
            // Download of about 20 MB.
            Self::WebInstall => 15 * MINUTE,
            // Downloads of up to 1 GB (25 min each at most).
            Self::RuntimeInstall { .. } => 45 * MINUTE,
            Self::RuntimeList
            | Self::IndexRefresh
            | Self::RuntimeRemove { .. }
            | Self::RuntimeEnv { .. }
            | Self::ServiceList => MINUTE,
            // The first start initialises the data directory.
            Self::ServiceStart { .. } | Self::ServiceStop { .. } => 10 * MINUTE,
        };
        Duration::from_secs(secs)
    }
}

/// Answers of the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// Answer to [`Request::Ping`].
    Pong,
    /// Answer to [`Request::Status`].
    Status(DaemonStatus),
    /// Answer to [`Request::Shutdown`].
    ShuttingDown,
    /// Answer to [`Request::ProjectAdd`]: the registered project.
    ProjectAdded(ProjectInfo),
    /// Answer to [`Request::ProjectList`], sorted by name.
    Projects {
        /// Registered projects.
        projects: Vec<ProjectInfo>,
    },
    /// Answer to [`Request::ProjectRemove`]: the removed project.
    ProjectRemoved(ProjectInfo),
    /// Answer to the web requests: the state after the request.
    WebStatus(Box<WebStatus>),
    /// Answer to [`Request::RuntimeList`] and [`Request::IndexRefresh`].
    Runtimes(RuntimeList),
    /// Answer to [`Request::RuntimeInstall`].
    RuntimeInstalled(RuntimeInfo),
    /// Answer to [`Request::RuntimeRemove`].
    RuntimeRemoved(RuntimeInfo),
    /// Answer to [`Request::RuntimeEnv`].
    RuntimeEnv(RuntimeEnv),
    /// Answer to the service requests: the state after the request.
    Services(ServiceList),
    /// The request was understood but failed.
    Error(ErrorResponse),
}

/// A registered project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Unique project name.
    pub name: String,
    /// Canonical absolute path of the project directory.
    pub path: PathBuf,
    /// Registration time, in seconds since the Unix epoch.
    pub added_at: u64,
}

/// Failure of a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Machine-readable category.
    pub code: ErrorCode,
    /// Human-readable explanation, possibly on several lines.
    pub message: String,
}

/// Category of a failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request arguments are invalid (name, path, manifest, …).
    InvalidRequest,
    /// The resource already exists.
    AlreadyExists,
    /// The resource does not exist.
    NotFound,
    /// The request cannot be performed in the current state (e.g. the web
    /// server is not installed, or the platform does not support it).
    Unavailable,
    /// An external operation failed (download, web server start, …).
    Failed,
    /// The daemon failed to process a valid request.
    Internal,
}

/// State of the web server and of everything making the local sites
/// reachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebStatus {
    /// Pinned Caddy version.
    pub caddy_version: String,
    /// Whether that version is installed.
    pub caddy_installed: bool,
    /// Web server process.
    pub server: WebServerState,
    /// Top-level domain of the sites.
    pub tld: String,
    /// Loopback address of the sites.
    pub address: String,
    /// HTTP port of the web server on that address.
    pub http_port: u16,
    /// HTTPS port of the web server on that address.
    pub https_port: u16,
    /// UDP port of the DNS server on `127.0.0.1`.
    pub dns_port: u16,
    /// DNS server.
    pub dns: DnsServerState,
    /// Local certificate authority, once the web server created it.
    pub certificate_authority: Option<CertificateAuthorityInfo>,
    /// Machine-wide integrations installed by the privileged helper.
    pub system: SystemIntegrations,
    /// Site of every registered project, sorted by project name.
    pub sites: Vec<SiteInfo>,
    /// PHP process managers serving the PHP sites, sorted by version.
    pub php: Vec<PhpPoolInfo>,
}

/// PHP-FPM process manager of one PHP version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhpPoolInfo {
    /// PHP version.
    pub version: String,
    /// Process state.
    pub state: WebServerState,
}

/// Runtime versions known to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeList {
    /// Serial of the package index.
    pub index_serial: u64,
    /// Where the index comes from: `embedded` in the build, or
    /// `downloaded` from the releases.
    pub index_source: String,
    /// Why the last download of the published index failed, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_error: Option<String>,
    /// Platform key of the index, e.g. `macos-aarch64`.
    pub platform: String,
    /// Every published or installed version, sorted by name then version.
    pub versions: Vec<RuntimeVersionInfo>,
}

/// One runtime version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeVersionInfo {
    /// Runtime name.
    pub name: String,
    /// Exact version.
    pub version: String,
    /// Published in the package index for this platform.
    pub available: bool,
    /// Installation directory, when installed.
    pub path: Option<PathBuf>,
}

/// An installed runtime version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// Runtime name.
    pub name: String,
    /// Exact version.
    pub version: String,
    /// Installation directory.
    pub path: PathBuf,
    /// For an installation: `false` when the version was already
    /// installed.
    pub changed: bool,
}

/// Runtime versions to use in a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEnv {
    /// Registered project containing the directory.
    pub project: Option<String>,
    /// Selected installed versions.
    pub runtimes: Vec<RuntimeEnvEntry>,
    /// Requirements of the project manifest that no installed version
    /// satisfies, as explanations.
    pub missing: Vec<String>,
}

/// One selected runtime version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEnvEntry {
    /// Runtime name.
    pub name: String,
    /// Exact version.
    pub version: String,
    /// Directories to put first in `PATH`.
    pub bin_dirs: Vec<PathBuf>,
    /// Environment variables to set, e.g. `PHPRC`.
    pub variables: Vec<(String, String)>,
}

/// Service instances known to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceList {
    /// Loopback address the services listen on.
    pub address: String,
    /// Every installed, required or chosen instance, sorted by service
    /// then series.
    pub instances: Vec<ServiceInstanceInfo>,
    /// Requirements of the projects that no installed version satisfies,
    /// as explanations.
    pub missing: Vec<String>,
}

/// One service instance: a service and a series of versions sharing a
/// data directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInstanceInfo {
    /// Service name.
    pub name: String,
    /// Series (`18`, `8.4`).
    pub series: String,
    /// Installed version it runs, if any.
    pub version: Option<String>,
    /// TCP port on [`ServiceList::address`].
    pub port: u16,
    /// Administrator account, without a password.
    pub user: String,
    /// Choice of the user.
    pub mode: ServiceMode,
    /// Registered projects requiring it.
    pub required_by: Vec<String>,
    /// Server process.
    pub state: WebServerState,
    /// Why it does not run although it should.
    pub blocked: Option<String>,
    /// Data directory (created at the first start).
    pub data_dir: PathBuf,
    /// Output of the server.
    pub log_file: PathBuf,
}

/// Choice of the user for a service instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceMode {
    /// Runs when a registered project requires it.
    Automatic,
    /// Started by the user: runs even if no project requires it.
    Started,
    /// Stopped by the user: does not run even if projects require it.
    Stopped,
}

/// State of the web server process (also of the PHP process managers and
/// of the service servers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WebServerState {
    /// Not running.
    Stopped,
    /// Started, not accepting connections yet.
    Starting,
    /// Running and accepting connections.
    Running {
        /// Process id of the web server.
        pid: u32,
    },
    /// Stopped after a failure.
    Failed {
        /// What happened.
        message: String,
    },
}

/// State of the DNS server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DnsServerState {
    /// Answering queries.
    Listening,
    /// Could not start.
    Failed {
        /// What happened.
        message: String,
    },
}

/// Root certificate of the local certificate authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateAuthorityInfo {
    /// Certificate in PEM form.
    pub certificate_pem: String,
    /// SHA-256 fingerprint, 64 uppercase hexadecimal digits.
    pub sha256: String,
}

/// Machine-wide integrations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemIntegrations {
    /// Resolution of `*.<tld>` by the DNS server.
    pub resolver: IntegrationStatus,
    /// Loopback address of the sites and forwarding of its ports 80 and
    /// 443 to the web server.
    pub port_forwarding: IntegrationStatus,
    /// Trust of the local certificate authority.
    pub certificate_trust: IntegrationStatus,
    /// Whether port 80 of the sites address currently reaches the DevSpace
    /// web server; `None` when the web server is not running.
    pub forwarding_active: Option<bool>,
}

/// State of one integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    /// Installed with the current settings.
    Installed,
    /// Not installed.
    Missing,
    /// Installed with other settings; setting it up again updates it.
    Outdated,
    /// The location is used by something else, which DevSpace never
    /// modifies.
    Conflict,
    /// Not available on this platform yet.
    Unsupported,
    /// Could not be determined (see the daemon logs).
    Unknown,
}

/// Local site of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteInfo {
    /// Project name.
    pub project: String,
    /// Domain, `<project>.<tld>`.
    pub domain: String,
    /// What the site serves.
    pub target: SiteTargetInfo,
}

/// Content of a site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SiteTargetInfo {
    /// Static files.
    Static {
        /// Directory served.
        root: PathBuf,
    },
    /// PHP application.
    Php {
        /// Document root.
        root: PathBuf,
        /// PHP version.
        version: String,
    },
    /// Development server.
    Proxy {
        /// Loopback address of the server.
        upstream: String,
    },
    /// Not served.
    Unavailable {
        /// Explanation.
        reason: String,
    },
}

/// State of the running daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// Daemon version.
    pub version: String,
    /// Process id of the daemon.
    pub pid: u32,
    /// Seconds since the daemon started.
    pub uptime_secs: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn assert_wire<T>(message: &T, expected: &serde_json::Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(&serde_json::to_value(message).unwrap(), expected);
        assert_eq!(
            &serde_json::from_value::<T>(expected.clone()).unwrap(),
            message
        );
    }

    #[test]
    fn long_requests_get_long_time_limits() {
        let quick = Request::ProjectList.time_limit();
        let install = Request::RuntimeInstall {
            name: "php".into(),
            version: None,
        }
        .time_limit();
        let service = Request::ServiceStart {
            name: "mysql".into(),
            series: None,
        }
        .time_limit();
        assert_eq!(quick, Duration::from_secs(10));
        assert!(Request::WebInstall.time_limit() >= Duration::from_secs(10 * 60));
        assert!(install >= Duration::from_secs(30 * 60));
        assert!(service > Request::ServiceList.time_limit());
        assert!(Request::WebRestart.time_limit() > quick);
    }

    #[test]
    fn handshake_wire_format_is_stable() {
        assert_wire(
            &ClientMessage::Hello {
                protocol_version: 1,
                client: PeerInfo {
                    name: "devspace-cli".into(),
                    version: "0.1.0".into(),
                },
            },
            &json!({"type": "hello", "protocol_version": 1,
                    "client": {"name": "devspace-cli", "version": "0.1.0"}}),
        );
        assert_wire(
            &ServerMessage::Welcome {
                protocol_version: 1,
                daemon: DaemonInfo {
                    version: "0.1.0".into(),
                    pid: 42,
                },
            },
            &json!({"type": "welcome", "protocol_version": 1,
                    "daemon": {"version": "0.1.0", "pid": 42}}),
        );
        assert_wire(
            &ServerMessage::Rejected {
                protocol_version: 1,
                message: "nope".into(),
            },
            &json!({"type": "rejected", "protocol_version": 1, "message": "nope"}),
        );
    }

    #[test]
    fn hello_from_a_newer_client_with_extra_fields_is_readable() {
        let hello: ClientMessage = serde_json::from_value(json!({
            "type": "hello", "protocol_version": 7,
            "client": {"name": "future", "version": "9.0.0", "extra": true},
            "capabilities": ["streaming"],
        }))
        .unwrap();
        assert!(matches!(
            hello,
            ClientMessage::Hello {
                protocol_version: 7,
                ..
            }
        ));
    }

    #[test]
    fn request_and_response_wire_format() {
        assert_wire(
            &ClientMessage::Request {
                id: 3,
                request: Request::Status,
            },
            &json!({"type": "request", "id": 3, "request": {"method": "status"}}),
        );
        assert_wire(
            &ServerMessage::Response {
                id: 3,
                response: Response::Status(DaemonStatus {
                    version: "0.1.0".into(),
                    pid: 42,
                    uptime_secs: 5,
                }),
            },
            &json!({"type": "response", "id": 3, "response":
                    {"kind": "status", "version": "0.1.0", "pid": 42, "uptime_secs": 5}}),
        );
        assert_wire(&Response::ShuttingDown, &json!({"kind": "shutting_down"}));
        assert_wire(&Request::Ping, &json!({"method": "ping"}));
        assert_wire(&Request::Shutdown, &json!({"method": "shutdown"}));
        assert_wire(&Response::Pong, &json!({"kind": "pong"}));
    }

    #[test]
    fn project_wire_format() {
        assert_wire(
            &Request::ProjectAdd {
                path: PathBuf::from("/home/me/blog"),
                name: Some("blog".into()),
            },
            &json!({"method": "project_add", "path": "/home/me/blog", "name": "blog"}),
        );
        assert_wire(
            &Request::ProjectAdd {
                path: PathBuf::from("/home/me/blog"),
                name: None,
            },
            &json!({"method": "project_add", "path": "/home/me/blog"}),
        );
        assert_wire(&Request::ProjectList, &json!({"method": "project_list"}));
        assert_wire(
            &Request::ProjectRemove {
                name: "blog".into(),
            },
            &json!({"method": "project_remove", "name": "blog"}),
        );

        let blog = ProjectInfo {
            name: "blog".into(),
            path: PathBuf::from("/home/me/blog"),
            added_at: 1_700_000_000,
        };
        let blog_json = json!({"name": "blog", "path": "/home/me/blog", "added_at": 1_700_000_000});
        let mut added = blog_json.clone();
        added["kind"] = json!("project_added");
        assert_wire(&Response::ProjectAdded(blog.clone()), &added);
        let mut removed = blog_json.clone();
        removed["kind"] = json!("project_removed");
        assert_wire(&Response::ProjectRemoved(blog.clone()), &removed);
        assert_wire(
            &Response::Projects {
                projects: vec![blog],
            },
            &json!({"kind": "projects", "projects": [blog_json]}),
        );
        assert_wire(
            &Response::Error(ErrorResponse {
                code: ErrorCode::NotFound,
                message: "no project named `x` is registered".into(),
            }),
            &json!({"kind": "error", "code": "not_found",
                    "message": "no project named `x` is registered"}),
        );
    }

    #[test]
    fn web_wire_format() {
        for (request, method) in [
            (Request::WebInstall, "web_install"),
            (Request::WebStart, "web_start"),
            (Request::WebStop, "web_stop"),
            (Request::WebRestart, "web_restart"),
            (Request::WebStatus, "web_status"),
        ] {
            assert_wire(&request, &json!({"method": method}));
        }
        let status = WebStatus {
            caddy_version: "2.11.9".into(),
            caddy_installed: true,
            server: WebServerState::Running { pid: 7 },
            tld: "internal".into(),
            address: "127.0.0.2".into(),
            http_port: 47080,
            https_port: 47443,
            dns_port: 47053,
            dns: DnsServerState::Failed {
                message: "port in use".into(),
            },
            certificate_authority: Some(CertificateAuthorityInfo {
                certificate_pem: "PEM".into(),
                sha256: "AB".into(),
            }),
            system: SystemIntegrations {
                resolver: IntegrationStatus::Installed,
                port_forwarding: IntegrationStatus::Conflict,
                certificate_trust: IntegrationStatus::Unsupported,
                forwarding_active: None,
            },
            sites: vec![
                SiteInfo {
                    project: "blog".into(),
                    domain: "blog.test".into(),
                    target: SiteTargetInfo::Static {
                        root: PathBuf::from("/src/blog/public"),
                    },
                },
                SiteInfo {
                    project: "api".into(),
                    domain: "api.test".into(),
                    target: SiteTargetInfo::Proxy {
                        upstream: "127.0.0.1:3000".into(),
                    },
                },
                SiteInfo {
                    project: "shop".into(),
                    domain: "shop.test".into(),
                    target: SiteTargetInfo::Php {
                        root: PathBuf::from("/src/shop/public"),
                        version: "8.4.23".into(),
                    },
                },
            ],
            php: vec![PhpPoolInfo {
                version: "8.4.23".into(),
                state: WebServerState::Running { pid: 9 },
            }],
        };
        assert_wire(
            &Response::WebStatus(Box::new(status)),
            &json!({
                "kind": "web_status",
                "caddy_version": "2.11.9",
                "caddy_installed": true,
                "server": {"state": "running", "pid": 7},
                "tld": "internal",
                "address": "127.0.0.2",
                "http_port": 47080,
                "https_port": 47443,
                "dns_port": 47053,
                "dns": {"state": "failed", "message": "port in use"},
                "certificate_authority": {"certificate_pem": "PEM", "sha256": "AB"},
                "system": {
                    "resolver": "installed",
                    "port_forwarding": "conflict",
                    "certificate_trust": "unsupported",
                    "forwarding_active": null,
                },
                "sites": [
                    {"project": "blog", "domain": "blog.test",
                     "target": {"type": "static", "root": "/src/blog/public"}},
                    {"project": "api", "domain": "api.test",
                     "target": {"type": "proxy", "upstream": "127.0.0.1:3000"}},
                    {"project": "shop", "domain": "shop.test",
                     "target": {"type": "php", "root": "/src/shop/public", "version": "8.4.23"}},
                ],
                "php": [{"version": "8.4.23", "state": {"state": "running", "pid": 9}}],
            }),
        );
        assert_wire(&ErrorCode::Unavailable, &json!("unavailable"));
        assert_wire(&ErrorCode::Failed, &json!("failed"));
    }

    #[test]
    fn runtime_wire_format() {
        assert_wire(&Request::RuntimeList, &json!({"method": "runtime_list"}));
        assert_wire(&Request::IndexRefresh, &json!({"method": "index_refresh"}));
        assert_wire(
            &Request::RuntimeInstall {
                name: "php".into(),
                version: Some("8.4".into()),
            },
            &json!({"method": "runtime_install", "name": "php", "version": "8.4"}),
        );
        assert_wire(
            &Request::RuntimeInstall {
                name: "node".into(),
                version: None,
            },
            &json!({"method": "runtime_install", "name": "node"}),
        );
        assert_wire(
            &Request::RuntimeRemove {
                name: "php".into(),
                version: "8.4.23".into(),
            },
            &json!({"method": "runtime_remove", "name": "php", "version": "8.4.23"}),
        );
        assert_wire(
            &Request::RuntimeEnv {
                directory: PathBuf::from("/src/shop"),
            },
            &json!({"method": "runtime_env", "directory": "/src/shop"}),
        );
        assert_wire(
            &Response::Runtimes(RuntimeList {
                index_serial: 1,
                index_source: "embedded".into(),
                index_error: None,
                platform: "macos-aarch64".into(),
                versions: vec![RuntimeVersionInfo {
                    name: "node".into(),
                    version: "24.22.3".into(),
                    available: true,
                    path: None,
                }],
            }),
            &json!({"kind": "runtimes", "index_serial": 1, "index_source": "embedded",
                    "platform": "macos-aarch64",
                    "versions": [{"name": "node", "version": "24.22.3", "available": true,
                                  "path": null}]}),
        );
        assert_wire(
            &Response::RuntimeInstalled(RuntimeInfo {
                name: "php".into(),
                version: "8.4.23".into(),
                path: PathBuf::from("/data/runtimes/php/8.4.23"),
                changed: true,
            }),
            &json!({"kind": "runtime_installed", "name": "php", "version": "8.4.23",
                    "path": "/data/runtimes/php/8.4.23", "changed": true}),
        );
        assert_wire(
            &Response::RuntimeEnv(RuntimeEnv {
                project: Some("shop".into()),
                runtimes: vec![RuntimeEnvEntry {
                    name: "php".into(),
                    version: "8.4.23".into(),
                    bin_dirs: vec![PathBuf::from("/data/runtimes/php/8.4.23/bin")],
                    variables: vec![("PHPRC".into(), "/data/php/8.4.23".into())],
                }],
                missing: vec!["node 24 is not installed".into()],
            }),
            &json!({"kind": "runtime_env", "project": "shop",
                    "runtimes": [{"name": "php", "version": "8.4.23",
                                  "bin_dirs": ["/data/runtimes/php/8.4.23/bin"],
                                  "variables": [["PHPRC", "/data/php/8.4.23"]]}],
                    "missing": ["node 24 is not installed"]}),
        );
    }

    #[test]
    fn service_wire_format() {
        assert_wire(&Request::ServiceList, &json!({"method": "service_list"}));
        assert_wire(
            &Request::ServiceStart {
                name: "postgresql".into(),
                series: Some("18".into()),
            },
            &json!({"method": "service_start", "name": "postgresql", "series": "18"}),
        );
        assert_wire(
            &Request::ServiceStop {
                name: "mysql".into(),
                series: None,
            },
            &json!({"method": "service_stop", "name": "mysql"}),
        );
        assert_wire(
            &Response::Services(ServiceList {
                address: "127.0.0.2".into(),
                instances: vec![ServiceInstanceInfo {
                    name: "postgresql".into(),
                    series: "18".into(),
                    version: Some("18.7.0".into()),
                    port: 5432,
                    user: "postgres".into(),
                    mode: ServiceMode::Automatic,
                    required_by: vec!["blog".into()],
                    state: WebServerState::Running { pid: 7 },
                    blocked: None,
                    data_dir: PathBuf::from("/data/services/postgresql/18"),
                    log_file: PathBuf::from("/logs/postgresql-18.log"),
                }],
                missing: vec![],
            }),
            &json!({"kind": "services", "address": "127.0.0.2", "missing": [],
                    "instances": [{"name": "postgresql", "series": "18", "version": "18.7.0",
                                   "port": 5432, "user": "postgres", "mode": "automatic",
                                   "required_by": ["blog"],
                                   "state": {"state": "running", "pid": 7}, "blocked": null,
                                   "data_dir": "/data/services/postgresql/18",
                                   "log_file": "/logs/postgresql-18.log"}]}),
        );
    }

    #[test]
    fn unknown_method_is_rejected() {
        assert!(serde_json::from_value::<Request>(json!({"method": "format_disk"})).is_err());
    }
}
