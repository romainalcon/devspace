//! SQLite storage of the daemon.
//!
//! The database lives in
//! [`AppPaths::database_file`](crate::AppPaths::database_file) and is only
//! opened by the daemon, which holds the single-instance lock; clients never
//! touch it directly.
//!
//! The schema version is stored in SQLite's `user_version` header field.
//! [`MIGRATIONS`] are applied in order at open time, each in its own
//! transaction together with the version bump, so a failed migration leaves
//! the database at the previous version. A database whose version is newer
//! than this build is refused rather than modified.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};

use crate::{Error, Result};

/// Schema migrations; migration `i` (0-based) upgrades the schema from
/// version `i` to version `i + 1`. Applied migrations must never change.
const MIGRATIONS: &[&str] = &[
    // Version 1: project registry.
    "CREATE TABLE projects (
        id       INTEGER PRIMARY KEY,
        name     TEXT    NOT NULL UNIQUE,
        path     TEXT    NOT NULL UNIQUE,
        added_at INTEGER NOT NULL
    ) STRICT;",
    // Version 2: choices of the user for the service instances; an
    // instance without a row follows the projects.
    "CREATE TABLE services (
        name   TEXT NOT NULL,
        series TEXT NOT NULL,
        mode   TEXT NOT NULL CHECK (mode IN ('started', 'stopped')),
        PRIMARY KEY (name, series)
    ) STRICT;",
];

/// Schema version produced by this build.
#[allow(clippy::cast_possible_truncation)] // A handful of migrations.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// How long a statement waits for a lock held by another connection.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// An open, migrated database.
#[derive(Debug)]
pub struct Database {
    pub(crate) conn: Connection,
    path: PathBuf,
}

impl Database {
    /// Opens (creating it if needed) the database at `path` and applies the
    /// pending migrations. The parent directory must exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DatabaseOpen`] if the file cannot be opened or
    /// configured, [`Error::DatabaseTooNew`] if it was created by a newer
    /// DevSpace, or [`Error::DatabaseMigration`] if a migration fails.
    pub fn open(path: &Path) -> Result<Self> {
        let open_err = |source| Error::DatabaseOpen {
            path: path.to_path_buf(),
            source,
        };
        let conn = Connection::open(path).map_err(open_err)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(open_err)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(open_err)?;
        // `journal_mode` returns the resulting mode as a row.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
            .map_err(open_err)?;
        let mut db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Opens a private in-memory database, for tests.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
        let path = PathBuf::from(":memory:");
        let conn = Connection::open_in_memory().map_err(|source| Error::DatabaseOpen {
            path: path.clone(),
            source,
        })?;
        let mut db = Self { conn, path };
        db.migrate()?;
        Ok(db)
    }

    /// Path of the database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current schema version of the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the version cannot be read.
    pub fn schema_version(&self) -> Result<u32> {
        read_version(&self.conn).map_err(|source| Error::Database {
            operation: "reading the schema version",
            source,
        })
    }

    fn migrate(&mut self) -> Result<()> {
        let current = self.schema_version()?;
        if current > SCHEMA_VERSION {
            return Err(Error::DatabaseTooNew {
                path: self.path.clone(),
                found: current,
                supported: SCHEMA_VERSION,
            });
        }
        for (version, sql) in (1..).zip(MIGRATIONS).skip(current as usize) {
            let migration_err = |source| Error::DatabaseMigration {
                path: self.path.clone(),
                version,
                source,
            };
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(migration_err)?;
            tx.execute_batch(sql).map_err(migration_err)?;
            tx.pragma_update(None, "user_version", version)
                .map_err(migration_err)?;
            tx.commit().map_err(migration_err)?;
            tracing::info!(
                database = %self.path.display(),
                version,
                "database schema migrated"
            );
        }
        Ok(())
    }
}

fn read_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_database_is_fully_migrated() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn reopening_keeps_data_and_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devspace.db");
        {
            let db = Database::open(&path).unwrap();
            db.conn
                .execute(
                    "INSERT INTO projects (name, path, added_at) VALUES ('a', '/a', 0)",
                    [],
                )
                .unwrap();
        }
        let db = Database::open(&path).unwrap();
        assert_eq!(db.path(), path);
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn newer_database_is_refused_and_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devspace.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        let err = Database::open(&path).unwrap_err();
        assert!(matches!(
            err,
            Error::DatabaseTooNew { found, supported, .. }
                if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));
        let conn = Connection::open(&path).unwrap();
        assert_eq!(read_version(&conn).unwrap(), SCHEMA_VERSION + 1);
    }

    #[test]
    fn file_that_is_not_a_database_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devspace.db");
        std::fs::write(&path, b"definitely not an SQLite file, just some text").unwrap();
        assert!(Database::open(&path).is_err());
    }
}
