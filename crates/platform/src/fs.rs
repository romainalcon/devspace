//! Portable filesystem helpers.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
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

/// Access granted to a file written by [`write_atomic`] or adjusted by
/// [`set_mode`]. Only meaningful on Unix; on Windows files inherit the ACL
/// of their directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// Readable and writable by the owner only (`0600`).
    Private,
    /// Writable by the owner, readable by everyone (`0644`).
    Readable,
    /// Executable program: writable by the owner, readable and executable
    /// by everyone (`0755`).
    Executable,
}

impl FileMode {
    #[cfg(unix)]
    const fn bits(self) -> u32 {
        match self {
            Self::Private => 0o600,
            Self::Readable => 0o644,
            Self::Executable => 0o755,
        }
    }
}

/// Sets the permissions of an existing file.
///
/// # Errors
///
/// Returns the underlying I/O error.
pub fn set_mode(path: &Path, mode: FileMode) -> io::Result<()> {
    imp::set_mode(path, mode)
}

/// Replaces (or creates) `path` with `contents` atomically: the data is
/// written and flushed to a temporary file in the same directory, which is
/// then renamed over `path`. Readers see either the old or the new file,
/// never a partial one.
///
/// # Errors
///
/// Returns the underlying I/O error; the temporary file is removed on
/// failure.
pub fn write_atomic(path: &Path, contents: &[u8], mode: FileMode) -> io::Result<()> {
    let temp = temporary_sibling(path)?;
    let result = (|| {
        let mut file = create_new(&temp, mode)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Creates a new file with the given mode, failing if it exists.
///
/// # Errors
///
/// Returns the underlying I/O error ([`io::ErrorKind::AlreadyExists`] if
/// the file exists).
pub fn create_new(path: &Path, mode: FileMode) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    imp::restrict(&mut options, mode);
    options.open(path)
}

/// Creates a symbolic link at `link` pointing to `target` (stored as
/// given, typically relative to the directory of `link`).
///
/// # Errors
///
/// Returns the underlying I/O error; on Windows, where creating symbolic
/// links needs a privilege regular users usually lack, always fails with
/// [`io::ErrorKind::Unsupported`].
pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    imp::symlink(target, link)
}

/// Path of a temporary file next to `path`, unique to this process.
///
/// # Errors
///
/// Fails with [`io::ErrorKind::InvalidInput`] if `path` has no file name.
pub fn temporary_sibling(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{}` has no file name", path.display()),
        )
    })?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".tmp-{}", std::process::id()));
    Ok(path.with_file_name(temp_name))
}

#[cfg(unix)]
mod imp {
    use std::fs::{self, OpenOptions, Permissions};
    use std::io;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::path::Path;

    use super::FileMode;

    pub(super) fn set_mode(path: &Path, mode: FileMode) -> io::Result<()> {
        fs::set_permissions(path, Permissions::from_mode(mode.bits()))
    }

    pub(super) fn restrict(options: &mut OpenOptions, mode: FileMode) {
        options.mode(mode.bits());
    }

    pub(super) fn symlink(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
}

#[cfg(windows)]
mod imp {
    use std::fs::OpenOptions;
    use std::io;
    use std::path::Path;

    use super::FileMode;

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn set_mode(_path: &Path, _mode: FileMode) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn restrict(_options: &mut OpenOptions, _mode: FileMode) {}

    pub(super) fn symlink(_target: &Path, link: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "cannot create the symbolic link `{}`: not supported on Windows",
                link.display()
            ),
        ))
    }
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

    #[test]
    fn write_atomic_creates_and_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        write_atomic(&file, b"first", FileMode::Private).unwrap();
        write_atomic(&file, b"second", FileMode::Readable).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"second");
        let leftovers: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temporary file must remain");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644);
        }
    }

    #[test]
    fn write_atomic_fails_cleanly_in_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("absent").join("file");
        assert!(write_atomic(&file, b"x", FileMode::Private).is_err());
    }
}
