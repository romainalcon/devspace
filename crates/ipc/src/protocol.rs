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
//! (M2) adds the project registry and the `error` response.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Version of the protocol implemented by this build.
pub const PROTOCOL_VERSION: u32 = 2;

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
    /// The daemon failed to process a valid request.
    Internal,
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
    fn unknown_method_is_rejected() {
        assert!(serde_json::from_value::<Request>(json!({"method": "format_disk"})).is_err());
    }
}
