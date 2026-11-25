//! Resolution of the DevSpace application directories.
//!
//! Default layout per platform:
//!
//! | Kind   | macOS                                         | Linux                                   | Windows                              |
//! |--------|-----------------------------------------------|-----------------------------------------|--------------------------------------|
//! | config | `~/Library/Application Support/DevSpace/config` | `$XDG_CONFIG_HOME/devspace`           | `%APPDATA%\DevSpace\config`          |
//! | data   | `~/Library/Application Support/DevSpace/data`   | `$XDG_DATA_HOME/devspace`             | `%LOCALAPPDATA%\DevSpace\data`       |
//! | cache  | `~/Library/Caches/DevSpace`                     | `$XDG_CACHE_HOME/devspace`            | `%LOCALAPPDATA%\DevSpace\cache`      |
//! | logs   | `~/Library/Logs/DevSpace`                       | `$XDG_STATE_HOME/devspace/logs`       | `%LOCALAPPDATA%\DevSpace\logs`       |
//! | run    | `~/Library/Application Support/DevSpace/run`    | `$XDG_RUNTIME_DIR/devspace`           | `%LOCALAPPDATA%\DevSpace\run`        |
//!
//! On Linux, when `XDG_RUNTIME_DIR` is unset, the runtime directory falls back
//! to `<data>/run`.
//!
//! Setting the `DEVSPACE_HOME` environment variable to an absolute path
//! replaces the whole layout with `config/`, `data/`, `cache/` and `logs/`
//! sub-directories of that path, plus `run/` (useful for tests, CI and portable setups).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::PlatformError;

/// Environment variable overriding the root of every DevSpace directory.
pub const HOME_ENV_VAR: &str = "DEVSPACE_HOME";

/// File name of the global configuration inside [`AppPaths::config_dir`].
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// File name of the SQLite database inside [`AppPaths::data_dir`].
pub const DATABASE_FILE_NAME: &str = "devspace.db";

/// File name of the daemon single-instance lock inside [`AppPaths::runtime_dir`].
pub const DAEMON_LOCK_FILE_NAME: &str = "devspaced.lock";

/// Directories used by DevSpace for the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// User configuration (global `config.toml`).
    pub config_dir: PathBuf,
    /// Persistent data (database, installed runtimes, project state).
    pub data_dir: PathBuf,
    /// Re-creatable data (downloads, temporary artifacts).
    pub cache_dir: PathBuf,
    /// Log files.
    pub log_dir: PathBuf,
    /// Private runtime state of the daemon (IPC socket, instance lock).
    ///
    /// Always restricted to the current user; see [`AppPaths::ensure_exists`].
    pub runtime_dir: PathBuf,
}

/// Target operating system family, used to select the directory layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsFamily {
    /// Apple macOS.
    MacOs,
    /// Microsoft Windows.
    Windows,
    /// Linux and other XDG-based Unix systems.
    Linux,
}

impl OsFamily {
    /// Family of the operating system the binary was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Base directories provided by the operating system for the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseLocations {
    /// Roaming configuration base (`~/Library/Application Support`, `$XDG_CONFIG_HOME`, `%APPDATA%`).
    pub config: PathBuf,
    /// Data base (`~/Library/Application Support`, `$XDG_DATA_HOME`, `%APPDATA%`).
    pub data: PathBuf,
    /// Machine-local data base (`%LOCALAPPDATA%` on Windows, same as `data` elsewhere).
    pub data_local: PathBuf,
    /// Cache base (`~/Library/Caches`, `$XDG_CACHE_HOME`, `%LOCALAPPDATA%`).
    pub cache: PathBuf,
    /// State base (`$XDG_STATE_HOME` on Linux, unavailable elsewhere).
    pub state: Option<PathBuf>,
    /// Runtime base (`$XDG_RUNTIME_DIR` on Linux, unavailable elsewhere).
    pub runtime: Option<PathBuf>,
    /// User home directory.
    pub home: PathBuf,
}

impl BaseLocations {
    /// Queries the operating system for the base directories of the current user.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::HomeDirectoryNotFound`] if the home directory
    /// cannot be determined.
    pub fn detect() -> Result<Self, PlatformError> {
        let base = directories::BaseDirs::new().ok_or(PlatformError::HomeDirectoryNotFound)?;
        Ok(Self {
            config: base.config_dir().to_path_buf(),
            data: base.data_dir().to_path_buf(),
            data_local: base.data_local_dir().to_path_buf(),
            cache: base.cache_dir().to_path_buf(),
            state: base.state_dir().map(Path::to_path_buf),
            runtime: base.runtime_dir().map(Path::to_path_buf),
            home: base.home_dir().to_path_buf(),
        })
    }
}

