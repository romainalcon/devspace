//! Initialisation of the global `tracing` subscriber.
//!
//! Logs are written to `stderr` so that command output on `stdout` stays
//! machine-readable. The `DEVSPACE_LOG` environment variable, using the
//! [`EnvFilter`] syntax (e.g. `debug` or `devspace_core=trace,info`),
//! overrides the configured level.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

use crate::config::{LogFormat, LoggingConfig};
use crate::{Error, Result};

/// Environment variable overriding the configured log filter.
pub const LOG_ENV_VAR: &str = "DEVSPACE_LOG";

/// Installs the global `tracing` subscriber.
///
/// # Errors
///
/// Returns [`Error::InvalidLogFilter`] if `DEVSPACE_LOG` is malformed, or
/// [`Error::LoggingAlreadyInitialised`] if called more than once.
pub fn init(config: &LoggingConfig) -> Result<()> {
    let filter = build_filter(config, std::env::var(LOG_ENV_VAR).ok().as_deref())?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()));
    let result = match config.format {
        LogFormat::Compact => tracing::subscriber::set_global_default(builder.compact().finish()),
        LogFormat::Pretty => tracing::subscriber::set_global_default(builder.pretty().finish()),
    };
    result.map_err(Error::LoggingAlreadyInitialised)
}

/// Builds the event filter from the configuration and the optional
/// `DEVSPACE_LOG` override.
fn build_filter(config: &LoggingConfig, env_override: Option<&str>) -> Result<EnvFilter> {
    match env_override.map(str::trim).filter(|v| !v.is_empty()) {
        Some(directives) => {
            EnvFilter::try_new(directives).map_err(|source| Error::InvalidLogFilter {
                variable: LOG_ENV_VAR,
                source,
            })
        }
        None => Ok(EnvFilter::default()
            .add_directive(LevelFilter::from_level(config.level.as_tracing()).into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogLevel;

    fn config(level: LogLevel) -> LoggingConfig {
        LoggingConfig {
            level,
            ..LoggingConfig::default()
        }
    }

    #[test]
    fn configured_level_is_used_without_override() {
        let filter = build_filter(&config(LogLevel::Debug), None).unwrap();
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::DEBUG));
        let filter = build_filter(&config(LogLevel::Warn), Some("  ")).unwrap();
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::WARN));
    }

    #[test]
    fn env_override_wins() {
        let filter = build_filter(&config(LogLevel::Error), Some("trace")).unwrap();
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::TRACE));
    }

    #[test]
    fn invalid_override_is_rejected() {
        let err = build_filter(&config(LogLevel::Info), Some("devspace=loud")).unwrap_err();
        assert!(matches!(err, Error::InvalidLogFilter { .. }));
    }
}
