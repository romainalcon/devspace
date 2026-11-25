//! DevSpace core engine.
//!
//! This crate holds the business logic shared by the daemon and the CLI. It
//! must stay independent from Tauri and from any graphical interface.
//!
//! Current scope (milestone M0):
//! - [`config`]: typed global configuration (`config.toml`).
//! - [`error`]: the crate-wide [`Error`] type and error reporting helpers.
//! - [`logging`]: `tracing` subscriber initialisation.
//! - [`ensure_unprivileged`]: guard refusing elevated execution.

pub mod config;
pub mod error;
pub mod logging;

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
