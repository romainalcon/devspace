//! Operating-system specific primitives used by DevSpace.
//!
//! This crate is the only place where platform-specific code (`cfg(target_os)`,
//! system calls, OS conventions) is allowed. Higher-level crates consume the
//! portable API exposed here and never branch on the platform themselves.
//!
//! Current scope (milestone M0):
//! - [`paths`]: resolution of the application directories following each OS
//!   conventions.
//! - [`privileges`]: detection of elevated (root / administrator) execution.

pub mod error;
pub mod paths;
pub mod privileges;

pub use error::PlatformError;
pub use paths::AppPaths;
