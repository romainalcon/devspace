//! System files owned by DevSpace, recognised by a marker.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use crate::PlatformError;
use crate::fs::{FileMode, write_atomic};

/// Text present in every system file written by DevSpace.
pub const MANAGED_MARKER: &str = "Managed by DevSpace";

/// Largest system file DevSpace reads back; its own files are tiny.
const MAX_MANAGED_FILE_LEN: u64 = 64 * 1024;

/// State of a system integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationState {
    /// Present with the expected content.
    Installed,
    /// Absent.
    Missing,
    /// Present and managed by DevSpace, but with other settings (e.g.
    /// another port); installing again updates it.
    Outdated,
    /// Occupied by a file that DevSpace does not manage.
    Conflict,
}

impl IntegrationState {
    /// Combines the states of the files making up one integration.
    pub(crate) fn combine(states: &[Self]) -> Self {
        if states.contains(&Self::Conflict) {
            Self::Conflict
        } else if states.iter().all(|s| *s == Self::Installed) {
            Self::Installed
        } else if states.iter().all(|s| *s == Self::Missing) {
            Self::Missing
        } else {
            Self::Outdated
        }
    }
}

fn file_error(path: &Path) -> impl FnOnce(io::Error) -> PlatformError + '_ {
    |source| PlatformError::SystemFile {
        path: path.to_path_buf(),
        source,
    }
}

/// Reads `path`, `None` if it does not exist.
fn read(path: &Path) -> Result<Option<String>, PlatformError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(file_error(path)(err)),
    };
    let mut text = String::new();
    file.take(MAX_MANAGED_FILE_LEN)
        .read_to_string(&mut text)
        .map_err(file_error(path))?;
    Ok(Some(text))
}

/// State of the file at `path` compared with the `expected` content.
pub(crate) fn state(path: &Path, expected: &str) -> Result<IntegrationState, PlatformError> {
    Ok(match read(path)? {
        None => IntegrationState::Missing,
        Some(text) if !text.contains(MANAGED_MARKER) => IntegrationState::Conflict,
        Some(text) if text == expected => IntegrationState::Installed,
        Some(_) => IntegrationState::Outdated,
    })
}

/// Fails with [`PlatformError::ForeignSystemFile`] if `path` exists and is
/// not managed by DevSpace.
pub(crate) fn ensure_not_foreign(path: &Path) -> Result<(), PlatformError> {
    match read(path)? {
        Some(text) if !text.contains(MANAGED_MARKER) => Err(PlatformError::ForeignSystemFile {
            path: path.to_path_buf(),
        }),
        _ => Ok(()),
    }
}

/// Writes `contents` (which must contain [`MANAGED_MARKER`]) to `path`,
/// world-readable, unless the file belongs to someone else.
pub(crate) fn install(path: &Path, contents: &str) -> Result<(), PlatformError> {
    debug_assert!(contents.contains(MANAGED_MARKER));
    ensure_not_foreign(path)?;
    write_atomic(path, contents.as_bytes(), FileMode::Readable).map_err(file_error(path))
}

/// Removes `path` if it is managed by DevSpace; returns `false` if it does
/// not exist.
pub(crate) fn remove(path: &Path) -> Result<bool, PlatformError> {
    if read(path)?.is_none() {
        return Ok(false);
    }
    ensure_not_foreign(path)?;
    fs::remove_file(path).map_err(file_error(path))?;
    Ok(true)
}

/// Creates `dir` (world-readable) if it does not exist.
pub(crate) fn ensure_dir(dir: &Path) -> Result<(), PlatformError> {
    fs::create_dir_all(dir).map_err(|source| PlatformError::SystemFile {
        path: PathBuf::from(dir),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_of_a_managed_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test");
        let v1 = format!("# {MANAGED_MARKER}\nport 1\n");
        let v2 = format!("# {MANAGED_MARKER}\nport 2\n");

        assert_eq!(state(&path, &v1).unwrap(), IntegrationState::Missing);
        assert!(!remove(&path).unwrap());
        install(&path, &v1).unwrap();
        assert_eq!(state(&path, &v1).unwrap(), IntegrationState::Installed);
        assert_eq!(state(&path, &v2).unwrap(), IntegrationState::Outdated);
        install(&path, &v2).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), v2);
        assert!(remove(&path).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn foreign_files_are_never_touched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test");
        fs::write(&path, "nameserver 127.0.0.1\n").unwrap();
        let ours = format!("# {MANAGED_MARKER}\n");

        assert_eq!(state(&path, &ours).unwrap(), IntegrationState::Conflict);
        assert!(matches!(
            install(&path, &ours),
            Err(PlatformError::ForeignSystemFile { .. })
        ));
        assert!(matches!(
            remove(&path),
            Err(PlatformError::ForeignSystemFile { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "nameserver 127.0.0.1\n");
    }

    #[test]
    fn combined_states() {
        use IntegrationState::{Conflict, Installed, Missing, Outdated};
        assert_eq!(
            IntegrationState::combine(&[Installed, Installed]),
            Installed
        );
        assert_eq!(IntegrationState::combine(&[Missing, Missing]), Missing);
        assert_eq!(IntegrationState::combine(&[Installed, Missing]), Outdated);
        assert_eq!(IntegrationState::combine(&[Outdated, Installed]), Outdated);
        assert_eq!(IntegrationState::combine(&[Installed, Conflict]), Conflict);
    }
}