impl AppPaths {
    /// Resolves the directories for the current user and platform, honouring
    /// the [`HOME_ENV_VAR`] override.
    ///
    /// No directory is created; call [`AppPaths::ensure_exists`] for that.
    ///
    /// # Errors
    ///
    /// Fails if the override is relative or if the OS base directories cannot
    /// be determined.
    pub fn resolve() -> Result<Self, PlatformError> {
        match home_override(std::env::var_os(HOME_ENV_VAR))? {
            Some(root) => Ok(Self::from_root(&root)),
            None => Ok(Self::from_base(
                OsFamily::current(),
                &BaseLocations::detect()?,
            )),
        }
    }

    /// Builds a self-contained layout under a single root directory.
    #[must_use]
    pub fn from_root(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            log_dir: root.join("logs"),
            runtime_dir: root.join("run"),
        }
    }

    /// Builds the conventional layout of `os` from its base directories.
    #[must_use]
    pub fn from_base(os: OsFamily, base: &BaseLocations) -> Self {
        match os {
            OsFamily::MacOs => {
                let app_support = base.data.join("DevSpace");
                Self {
                    config_dir: app_support.join("config"),
                    data_dir: app_support.join("data"),
                    cache_dir: base.cache.join("DevSpace"),
                    log_dir: base.home.join("Library").join("Logs").join("DevSpace"),
                    runtime_dir: app_support.join("run"),
                }
            }
            OsFamily::Windows => {
                let local = base.data_local.join("DevSpace");
                Self {
                    config_dir: base.config.join("DevSpace").join("config"),
                    data_dir: local.join("data"),
                    cache_dir: local.join("cache"),
                    log_dir: local.join("logs"),
                    runtime_dir: local.join("run"),
                }
            }
            OsFamily::Linux => {
                let data_dir = base.data.join("devspace");
                let log_dir = match &base.state {
                    Some(state) => state.join("devspace").join("logs"),
                    None => data_dir.join("logs"),
                };
                let runtime_dir = match &base.runtime {
                    Some(runtime) => runtime.join("devspace"),
                    None => data_dir.join("run"),
                };
                Self {
                    config_dir: base.config.join("devspace"),
                    data_dir,
                    cache_dir: base.cache.join("devspace"),
                    log_dir,
                    runtime_dir,
                }
            }
        }
    }

    /// Path of the global configuration file.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE_NAME)
    }

    /// Path of the SQLite database owned by the daemon.
    #[must_use]
    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join(DATABASE_FILE_NAME)
    }

    /// Path of the daemon single-instance lock file.
    #[must_use]
    pub fn daemon_lock_file(&self) -> PathBuf {
        self.runtime_dir.join(DAEMON_LOCK_FILE_NAME)
    }

    /// Iterates over every managed directory.
    pub fn all(&self) -> [&Path; 5] {
        [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
            &self.runtime_dir,
        ]
    }

    /// Creates every directory that does not exist yet, then checks that the
    /// runtime directory is private.
    ///
    /// On Unix, newly created directories are restricted to the current user
    /// (`0700`). Existing directories are left untouched, except that the
    /// runtime directory — which protects the IPC socket — must be owned by
    /// the current user and inaccessible to group and others. On Windows the
    /// directories inherit the ACL of the user profile and the IPC pipe is
    /// protected by its own ACL.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::CreateDirectory`] for the first directory that
    /// could not be created, or [`PlatformError::InsecureDirectory`] if the
    /// runtime directory is not private.
    pub fn ensure_exists(&self) -> Result<(), PlatformError> {
        for dir in self.all() {
            create_private_dir(dir)?;
        }
        verify_private_dir(&self.runtime_dir)
    }
}

/// Validates the raw value of the [`HOME_ENV_VAR`] override.
fn home_override(value: Option<OsString>) -> Result<Option<PathBuf>, PlatformError> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Err(PlatformError::RelativeHomeOverride {
            variable: HOME_ENV_VAR,
            value: path,
        })
    }
}

fn create_private_dir(path: &Path) -> Result<(), PlatformError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|source| PlatformError::CreateDirectory {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
fn verify_private_dir(path: &Path) -> Result<(), PlatformError> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(path).map_err(|source| PlatformError::InspectDirectory {
        path: path.to_path_buf(),
        source,
    })?;
    let expected_owner = crate::privileges::effective_uid();
    let mode = meta.mode() & 0o7777;
    if meta.uid() != expected_owner || mode & 0o077 != 0 {
        return Err(PlatformError::InsecureDirectory {
            path: path.to_path_buf(),
            owner: meta.uid(),
            expected_owner,
            mode,
        });
    }
    Ok(())
}

