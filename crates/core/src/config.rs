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
//!
//! [web]
//! tld = "internal"       # sites at https://<project>.internal
//! address = "127.0.0.2"  # loopback address of the sites
//! http_port = 47080      # web server ports on that address
//! https_port = 47443
//! dns_port = 47053       # DNS server (127.0.0.1, UDP) answering for *.<tld>
//!
//! [services]             # services listen on web.address
//! mysql_port = 3306
//! postgresql_port = 5432
//! ```

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use devspace_helper::{LoopbackAddress, Tld};
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
    /// Web server and local domains.
    pub web: WebConfig,
    /// Services (databases).
    pub services: ServicesConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            logging: LoggingConfig::default(),
            web: WebConfig::default(),
            services: ServicesConfig::default(),
        }
    }
}

/// Web server and local domain settings.
///
/// Sites are served at `https://<project>.<tld>` on `address`, a loopback
/// address. The defaults (`internal`, `127.0.0.2`) do not collide with
/// other tools using `*.test` on `127.0.0.1` (Laravel Herd, Valet).
///
/// Every port must be unprivileged (at least 1024): the web server and the
/// DNS server never run elevated. Ports 80 and 443 of `address` reach the
/// web server through the port forwarding installed by
/// `devspace system setup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebConfig {
    /// Top-level domain of the sites.
    pub tld: Tld,
    /// Loopback address of the sites.
    pub address: LoopbackAddress,
    /// HTTP port of the web server (redirects to HTTPS).
    pub http_port: u16,
    /// HTTPS port of the web server.
    pub https_port: u16,
    /// UDP port of the DNS server answering for the local domains.
    pub dns_port: u16,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            tld: Tld::default(),
            address: LoopbackAddress::default(),
            http_port: 47080,
            https_port: 47443,
            dns_port: 47053,
        }
    }
}

impl WebConfig {
    /// Checks that the ports are unprivileged and that the two web server
    /// ports differ.
    fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.http_port < 1024 || self.https_port < 1024 || self.dns_port < 1024 {
            return Err("web ports must be at least 1024 (DevSpace never runs privileged)");
        }
        if self.http_port == self.https_port {
            return Err("web.http_port and web.https_port must differ");
        }
        Ok(())
    }
}

/// Service settings.
///
/// The services listen on the loopback address of the sites
/// ([`WebConfig::address`]), on their usual ports by default: other tools
/// using the same ports on `127.0.0.1` (Laravel Herd) are not disturbed.
/// One instance of a service can use its port at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesConfig {
    /// TCP port of MySQL.
    pub mysql_port: u16,
    /// TCP port of PostgreSQL.
    pub postgresql_port: u16,
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            mysql_port: 3306,
            postgresql_port: 5432,
        }
    }
}

impl ServicesConfig {
    /// Checks that the ports are unprivileged, distinct, and not used by
    /// the web server on the same address.
    fn validate(&self, web: &WebConfig) -> std::result::Result<(), &'static str> {
        if self.mysql_port < 1024 || self.postgresql_port < 1024 {
            return Err("service ports must be at least 1024 (DevSpace never runs privileged)");
        }
        if self.mysql_port == self.postgresql_port {
            return Err("services.mysql_port and services.postgresql_port must differ");
        }
        if [self.mysql_port, self.postgresql_port]
            .iter()
            .any(|port| *port == web.http_port || *port == web.https_port)
        {
            return Err("service ports must differ from the web server ports");
        }
        Ok(())
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
    /// Returns [`Error::ConfigParse`] for invalid TOML or schema mismatch,
    /// [`Error::UnsupportedConfigVersion`] for an unknown `schema_version`,
    /// and [`Error::InvalidConfig`] for inconsistent values.
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
        config
            .web
            .validate()
            .and_then(|()| config.services.validate(&config.web))
            .map_err(|reason| Error::InvalidConfig {
                path: origin.to_path_buf(),
                reason,
            })?;
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
    fn web_ports_are_validated() {
        let config = GlobalConfig::from_toml("[web]\nhttps_port = 8443\n", origin()).unwrap();
        assert_eq!(config.web.https_port, 8443);
        assert_eq!(config.web.http_port, WebConfig::default().http_port);
        for invalid in [
            "[web]\nhttp_port = 80\n",
            "[web]\ndns_port = 53\n",
            "[web]\nhttp_port = 9000\nhttps_port = 9000\n",
        ] {
            assert!(
                matches!(
                    GlobalConfig::from_toml(invalid, origin()),
                    Err(Error::InvalidConfig { .. })
                ),
                "{invalid}"
            );
        }
        assert!(GlobalConfig::from_toml("[web]\nhttp_port = 70000\n", origin()).is_err());

        let config =
            GlobalConfig::from_toml("[web]\ntld = \"test\"\naddress = \"127.0.0.1\"\n", origin())
                .unwrap();
        assert_eq!(config.web.tld.as_str(), "test");
        assert!(config.web.address.is_primary());
        assert_eq!(GlobalConfig::default().web.tld.as_str(), "internal");
        assert_eq!(GlobalConfig::default().web.address.to_string(), "127.0.0.2");
        for invalid in [
            "[web]\ntld = \"local\"\n",
            "[web]\ntld = \"Test\"\n",
            "[web]\naddress = \"192.168.1.2\"\n",
            "[web]\naddress = \"::1\"\n",
        ] {
            assert!(
                matches!(
                    GlobalConfig::from_toml(invalid, origin()),
                    Err(Error::ConfigParse { .. })
                ),
                "{invalid}"
            );
        }
    }

    #[test]
    fn service_ports_are_validated() {
        let defaults = GlobalConfig::default().services;
        assert_eq!(
            (defaults.mysql_port, defaults.postgresql_port),
            (3306, 5432)
        );
        let config =
            GlobalConfig::from_toml("[services]\npostgresql_port = 5433\n", origin()).unwrap();
        assert_eq!(config.services.postgresql_port, 5433);
        for invalid in [
            "[services]\nmysql_port = 1000\n",
            "[services]\nmysql_port = 5432\n",
            "[services]\npostgresql_port = 47443\n",
            "[web]\nhttp_port = 3306\n",
        ] {
            assert!(
                matches!(
                    GlobalConfig::from_toml(invalid, origin()),
                    Err(Error::InvalidConfig { .. })
                ),
                "{invalid}"
            );
        }
        assert!(GlobalConfig::from_toml("[services]\nredis_port = 6379\n", origin()).is_err());
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
