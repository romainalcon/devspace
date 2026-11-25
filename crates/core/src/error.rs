//! Error type of the core engine.

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;

use devspace_platform::PlatformError;

/// Convenience alias for results of the core engine.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error returned by the core engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Error coming from the platform layer.
    #[error(transparent)]
    Platform(#[from] PlatformError),

    /// The process runs as root / administrator.
    #[error(
        "DevSpace must not run with elevated privileges (root or administrator); \
         run it as a regular user"
    )]
    ElevatedPrivileges,

    /// The configuration file exists but could not be read.
    #[error("failed to read configuration file `{}`", path.display())]
    ConfigRead {
        /// Configuration file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The configuration file is not valid TOML or does not match the schema.
    #[error("invalid configuration file `{}`", path.display())]
    ConfigParse {
        /// Configuration file.
        path: PathBuf,
        /// Parser error, including the location of the problem.
        #[source]
        source: Box<toml::de::Error>,
    },

    /// The configuration declares an unsupported schema version.
    #[error(
        "configuration file `{}` uses schema version {found}, \
         but this version of DevSpace supports version {supported}",
        path.display()
    )]
    UnsupportedConfigVersion {
        /// Configuration file.
        path: PathBuf,
        /// Version found in the file.
        found: u32,
        /// Version supported by this build.
        supported: u32,
    },

    /// The configuration could not be serialized.
    #[error("failed to serialize configuration")]
    ConfigSerialize(#[source] toml::ser::Error),

    /// The configuration file could not be written.
    #[error("failed to write configuration file `{}`", path.display())]
    ConfigWrite {
        /// Configuration file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Refusing to overwrite an existing configuration file.
    #[error("configuration file `{}` already exists", path.display())]
    ConfigAlreadyExists {
        /// Configuration file.
        path: PathBuf,
    },

    /// The `DEVSPACE_LOG` filter directive is invalid.
    #[error("invalid log filter in {variable}")]
    InvalidLogFilter {
        /// Environment variable holding the filter.
        variable: &'static str,
        /// Parser error.
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },

    /// A global `tracing` subscriber was already installed.
    #[error("logging is already initialised")]
    LoggingAlreadyInitialised(#[source] tracing::subscriber::SetGlobalDefaultError),
}

/// Formats an error and its whole `source()` chain on multiple lines.
///
/// ```
/// let err = std::io::Error::other("boom");
/// assert_eq!(devspace_core::error::report(&err), "boom");
/// ```
#[must_use]
pub fn report(error: &(dyn std::error::Error + 'static)) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        // Writing to a String cannot fail.
        let _ = write!(out, "\n  caused by: {cause}");
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_includes_source_chain() {
        let err = Error::ConfigRead {
            path: PathBuf::from("config.toml"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "access denied"),
        };
        assert_eq!(
            report(&err),
            "failed to read configuration file `config.toml`\n  caused by: access denied"
        );
    }
}
