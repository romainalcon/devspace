//! Operating-system specific primitives used by DevSpace.
//!
//! This crate is the only place where platform-specific code (`cfg(target_os)`,
//! system calls, OS conventions) is allowed. Higher-level crates consume the
//! portable API exposed here and never branch on the platform themselves.
//!
//! Modules:
//! - [`fs`]: portable filesystem helpers (path canonicalisation).
//! - [`instance`]: single-instance lock of the daemon (OS advisory lock).
//! - [`paths`]: resolution of the application directories following each OS
//!   conventions.
//! - [`privileges`]: detection of elevated (root / administrator) execution.
//! - [`local_socket`]: user-restricted local IPC transport (Unix domain
//!   sockets, Windows named pipes) with peer identity checks.
//! - [`signals`]: signals and console events requesting a graceful shutdown.
//! - [`process`]: spawning detached processes, finding the executable of a
//!   process, stopping processes.
//! - [`system`]: machine-wide integrations performed by the privileged
//!   helper (local DNS resolver, port forwarding, trust store) and elevation.
//! - [`target`]: operating system and architecture of the build.

pub mod error;
pub mod fs;
pub mod instance;
pub mod local_socket;
pub mod paths;
pub mod privileges;
pub mod process;
pub mod signals;
pub mod system;
pub mod target;

pub use error::PlatformError;
pub use paths::AppPaths;
