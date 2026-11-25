//! Operating-system specific primitives used by DevSpace.
//!
//! This crate is the only place where platform-specific code (`cfg(target_os)`,
//! system calls, OS conventions) is allowed. Higher-level crates consume the
//! portable API exposed here and never branch on the platform themselves.
//!
//! Modules:
//! - [`fs`]: portable filesystem helpers (path canonicalisation).
//! - [`paths`]: resolution of the application directories following each OS
//!   conventions.
//! - [`privileges`]: detection of elevated (root / administrator) execution.
//! - [`local_socket`]: user-restricted local IPC transport (Unix domain
//!   sockets, Windows named pipes) with peer identity checks.
//! - [`signals`]: signals and console events requesting a graceful shutdown.
//! - [`process`]: spawning detached background processes.

pub mod error;
pub mod fs;
pub mod local_socket;
pub mod paths;
pub mod privileges;
pub mod process;
pub mod signals;

pub use error::PlatformError;
pub use paths::AppPaths;
