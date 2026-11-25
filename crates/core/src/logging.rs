//! Initialisation of the global `tracing` subscriber.
//!
//! Two outputs can be combined:
//! - **stderr**, so that command output on `stdout` stays machine-readable;
//! - **rolling files** in the log directory (used by the daemon), rotated
//!   daily and pruned to the [`LOG_FILE_RETENTION`] most recent files.
//!
//! The `DEVSPACE_LOG` environment variable, using the [`EnvFilter`] syntax
//! (e.g. `debug` or `devspace_core=trace,info`), overrides the configured
//! level for every output.

use std::io::IsTerminal as _;
use std::path::PathBuf;

use tracing::Subscriber;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::{LogFormat, LoggingConfig};
use crate::{Error, Result};

/// Environment variable overriding the configured log filter.
pub const LOG_ENV_VAR: &str = "DEVSPACE_LOG";

/// Number of daily log files kept per prefix.
pub const LOG_FILE_RETENTION: usize = 7;

/// Extension of log files.
pub const LOG_FILE_SUFFIX: &str = "log";

/// Where log events are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTargets {
    /// Write to stderr (ANSI colours only when stderr is a terminal).
    pub stderr: bool,
    /// Also write to rolling files.
    pub file: Option<LogFile>,
}

impl LogTargets {
    /// Stderr only, the setting used by the CLI.
    #[must_use]
    pub const fn stderr_only() -> Self {
        Self {
            stderr: true,
            file: None,
        }
    }
}

/// Rolling log files `<dir>/<prefix>.<YYYY-MM-DD>.log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    /// Directory holding the files; it must exist.
    pub dir: PathBuf,
    /// File name prefix, e.g. `devspaced`.
    pub prefix: String,
}

/// Installs the global `tracing` subscriber.
///
/// # Errors
///
/// Returns [`Error::InvalidLogFilter`] if `DEVSPACE_LOG` is malformed,
/// [`Error::LogFile`] if the log files cannot be opened, or
/// [`Error::LoggingAlreadyInitialised`] if called more than once.
pub fn init(config: &LoggingConfig, targets: &LogTargets) -> Result<()> {
    let subscriber = build_subscriber(config, targets, std::env::var(LOG_ENV_VAR).ok().as_deref())?;
    tracing::subscriber::set_global_default(subscriber).map_err(Error::LoggingAlreadyInitialised)
}

fn build_subscriber(
    config: &LoggingConfig,
    targets: &LogTargets,
    env_override: Option<&str>,
) -> Result<impl Subscriber + Send + Sync + 'static> {
    let filter = build_filter(config, env_override)?;
    let stderr = targets.stderr.then(|| {
        fmt_layer(
            config.format,
            std::io::stderr,
            std::io::stderr().is_terminal(),
        )
    });
    let file = match &targets.file {
        Some(file) => Some(fmt_layer(config.format, file_appender(file)?, false)),
        None => None,
    };
    Ok(tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file))
}

fn file_appender(file: &LogFile) -> Result<RollingFileAppender> {
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(&file.prefix)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(LOG_FILE_RETENTION)
        .build(&file.dir)
        .map_err(|source| Error::LogFile {
            dir: file.dir.clone(),
            source,
        })
}

type BoxedLayer<S> = Box<dyn Layer<S> + Send + Sync + 'static>;

fn fmt_layer<S, W>(format: LogFormat, writer: W, ansi: bool) -> BoxedLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(ansi);
    match format {
        LogFormat::Compact => layer.compact().boxed(),
        LogFormat::Pretty => layer.pretty().boxed(),
    }
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

    #[test]
    fn file_target_writes_filtered_events() {
        let tmp = tempfile::tempdir().unwrap();
        let targets = LogTargets {
            stderr: false,
            file: Some(LogFile {
                dir: tmp.path().to_path_buf(),
                prefix: "devspaced".to_owned(),
            }),
        };
        let subscriber = build_subscriber(&config(LogLevel::Info), &targets, None).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(answer = 42, "kept event");
            tracing::debug!("filtered event");
        });

        let files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("devspaced.") && name.ends_with(".log"),
            "{name}"
        );
        let content = std::fs::read_to_string(&files[0]).unwrap();
        assert!(content.contains("kept event") && content.contains("answer=42"));
        assert!(!content.contains("filtered event"));
        // No ANSI escape codes in files.
        assert!(!content.contains('\u{1b}'));
    }

    #[test]
    fn missing_log_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        // A regular file where the directory should be.
        let not_a_dir = tmp.path().join("file");
        std::fs::write(&not_a_dir, "").unwrap();
        let targets = LogTargets {
            stderr: false,
            file: Some(LogFile {
                dir: not_a_dir,
                prefix: "devspaced".to_owned(),
            }),
        };
        assert!(matches!(
            build_subscriber(&config(LogLevel::Info), &targets, None),
            Err(Error::LogFile { .. })
        ));
    }
}
