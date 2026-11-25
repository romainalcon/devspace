//! Typed global configuration stored in `config.toml`.
//!
//! Every field has a default, so a missing file or a partial file is valid.
//! Unknown keys are rejected to surface typos early.
//!
//! ```toml
//! schema_version = 1
//!
//! [logging]
//! level = "info"     # error | warn | info | debug | trace
//! format = "compact" # compact | pretty
//! ```

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Schema version supported by this build.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Global DevSpace configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalConfig {
    /// Version of the configuration schema, used for future migrations.
    pub schema_version: u32,
    /// Logging settings.
    pub logging: LoggingConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            logging: LoggingConfig::default(),
        }
    }
}

/// Logging settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// Minimum level of emitted events. Overridden by `DEVSPACE_LOG`.
    pub level: LogLevel,
    /// Output format.
    pub format: LogFormat,
}

/// Minimum severity of logged events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Informational messages (default).
    #[default]
    Info,
    /// Debugging details.
    Debug,
    /// Very verbose tracing.
    Trace,
}

impl LogLevel {
    /// Equivalent `tracing` level.
    #[must_use]
    pub const fn as_tracing(self) -> tracing::Level {
        match self {
            Self::Error => tracing::Level::ERROR,
            Self::Warn => tracing::Level::WARN,
            Self::Info => tracing::Level::INFO,
            Self::Debug => tracing::Level::DEBUG,
            Self::Trace => tracing::Level::TRACE,
        }
    }
}

/// Text format of log output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Single line per event with structured `key=value` fields (default).
    #[default]
    Compact,
    /// Multi-line, human-oriented output.
    Pretty,
}

impl GlobalConfig {
    /// Parses a configuration from TOML text. `origin` is only used in errors.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigParse`] for invalid TOML or schema mismatch, and
    /// [`Error::UnsupportedConfigVersion`] for an unknown `schema_version`.
    pub fn from_toml(text: &str, origin: &Path) -> Result<Self> {
        let config: Self = toml::from_str(text).map_err(|source| Error::ConfigParse {
            path: origin.to_path_buf(),
            source: Box::new(source),
        })?;
        if config.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(Error::UnsupportedConfigVersion {
                path: origin.to_path_buf(),
                found: config.schema_version,
                supported: CONFIG_SCHEMA_VERSION,
            });
        }
        Ok(config)
    }

    /// Serializes the configuration to TOML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigSerialize`] if serialization fails.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(Error::ConfigSerialize)
    }

    /// Loads the configuration from `path`, or returns the defaults when the
    /// file does not exist.
    ///
    /// # Errors
    ///
    /// Fails if the file exists but cannot be read or parsed.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text, path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::ConfigRead {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Writes the configuration to a new file at `path`.
    ///
    /// The parent directory must exist. An existing file is never
    /// overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigAlreadyExists`] if the file exists, or
    /// [`Error::ConfigWrite`] on I/O failure.
    pub fn write_new(&self, path: &Path) -> Result<()> {
        let text = self.to_toml()?;
        let write_err = |source| Error::ConfigWrite {
            path: path.to_path_buf(),
            source,
        };
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|err| {
                if err.kind() == io::ErrorKind::AlreadyExists {
                    Error::ConfigAlreadyExists {
                        path: path.to_path_buf(),
                    }
                } else {
                    write_err(err)
                }
            })?;
        file.write_all(text.as_bytes()).map_err(write_err)?;
        file.sync_all().map_err(write_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> &'static Path {
        Path::new("test.toml")
    }

    #[test]
    fn empty_file_yields_defaults() {
        let config = GlobalConfig::from_toml("", origin()).unwrap();
        assert_eq!(config, GlobalConfig::default());
        assert_eq!(config.logging.level, LogLevel::Info);
        assert_eq!(config.logging.format, LogFormat::Compact);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config = GlobalConfig::from_toml("[logging]\nlevel = \"debug\"\n", origin()).unwrap();
        assert_eq!(config.logging.level, LogLevel::Debug);
        assert_eq!(config.logging.format, LogFormat::Compact);
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn roundtrip() {
        let config = GlobalConfig {
            logging: LoggingConfig {
                level: LogLevel::Trace,
                format: LogFormat::Pretty,
            },
            ..GlobalConfig::default()
        };
        let text = config.to_toml().unwrap();
        assert_eq!(GlobalConfig::from_toml(&text, origin()).unwrap(), config);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = GlobalConfig::from_toml("[logging]\nlevle = \"debug\"\n", origin()).unwrap_err();
        assert!(matches!(err, Error::ConfigParse { .. }));
    }

    #[test]
    fn invalid_level_is_rejected() {
        let err = GlobalConfig::from_toml("[logging]\nlevel = \"loud\"\n", origin()).unwrap_err();
        assert!(matches!(err, Error::ConfigParse { .. }));
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let err = GlobalConfig::from_toml("schema_version = 99\n", origin()).unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedConfigVersion {
                found: 99,
                supported: CONFIG_SCHEMA_VERSION,
                ..
            }
        ));
    }

    #[test]
    fn missing_file_yields_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let config = GlobalConfig::load_or_default(&tmp.path().join("absent.toml")).unwrap();
        assert_eq!(config, GlobalConfig::default());
    }

    #[test]
    fn write_new_then_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let config = GlobalConfig::default();
        config.write_new(&path).unwrap();
        assert_eq!(GlobalConfig::load_or_default(&path).unwrap(), config);
    }

    #[test]
    fn write_new_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "# user content\n").unwrap();
        let err = GlobalConfig::default().write_new(&path).unwrap_err();
        assert!(matches!(err, Error::ConfigAlreadyExists { .. }));
        assert_eq!(fs::read_to_string(&path).unwrap(), "# user content\n");
    }

    #[test]
    fn unreadable_path_is_a_read_error() {
        // A directory cannot be read as a file.
        let tmp = tempfile::tempdir().unwrap();
        let err = GlobalConfig::load_or_default(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigRead { .. }));
    }
}
