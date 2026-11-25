//! DevSpace core engine.
//!
//! This crate holds the business logic shared by the daemon and the CLI. It
//! must stay independent from Tauri and from any graphical interface.
//!
//! Modules:
//! - [`archive`]: safe extraction of downloaded archives.
//! - [`config`]: typed global configuration (`config.toml`).
//! - [`db`]: SQLite storage of the daemon, with schema migrations.
//! - [`download`]: HTTPS downloads and SHA-256 checksums.
//! - [`error`]: the crate-wide [`Error`] type and error reporting helpers.
//! - [`instance`]: single-instance lock of the daemon.
//! - [`logging`]: `tracing` subscriber initialisation (stderr, rolling files).
//! - [`project`]: project registry and per-project manifest (`devspace.toml`).
//! - [`runtime`]: runtime package manager (PHP, Node.js): signed index,
//!   verified installation, versions side by side, PHP-FPM configuration.
//! - [`web`]: web server (Caddy), local domains, DNS answers and local
//!   certificate authority.
//! - [`ensure_unprivileged`]: guard refusing elevated execution.

pub mod archive;
pub mod config;
pub mod db;
pub mod download;
pub mod error;
pub mod instance;
pub mod logging;
pub mod project;
pub mod runtime;
pub mod web;

pub use devspace_platform::AppPaths;
pub use error::{Error, Result};

/// Fails if the current process runs as root / administrator.
///
/// Every DevSpace entry point calls this before doing any work.
///
/// # Errors
///
/// Returns [`Error::ElevatedPrivileges`] when elevated, or
/// [`Error::Platform`] if the privilege level cannot be determined.
pub fn ensure_unprivileged() -> Result<()> {
    if devspace_platform::privileges::is_elevated()? {
        return Err(Error::ElevatedPrivileges);
    }
    Ok(())
}
