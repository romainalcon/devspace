//! Portable filesystem helpers.

use std::io;
use std::path::{Path, PathBuf};

/// Returns the canonical, absolute form of an existing `path`, with every
/// symbolic link and `.`/`..` component resolved.
///
/// On Windows the result avoids the `\\?\` verbatim prefix whenever the path
/// can be represented without it, so that it stays readable and usable by
/// other programs.
///
/// # Errors
///
/// Returns the underlying I/O error, e.g. [`io::ErrorKind::NotFound`] when
/// the path does not exist.
pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_dot_components() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a");
        std::fs::create_dir(&nested).unwrap();
        let canonical = canonicalize(&nested.join("..").join("a")).unwrap();
        assert_eq!(canonical, canonicalize(&nested).unwrap());
        assert!(canonical.is_absolute());
    }

    #[test]
    fn missing_path_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = canonicalize(&tmp.path().join("absent")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
