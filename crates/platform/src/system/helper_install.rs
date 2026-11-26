//! Root-owned copy of the privileged helper.
//!
//! The helper shipped in the application bundle is writable by the user who
//! installed the application, so running it with administrator rights more
//! than once would let any program of that user gain those rights. `devspace
//! system setup` therefore first copies it (with the user's approval, after
//! checking its SHA-256) into a directory only the administrator can write,
//! and from then on only runs that copy.
//!
//! macOS: `/Library/PrivilegedHelperTools/me.romainalcon.devspace.helper`
//! (`root:wheel`, `0755`). Other platforms return
//! [`PlatformError::Unsupported`]: they have no system integration yet.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::IntegrationState;
use crate::PlatformError;
use crate::fs::{FileMode, write_atomic};

/// Directory of the privileged helpers (macOS).
const HELPER_DIR: &str = "/Library/PrivilegedHelperTools";

/// File name of the installed helper.
const HELPER_FILE: &str = "me.romainalcon.devspace.helper";

/// Largest helper DevSpace copies.
const MAX_HELPER_LEN: u64 = 64 * 1024 * 1024;

/// Path of the installed helper.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS.
pub fn installed_path() -> Result<PathBuf, PlatformError> {
    ensure_supported()?;
    Ok(Path::new(HELPER_DIR).join(HELPER_FILE))
}

/// Whether the installed helper exists, belongs to the administrator, is
/// writable by nobody else, and has the same content as `source` (the
/// helper next to the running executable). Does not need privileges.
///
/// [`IntegrationState::Outdated`] means that installing again replaces it;
/// [`IntegrationState::Conflict`] that the path holds something else than a
/// regular file.
///
/// # Errors
///
/// Those of [`installed_path`], or [`PlatformError::SystemFile`] if a file
/// cannot be read.
pub fn state(source: &Path) -> Result<IntegrationState, PlatformError> {
    state_at(&installed_path()?, source)
}

fn state_at(installed: &Path, source: &Path) -> Result<IntegrationState, PlatformError> {
    let metadata = match fs::symlink_metadata(installed) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(IntegrationState::Missing),
        Err(source) => {
            return Err(PlatformError::SystemFile {
                path: installed.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.is_file() {
        return Ok(IntegrationState::Conflict);
    }
    if !imp::is_admin_only(&metadata) {
        return Ok(IntegrationState::Outdated);
    }
    let same = read(installed)? == read(source)?;
    Ok(if same {
        IntegrationState::Installed
    } else {
        IntegrationState::Outdated
    })
}

/// Describes what [`install`] changes, for the approval prompt.
///
/// # Errors
///
/// Those of [`installed_path`].
pub fn describe_install(sha256: &str) -> Result<String, PlatformError> {
    Ok(format!(
        "install the DevSpace privileged helper (SHA-256 {sha256}) as `{}`, owned by the \
         administrator; the next changes run that copy",
        installed_path()?.display()
    ))
}

/// Describes what [`remove`] changes, for the approval prompt.
///
/// # Errors
///
/// Those of [`installed_path`].
pub fn describe_remove() -> Result<String, PlatformError> {
    Ok(format!(
        "remove the DevSpace privileged helper `{}`",
        installed_path()?.display()
    ))
}

/// Reads `source` (at most 64 MiB) for [`install`].
///
/// # Errors
///
/// [`PlatformError::SystemFile`] if it cannot be read.
pub fn read_source(source: &Path) -> Result<Vec<u8>, PlatformError> {
    read(source)
}

/// Installs `program` (the content of the helper, already checked by the
/// caller) as the installed helper, atomically. Requires administrator
/// rights: the file then belongs to the administrator.
///
/// # Errors
///
/// Those of [`installed_path`], [`PlatformError::InsecureSystemDirectory`] if the
/// helper directory is writable by others than the administrator,
/// [`PlatformError::ForeignSystemFile`] if the path is not a regular file,
/// or [`PlatformError::SystemFile`].
pub fn install(program: &[u8]) -> Result<(), PlatformError> {
    let path = installed_path()?;
    let dir = Path::new(HELPER_DIR);
    imp::ensure_admin_dir(dir)?;
    if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(PlatformError::ForeignSystemFile { path });
    }
    write_atomic(&path, program, FileMode::Executable).map_err(|source| {
        PlatformError::SystemFile {
            path: path.clone(),
            source,
        }
    })?;
    // `write_atomic` honours the umask; the helper must be executable by
    // the user, who runs it through sudo.
    crate::fs::set_mode(&path, FileMode::Executable)
        .map_err(|source| PlatformError::SystemFile { path, source })
}

/// Removes the installed helper; returns `false` if it was absent. Requires
/// administrator rights. The running helper may remove itself.
///
/// # Errors
///
/// Those of [`installed_path`], [`PlatformError::ForeignSystemFile`] if the
/// path is not a regular file, or [`PlatformError::SystemFile`].
pub fn remove() -> Result<bool, PlatformError> {
    let path = installed_path()?;
    match fs::symlink_metadata(&path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PlatformError::SystemFile { path, source }),
        Ok(metadata) if !metadata.is_file() => Err(PlatformError::ForeignSystemFile { path }),
        Ok(_) => {
            fs::remove_file(&path).map_err(|source| PlatformError::SystemFile { path, source })?;
            Ok(true)
        }
    }
}

/// Whether `exe` (the running helper) is the installed helper, owned by the
/// administrator. On platforms without an installed helper, `true`.
///
/// # Errors
///
/// [`PlatformError::SystemFile`] if a path cannot be inspected.
pub fn is_installed_copy(exe: &Path) -> Result<bool, PlatformError> {
    let Ok(installed) = installed_path() else {
        return Ok(true);
    };
    let inspect = |path: &Path| {
        crate::fs::canonicalize(path).map_err(|source| PlatformError::SystemFile {
            path: path.to_path_buf(),
            source,
        })
    };
    if fs::symlink_metadata(&installed).is_err() {
        return Ok(false);
    }
    if inspect(exe)? != inspect(&installed)? {
        return Ok(false);
    }
    let metadata =
        fs::symlink_metadata(&installed).map_err(|source| PlatformError::SystemFile {
            path: installed.clone(),
            source,
        })?;
    Ok(metadata.is_file() && imp::is_admin_only(&metadata))
}

fn read(path: &Path) -> Result<Vec<u8>, PlatformError> {
    let failed = |source| PlatformError::SystemFile {
        path: path.to_path_buf(),
        source,
    };
    let file = fs::File::open(path).map_err(failed)?;
    let mut data = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(file, MAX_HELPER_LEN + 1), &mut data)
        .map_err(failed)?;
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > MAX_HELPER_LEN {
        return Err(failed(io::Error::new(
            io::ErrorKind::InvalidData,
            "the helper is larger than 64 MiB",
        )));
    }
    Ok(data)
}

