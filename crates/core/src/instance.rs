//! Single-instance lock of the daemon.
//!
//! The daemon holds an exclusive advisory lock on
//! [`AppPaths::daemon_lock_file`](crate::AppPaths::daemon_lock_file) for its
//! whole lifetime. The operating system releases the lock when the process
//! exits, even after a crash, so a stale lock file never blocks a restart.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Exclusive lock held for the lifetime of the value.
#[derive(Debug)]
pub struct InstanceLock {
    // Closing the file releases the lock.
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Acquires the lock at `path`, creating the file if needed. Never blocks.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AlreadyRunning`] if another process (or another
    /// handle in this process) holds the lock, or [`Error::InstanceLock`] on
    /// I/O failure.
    pub fn acquire(path: &Path) -> Result<Self> {
        let lock_err = |source| Error::InstanceLock {
            path: path.to_path_buf(),
            source,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(lock_err)?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: file,
                path: path.to_path_buf(),
            }),
            Err(TryLockError::WouldBlock) => Err(Error::AlreadyRunning {
                path: path.to_path_buf(),
            }),
            Err(TryLockError::Error(source)) => Err(lock_err(source)),
        }
    }

    /// Returns `true` if a process currently holds the lock at `path`.
    ///
    /// The check briefly takes the lock when it is free; a daemon starting at
    /// that exact moment would fail with [`Error::AlreadyRunning`]. Callers
    /// use it only to diagnose or wait for the daemon, never in a loop that
    /// competes with a starting daemon.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InstanceLock`] on I/O failure.
    pub fn is_held(path: &Path) -> Result<bool> {
        let lock_err = |source| Error::InstanceLock {
            path: path.to_path_buf(),
            source,
        };
        let file = match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => return Err(lock_err(source)),
        };
        match file.try_lock() {
            // Dropping `file` releases the lock immediately.
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(source)) => Err(lock_err(source)),
        }
    }

    /// Path of the lock file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devspaced.lock");
        assert!(!InstanceLock::is_held(&path).unwrap());

        let lock = InstanceLock::acquire(&path).unwrap();
        assert_eq!(lock.path(), path);
        assert!(InstanceLock::is_held(&path).unwrap());
        assert!(matches!(
            InstanceLock::acquire(&path),
            Err(Error::AlreadyRunning { .. })
        ));

        drop(lock);
        assert!(!InstanceLock::is_held(&path).unwrap());
        // A leftover lock file does not prevent a new acquisition.
        assert!(path.exists());
        InstanceLock::acquire(&path).unwrap();
    }

    #[test]
    fn missing_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing").join("devspaced.lock");
        assert!(matches!(
            InstanceLock::acquire(&path),
            Err(Error::InstanceLock { .. })
        ));
    }
}
