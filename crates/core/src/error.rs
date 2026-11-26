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

    /// The configuration holds inconsistent values.
    #[error("invalid configuration file `{}`: {reason}", path.display())]
    InvalidConfig {
        /// Configuration file.
        path: PathBuf,
        /// Broken rule.
        reason: &'static str,
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

    /// The rolling log file could not be opened.
    #[error("failed to open log files in `{}`", dir.display())]
    LogFile {
        /// Log directory.
        dir: PathBuf,
        /// Appender initialisation error.
        #[source]
        source: tracing_appender::rolling::InitError,
    },

    /// Another daemon instance holds the single-instance lock.
    #[error("another DevSpace daemon is already running (lock `{}` is held)", path.display())]
    AlreadyRunning {
        /// Lock file.
        path: PathBuf,
    },

    /// The single-instance lock file could not be opened or locked.
    #[error("failed to lock `{}`", path.display())]
    InstanceLock {
        /// Lock file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A global `tracing` subscriber was already installed.
    #[error("logging is already initialised")]
    LoggingAlreadyInitialised(#[source] tracing::subscriber::SetGlobalDefaultError),

    /// The database could not be opened.
    #[error("failed to open database `{}`", path.display())]
    DatabaseOpen {
        /// Database file.
        path: PathBuf,
        /// SQLite error.
        #[source]
        source: rusqlite::Error,
    },

    /// The database was created by a newer version of DevSpace.
    #[error(
        "database `{}` has schema version {found}, but this version of DevSpace supports \
         up to version {supported}; upgrade DevSpace",
        path.display()
    )]
    DatabaseTooNew {
        /// Database file.
        path: PathBuf,
        /// Version found in the database.
        found: u32,
        /// Latest version known by this build.
        supported: u32,
    },

    /// A database migration failed; the database is left at the previous
    /// schema version.
    #[error("failed to migrate database `{}` to schema version {version}", path.display())]
    DatabaseMigration {
        /// Database file.
        path: PathBuf,
        /// Version that could not be reached.
        version: u32,
        /// SQLite error.
        #[source]
        source: rusqlite::Error,
    },

    /// A database query failed.
    #[error("database error while {operation}")]
    Database {
        /// What was being done.
        operation: &'static str,
        /// SQLite error.
        #[source]
        source: rusqlite::Error,
    },

    /// A project name does not follow the naming rules.
    #[error("invalid project name `{name}`: {reason}")]
    InvalidProjectName {
        /// Rejected name.
        name: String,
        /// Broken rule.
        reason: &'static str,
    },

    /// No valid project name can be derived from the directory name.
    #[error(
        "cannot derive a project name from directory `{}`; choose one with `--name` \
         or in the project manifest",
        path.display()
    )]
    ProjectNameUnavailable {
        /// Project directory.
        path: PathBuf,
    },

    /// A project path is relative; the daemon cannot interpret it.
    #[error("project path `{}` must be absolute", path.display())]
    RelativeProjectPath {
        /// Rejected path.
        path: PathBuf,
    },

    /// A project path does not exist or cannot be resolved.
    #[error("cannot access project directory `{}`", path.display())]
    ProjectPath {
        /// Requested path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A project path is not a directory.
    #[error("project path `{}` is not a directory", path.display())]
    ProjectNotDirectory {
        /// Resolved path.
        path: PathBuf,
    },

    /// A project path is not valid Unicode and cannot be stored.
    #[error("project path `{}` is not valid UTF-8", path.display())]
    NonUnicodeProjectPath {
        /// Resolved path.
        path: PathBuf,
    },

    /// A project manifest exists but could not be read.
    #[error("failed to read project manifest `{}`", path.display())]
    ManifestRead {
        /// Manifest file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A project manifest exceeds the size limit.
    #[error("project manifest `{}` exceeds the limit of {max} bytes", path.display())]
    ManifestTooLarge {
        /// Manifest file.
        path: PathBuf,
        /// Maximum accepted size.
        max: u64,
    },

    /// A project manifest is not valid TOML or does not match the schema.
    #[error("invalid project manifest `{}`", path.display())]
    ManifestParse {
        /// Manifest file.
        path: PathBuf,
        /// Parser error, including the location of the problem.
        #[source]
        source: Box<toml::de::Error>,
    },

    /// A project manifest declares an unsupported schema version.
    #[error(
        "project manifest `{}` uses schema version {found}, \
         but this version of DevSpace supports version {supported}",
        path.display()
    )]
    UnsupportedManifestVersion {
        /// Manifest file.
        path: PathBuf,
        /// Version found in the file.
        found: u32,
        /// Version supported by this build.
        supported: u32,
    },

    /// A project manifest holds conflicting settings.
    #[error("invalid project manifest `{}`: {reason}", path.display())]
    InvalidManifest {
        /// Manifest file.
        path: PathBuf,
        /// Broken rule.
        reason: &'static str,
    },

    /// Another project already uses this name.
    #[error("a project named `{name}` is already registered (at `{}`)", path.display())]
    ProjectNameTaken {
        /// Requested name.
        name: String,
        /// Directory of the existing project.
        path: PathBuf,
    },

    /// This directory is already registered.
    #[error("directory `{}` is already registered as project `{name}`", path.display())]
    ProjectPathTaken {
        /// Requested directory.
        path: PathBuf,
        /// Name of the existing project.
        name: String,
    },

    /// No project has this name.
    #[error("no project named `{name}` is registered")]
    ProjectNotFound {
        /// Requested name.
        name: String,
    },

    /// Caddy publishes no build for this platform.
    #[error("no Caddy build is available for this platform")]
    CaddyUnavailable,

    /// A download could not be started or was refused by the server.
    #[error("failed to download `{url}`")]
    Download {
        /// Requested URL.
        url: String,
        /// HTTP client error.
        #[source]
        source: Box<ureq::Error>,
    },

    /// A download was interrupted or exceeded its size limit.
    #[error("failed to receive `{url}`")]
    DownloadRead {
        /// Requested URL.
        url: String,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A downloaded file does not have the expected checksum; it was not
    /// used.
    #[error(
        "checksum mismatch for `{file}`: expected SHA-256 {expected}, got {actual}; \
         the file was discarded"
    )]
    ChecksumMismatch {
        /// Downloaded file name.
        file: String,
        /// Embedded checksum.
        expected: String,
        /// Checksum of the received data.
        actual: String,
    },

    /// A file could not be extracted from a downloaded archive.
    #[error("failed to extract from `{}`", path.display())]
    Archive {
        /// Archive file.
        path: PathBuf,
        /// Extraction error.
        #[source]
        source: crate::archive::ArchiveError,
    },

    /// A file or directory of an installation could not be created.
    #[error("failed to install `{}`", path.display())]
    InstallFile {
        /// Path being created.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The installed Caddy does not report the expected version.
    #[error("`{}` is not the expected Caddy build: {detail}", path.display())]
    CaddyVersion {
        /// Caddy executable.
        path: PathBuf,
        /// What went wrong.
        detail: String,
    },

    /// The signature of the package index does not match the DevSpace
    /// public key; the index was not used.
    #[error("the package index signature is invalid: {0}")]
    IndexSignature(String),

    /// The package index is not valid JSON or does not match its schema.
    #[error("invalid package index")]
    IndexParse(#[source] Box<serde_json::Error>),

    /// The package index holds inconsistent values.
    #[error("invalid package index: {0}")]
    InvalidIndex(String),

    /// The package index has no matching package for this platform.
    #[error(
        "no {name} {requirement} package is available for this platform{}",
        if available.is_empty() { String::new() } else { format!(" (available: {available})") }
    )]
    RuntimeUnavailable {
        /// Runtime.
        name: crate::runtime::RuntimeName,
        /// Requested version or requirement.
        requirement: String,
        /// Versions published for this platform, comma-separated.
        available: String,
    },

    /// An installed runtime does not behave as expected; it was not
    /// installed.
    #[error("{name} {version} failed its installation check: {detail}")]
    RuntimeCheck {
        /// Runtime.
        name: crate::runtime::RuntimeName,
        /// Version being installed.
        version: crate::runtime::Version,
        /// What went wrong.
        detail: String,
    },

    /// The runtime version is not installed.
    #[error("{name} {version} is not installed")]
    RuntimeNotInstalled {
        /// Runtime.
        name: crate::runtime::RuntimeName,
        /// Version.
        version: crate::runtime::Version,
    },

    /// The data directory of a service instance could not be created.
    #[error("{instance} cannot be initialised: {detail}")]
    ServiceInit {
        /// Service and series.
        instance: String,
        /// What went wrong.
        detail: String,
    },

    /// A path cannot be used in a generated configuration.
    #[error("unsupported path `{}`: {reason}", path.display())]
    UnsupportedPath {
        /// Path.
        path: PathBuf,
        /// Why.
        reason: &'static str,
    },

    /// The local certificate authority could not be read.
    #[error("failed to read the local certificate authority `{}`", path.display())]
    CaRead {
        /// Root certificate file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The local certificate authority file is not a certificate.
    #[error("invalid local certificate authority `{}`", path.display())]
    CaInvalid {
        /// Root certificate file.
        path: PathBuf,
        /// Parse error.
        #[source]
        source: devspace_helper::CertificateError,
    },
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
