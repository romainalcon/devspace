//! Configuration of the PHP FastCGI process manager (PHP-FPM).
//!
//! The daemon runs one PHP-FPM master per PHP version used by a site, as
//! the current user, in the foreground. It listens on a Unix socket of the
//! DevSpace runtime directory (mode `0600`, so only the user's own web
//! server can reach it) and starts workers on demand. Both files are
//! regenerated at every start:
//!
//! - `<data>/php/<version>/php-fpm.conf`: the master and its single pool;
//! - `<data>/php/<version>/php.ini`: development defaults, also used by the
//!   command-line PHP run through `devspace exec` (`PHPRC`).
//!
//! PHP-FPM does not exist on Windows; PHP sites are not served there yet.

use std::path::{Path, PathBuf};

use super::{RuntimeLayout, Version};
use crate::{Error, Result};

/// Longest Unix socket path accepted by every supported system
/// (`sun_path` is 104 bytes on macOS, 108 on Linux).
const MAX_SOCKET_PATH_LEN: usize = 100;

/// Files of the PHP-FPM master of one PHP version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FpmPaths {
    /// PHP version.
    pub version: Version,
    /// Generated PHP-FPM configuration.
    pub config_file: PathBuf,
    /// Directory holding the generated `php.ini` (value of `PHPRC`).
    pub ini_dir: PathBuf,
    /// Generated `php.ini`.
    pub ini_file: PathBuf,
    /// FastCGI socket.
    pub socket: PathBuf,
    /// Process id of the master, to stop it after a daemon crash.
    pub pid_file: PathBuf,
    /// Log of the master and of the workers' output.
    pub log_file: PathBuf,
}

impl RuntimeLayout {
    /// Files of the PHP-FPM master of `version`.
    #[must_use]
    pub fn fpm(&self, version: Version) -> FpmPaths {
        let ini_dir = self.php_config_dir.join(version.to_string());
        FpmPaths {
            version,
            config_file: ini_dir.join("php-fpm.conf"),
            ini_file: ini_dir.join("php.ini"),
            ini_dir,
            socket: self.php_run_dir.join(format!("{version}.sock")),
            pid_file: self.php_run_dir.join(format!("{version}.pid")),
            log_file: self.log_dir.join(format!("php-{version}-fpm.log")),
        }
    }
}

impl FpmPaths {
    /// Arguments of `php-fpm`: foreground, this configuration and `php.ini`.
    #[must_use]
    pub fn arguments(&self) -> Vec<&Path> {
        vec![
            Path::new("--nodaemonize"),
            Path::new("--fpm-config"),
            &self.config_file,
            Path::new("-c"),
            &self.ini_file,
        ]
    }

    /// The PHP-FPM configuration.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPath`] if a path cannot be written in the
    /// configuration or the socket path is too long.
    pub fn render_config(&self) -> Result<String> {
        let socket = quoted(&self.socket)?;
        if self.socket.as_os_str().len() > MAX_SOCKET_PATH_LEN {
            return Err(Error::UnsupportedPath {
                path: self.socket.clone(),
                reason: "the socket path is too long; use a shorter DEVSPACE_HOME",
            });
        }
        let log = quoted(&self.log_file)?;
        Ok(format!(
            "; Managed by DevSpace: regenerated at every start, do not edit.\n\
             [global]\n\
             error_log = {log}\n\
             log_level = notice\n\
             daemonize = no\n\
             \n\
             [devspace]\n\
             listen = {socket}\n\
             listen.mode = 0600\n\
             pm = ondemand\n\
             pm.max_children = 10\n\
             pm.process_idle_timeout = 30s\n\
             pm.max_requests = 500\n\
             catch_workers_output = yes\n\
             decorate_workers_output = no\n\
             security.limit_extensions = .php\n"
        ))
    }
}

/// The `php.ini` of every version: development settings.
pub const PHP_INI: &str = "; Managed by DevSpace: regenerated at every start, do not edit.\n\
         memory_limit = 512M\n\
         max_execution_time = 60\n\
         upload_max_filesize = 128M\n\
         post_max_size = 128M\n\
         error_reporting = E_ALL\n\
         display_errors = On\n\
         display_startup_errors = On\n\
         log_errors = On\n\
         date.timezone = UTC\n\
         expose_php = Off\n";

/// A path as a double-quoted ini string. Quotes, `$` (variable
/// interpolation), line breaks and non-UTF-8 paths are refused.
fn quoted(path: &Path) -> Result<String> {
    let unsupported = |reason| Error::UnsupportedPath {
        path: path.to_path_buf(),
        reason,
    };
    let text = path
        .to_str()
        .ok_or_else(|| unsupported("the path is not valid UTF-8"))?;
    if text.contains(['"', '$', '\n', '\r', '\0']) {
        return Err(unsupported(
            "the path contains a character that cannot be written in a PHP configuration",
        ));
    }
    Ok(format!("\"{text}\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppPaths;

    fn fpm(root: &str) -> FpmPaths {
        RuntimeLayout::new(&AppPaths::from_root(Path::new(root)))
            .fpm(Version::parse("8.4.23").unwrap())
    }

    #[test]
    fn configuration_listens_on_a_private_socket() {
        let fpm = fpm("/Users/me/Library/Application Support/DevSpace");
        let config = fpm.render_config().unwrap();
        assert!(
            config.contains(
                "listen = \"/Users/me/Library/Application Support/DevSpace/run/php/8.4.23.sock\"\n"
            ),
            "{config}"
        );
        assert!(config.contains("listen.mode = 0600\n"));
        assert!(config.contains("daemonize = no\n"));
        assert!(
            !config.contains("listen.owner"),
            "no ownership change needed"
        );
        assert!(fpm.log_file.ends_with("logs/php-8.4.23-fpm.log"));
        assert_eq!(fpm.arguments()[0], Path::new("--nodaemonize"));
    }

    #[test]
    fn unsafe_or_long_paths_are_refused() {
        for root in ["/tmp/a\"b", "/tmp/$HOME", "/tmp/a\nb"] {
            assert!(
                matches!(
                    fpm(root).render_config(),
                    Err(Error::UnsupportedPath { .. })
                ),
                "{root:?}"
            );
        }
        let long = format!("/{}", "x".repeat(90));
        assert!(matches!(
            fpm(&long).render_config(),
            Err(Error::UnsupportedPath { .. })
        ));
    }
}
