//! Definitions shared by the DevSpace privileged helper and its callers.
//!
//! `devspace-helper` is the only DevSpace program meant to run with
//! administrator rights. It is started by the CLI through `sudo`, after the
//! user approved the change, performs exactly one [`Operation`] and exits.
//! It never runs as a service and accepts no free-form input: every
//! operation and argument is typed and validated, and the only data read
//! from standard input is a certificate whose fingerprint must match the
//! one approved by the user.
//!
//! This library holds what callers need to talk to the helper: the
//! operations and their command-line encoding, certificate handling, and
//! the validated local domain and loopback address.

pub mod certificate;
pub mod local;
pub mod operation;

pub use certificate::{Certificate, CertificateError, Fingerprint};
pub use local::{LoopbackAddress, Tld};
pub use operation::{Operation, OperationError};

/// File name (without extension) of the helper executable, installed next
/// to the `devspace` executable.
pub const HELPER_NAME: &str = "devspace-helper";
