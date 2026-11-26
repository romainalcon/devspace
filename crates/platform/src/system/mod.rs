//! System-wide integrations requiring administrator rights.
//!
//! These functions change machine-wide configuration (DNS resolver, packet
//! filter, trust store, installed helper, `devspace` command). The mutating ones must only be called by the
//! privileged helper (`devspace-helper`), after the user approved the
//! change; the main DevSpace processes only use the read-only queries
//! ([`resolver::state`], [`port_forwarding::state`],
//! [`trust_store::is_trusted`], [`helper_install::state`],
//! [`cli_link::state`]) and [`elevation`].
//!
//! Every file written by DevSpace contains [`MANAGED_MARKER`]. A file
//! without it belongs to someone else and is never modified or removed
//! ([`PlatformError::ForeignSystemFile`]).
//!
//! Implemented on macOS; other platforms return
//! [`PlatformError::Unsupported`].
//!
//! [`PlatformError::ForeignSystemFile`]: crate::PlatformError::ForeignSystemFile
//! [`PlatformError::Unsupported`]: crate::PlatformError::Unsupported

// Only the macOS integrations run system tools and write managed files.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod cli_link;
mod command;
pub mod elevation;
pub mod helper_install;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod managed;
pub mod port_forwarding;
pub mod resolver;
pub mod trust_store;

pub use managed::{IntegrationState, MANAGED_MARKER};
