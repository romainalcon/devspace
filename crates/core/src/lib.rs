//! DevSpace core engine.
//!
//! This crate holds the business logic shared by the daemon and the CLI. It
//! must stay independent from Tauri and from any graphical interface.
//!
//! Modules:
//! - [`config`]: typed global configuration (`config.toml`).
//! - [`db`]: SQLite storage of the daemon, with schema migrations.
//! - [`error`]: the crate-wide [`Error`] type and error reporting helpers.
//! - [`instance`]: single-instance lock of the daemon.
//! - [`logging`]: `tracing` subscriber initialisation (stderr, rolling files).
//! - [`project`]: project registry and per-project manifest (`devspace.toml`).
//! - [`ensure_unprivileged`]: guard refusing elevated execution.

pub mod config;
pub mod db;
pub mod error;
pub mod instance;
pub mod logging;
pub mod project;

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