#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)]
fn verify_private_dir(_path: &Path) -> Result<(), PlatformError> {
    // Access control relies on the user-profile ACL and on the pipe DACL.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_base() -> BaseLocations {
        BaseLocations {
            config: PathBuf::from("/cfg"),
            data: PathBuf::from("/data"),
            data_local: PathBuf::from("/local"),
            cache: PathBuf::from("/cache"),
            state: Some(PathBuf::from("/state")),
            runtime: Some(PathBuf::from("/run/user/1000")),
            home: PathBuf::from("/home/user"),
        }
    }

    #[test]
    fn macos_layout() {
        let paths = AppPaths::from_base(OsFamily::MacOs, &fake_base());
        assert_eq!(paths.config_dir, Path::new("/data/DevSpace/config"));
        assert_eq!(paths.data_dir, Path::new("/data/DevSpace/data"));
        assert_eq!(paths.cache_dir, Path::new("/cache/DevSpace"));
        assert_eq!(paths.log_dir, Path::new("/home/user/Library/Logs/DevSpace"));
        assert_eq!(paths.runtime_dir, Path::new("/data/DevSpace/run"));
    }

    #[test]
    fn windows_layout_keeps_heavy_data_local() {
        let paths = AppPaths::from_base(OsFamily::Windows, &fake_base());
        assert_eq!(paths.config_dir, Path::new("/cfg/DevSpace/config"));
        assert_eq!(paths.data_dir, Path::new("/local/DevSpace/data"));
        assert_eq!(paths.cache_dir, Path::new("/local/DevSpace/cache"));
        assert_eq!(paths.log_dir, Path::new("/local/DevSpace/logs"));
        assert_eq!(paths.runtime_dir, Path::new("/local/DevSpace/run"));
    }

    #[test]
    fn linux_layout_uses_xdg_state_for_logs() {
        let paths = AppPaths::from_base(OsFamily::Linux, &fake_base());
        assert_eq!(paths.config_dir, Path::new("/cfg/devspace"));
        assert_eq!(paths.data_dir, Path::new("/data/devspace"));
        assert_eq!(paths.cache_dir, Path::new("/cache/devspace"));
        assert_eq!(paths.log_dir, Path::new("/state/devspace/logs"));
        assert_eq!(paths.runtime_dir, Path::new("/run/user/1000/devspace"));
    }

    #[test]
    fn linux_layout_without_state_dir_falls_back_to_data() {
        let base = BaseLocations {
            state: None,
            runtime: None,
            ..fake_base()
        };
        let paths = AppPaths::from_base(OsFamily::Linux, &base);
        assert_eq!(paths.log_dir, Path::new("/data/devspace/logs"));
        assert_eq!(paths.runtime_dir, Path::new("/data/devspace/run"));
    }

    #[test]
    fn root_layout_and_config_file() {
        let paths = AppPaths::from_root(Path::new("/opt/ds"));
        assert_eq!(paths.config_dir, Path::new("/opt/ds/config"));
        assert_eq!(paths.log_dir, Path::new("/opt/ds/logs"));
        assert_eq!(paths.config_file(), Path::new("/opt/ds/config/config.toml"));
        assert_eq!(paths.runtime_dir, Path::new("/opt/ds/run"));
        assert_eq!(paths.database_file(), Path::new("/opt/ds/data/devspace.db"));
        assert_eq!(
            paths.daemon_lock_file(),
            Path::new("/opt/ds/run/devspaced.lock")
        );
    }

    #[test]
    fn home_override_rules() {
        assert_eq!(home_override(None).unwrap(), None);
        assert_eq!(home_override(Some(OsString::new())).unwrap(), None);
        assert!(matches!(
            home_override(Some("relative/dir".into())),
            Err(PlatformError::RelativeHomeOverride { .. })
        ));
        let absolute = std::env::temp_dir();
        assert_eq!(
            home_override(Some(absolute.clone().into_os_string())).unwrap(),
            Some(absolute)
        );
    }

    #[test]
    fn ensure_exists_creates_all_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_root(&tmp.path().join("root"));
        paths.ensure_exists().unwrap();
        // Idempotent.
        paths.ensure_exists().unwrap();
        for dir in paths.all() {
            assert!(dir.is_dir(), "{} should exist", dir.display());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&paths.config_dir)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn insecure_runtime_dir_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_root(tmp.path());
        paths.ensure_exists().unwrap();
        std::fs::set_permissions(&paths.runtime_dir, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let err = paths.ensure_exists().unwrap_err();
        assert!(matches!(
            err,
            PlatformError::InsecureDirectory { mode: 0o755, .. }
        ));
    }
}