fn ensure_supported() -> Result<(), PlatformError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(PlatformError::Unsupported {
            feature: "the installed privileged helper",
        })
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::{self, Metadata};
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    use std::path::Path;

    use crate::PlatformError;

    /// Owned by `root`, writable by nobody else.
    pub(super) fn is_admin_only(metadata: &Metadata) -> bool {
        metadata.uid() == 0 && metadata.mode() & 0o022 == 0
    }

    /// Creates `dir` (`0755`) if needed and checks that only the
    /// administrator can write to it.
    pub(super) fn ensure_admin_dir(dir: &Path) -> Result<(), PlatformError> {
        let failed = |source| PlatformError::SystemFile {
            path: dir.to_path_buf(),
            source,
        };
        if fs::symlink_metadata(dir).is_err() {
            fs::DirBuilder::new()
                .mode(0o755)
                .create(dir)
                .map_err(failed)?;
        }
        let metadata = fs::symlink_metadata(dir).map_err(failed)?;
        if !metadata.is_dir() || !is_admin_only(&metadata) {
            return Err(PlatformError::InsecureSystemDirectory {
                path: dir.to_path_buf(),
            });
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use std::fs::Metadata;
    use std::path::Path;

    use crate::PlatformError;

    pub(super) fn is_admin_only(_metadata: &Metadata) -> bool {
        false
    }

    pub(super) fn ensure_admin_dir(_dir: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            feature: "the installed privileged helper",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_conflicting_and_user_owned_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("devspace-helper");
        fs::write(&source, b"helper v1").unwrap();
        let installed = tmp.path().join("installed");

        assert_eq!(
            state_at(&installed, &source).unwrap(),
            IntegrationState::Missing
        );
        fs::create_dir(&installed).unwrap();
        assert_eq!(
            state_at(&installed, &source).unwrap(),
            IntegrationState::Conflict
        );
        fs::remove_dir(&installed).unwrap();

        // A copy owned by the user running the tests is never trusted, even
        // with the same content.
        fs::write(&installed, b"helper v1").unwrap();
        let expected = if imp::is_admin_only(&fs::metadata(&installed).unwrap()) {
            IntegrationState::Installed
        } else {
            IntegrationState::Outdated
        };
        assert_eq!(state_at(&installed, &source).unwrap(), expected);
    }

    #[test]
    fn running_copy_outside_the_installed_path_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("devspace-helper");
        fs::write(&exe, b"helper").unwrap();
        assert_eq!(
            is_installed_copy(&exe).unwrap(),
            !cfg!(target_os = "macos") || installed_path().unwrap() == exe
        );
    }

    #[test]
    fn support_matches_platform() {
        assert_eq!(installed_path().is_ok(), cfg!(target_os = "macos"));
    }
}
