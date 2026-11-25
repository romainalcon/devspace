//! Project registry.
//!
//! A project is a directory registered under a unique [`ProjectName`]. The
//! registry is stored in the daemon [`Database`]; each directory can be
//! registered once, under its canonical path.
//!
//! The name of a new project is, by order of precedence: the name given
//! explicitly, the `project.name` of its [`Manifest`], or the directory name
//! normalised with [`ProjectName::from_directory_name`].
//!
//! Registering or removing a project never modifies the project directory.

pub mod manifest;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension as _, Row, TransactionBehavior, params};
use serde::Deserialize;

pub use self::manifest::{MANIFEST_FILE_NAME, Manifest};
use crate::db::Database;
use crate::{Error, Result};

/// Name identifying a project.
///
/// Names are valid DNS labels so that they can later be used as local domain
/// names: 1 to [`ProjectName::MAX_LEN`] characters among lowercase ASCII
/// letters, digits and hyphens, not starting or ending with a hyphen.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct ProjectName(String);

impl ProjectName {
    /// Maximum length of a name (a DNS label).
    pub const MAX_LEN: usize = 63;

    /// Validates `name`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidProjectName`] describing the broken rule.
    pub fn parse(name: &str) -> Result<Self> {
        let invalid = |reason| Error::InvalidProjectName {
            name: name.to_owned(),
            reason,
        };
        if name.is_empty() {
            return Err(invalid("the name is empty"));
        }
        if name.len() > Self::MAX_LEN {
            return Err(invalid("the name is longer than 63 characters"));
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid(
                "only lowercase ASCII letters, digits and hyphens are allowed",
            ));
        }
        if name.starts_with('-') || name.ends_with('-') {
            return Err(invalid("the name cannot start or end with a hyphen"));
        }
        Ok(Self(name.to_owned()))
    }

    /// Derives a name from a directory name: ASCII letters are lowercased,
    /// every run of other characters becomes a single hyphen, leading and
    /// trailing hyphens are dropped and the result is truncated to
    /// [`ProjectName::MAX_LEN`]. Returns `None` if nothing is left.
    ///
    /// ```
    /// use devspace_core::project::ProjectName;
    /// let name = ProjectName::from_directory_name("My_Laravel App").unwrap();
    /// assert_eq!(name.as_str(), "my-laravel-app");
    /// assert!(ProjectName::from_directory_name("___").is_none());
    /// ```
    #[must_use]
    pub fn from_directory_name(dir_name: &str) -> Option<Self> {
        let mut slug = String::with_capacity(dir_name.len());
        for c in dir_name.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
        }
        // Only ASCII characters were pushed, so byte truncation is safe.
        slug.truncate(Self::MAX_LEN);
        let slug = slug.trim_end_matches('-');
        Self::parse(slug).ok()
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProjectName {
    type Error = Error;

    fn try_from(name: String) -> Result<Self> {
        Self::parse(&name)
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A registered project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Unique name.
    pub name: ProjectName,
    /// Canonical absolute path of the project directory.
    pub path: PathBuf,
    /// Registration time, in seconds since the Unix epoch.
    pub added_at: u64,
}

impl Database {
    /// Registers the directory at `path`.
    ///
    /// `path` must be absolute; it is canonicalised (symbolic links
    /// resolved) and must be a directory. The name is `name` if given,
    /// otherwise taken from the project manifest or derived from the
    /// directory name.
    ///
    /// # Errors
    ///
    /// - [`Error::RelativeProjectPath`], [`Error::ProjectPath`],
    ///   [`Error::ProjectNotDirectory`], [`Error::NonUnicodeProjectPath`]
    ///   for an unusable path;
    /// - the errors of [`Manifest::load`];
    /// - [`Error::ProjectNameUnavailable`] if no name can be determined;
    /// - [`Error::ProjectPathTaken`] or [`Error::ProjectNameTaken`] if the
    ///   directory or the name is already registered;
    /// - [`Error::Database`] on storage failure.
    pub fn add_project(&mut self, path: &Path, name: Option<ProjectName>) -> Result<Project> {
        let path = resolve_project_dir(path)?;
        let path_text = path
            .to_str()
            .ok_or_else(|| Error::NonUnicodeProjectPath { path: path.clone() })?
            .to_owned();
        let name = match name {
            Some(name) => name,
            None => default_name(&path)?,
        };
        let added_at = now_unix_secs();

        let db_err = |source| Error::Database {
            operation: "registering a project",
            source,
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let by_path = tx
            .query_row(
                "SELECT name FROM projects WHERE path = ?1",
                [&path_text],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        if let Some(existing) = by_path {
            return Err(Error::ProjectPathTaken {
                path,
                name: existing,
            });
        }
        let by_name = tx
            .query_row(
                "SELECT path FROM projects WHERE name = ?1",
                [name.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        if let Some(existing) = by_name {
            return Err(Error::ProjectNameTaken {
                name: name.0,
                path: PathBuf::from(existing),
            });
        }
        tx.execute(
            "INSERT INTO projects (name, path, added_at) VALUES (?1, ?2, ?3)",
            params![name.as_str(), path_text, to_sql_time(added_at)],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;

        tracing::info!(project = %name, path = %path.display(), "project registered");
        Ok(Project {
            name,
            path,
            added_at,
        })
    }

    /// Lists the registered projects, sorted by name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on storage failure.
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let db_err = |source| Error::Database {
            operation: "listing projects",
            source,
        };
        let mut statement = self
            .conn
            .prepare("SELECT name, path, added_at FROM projects ORDER BY name")
            .map_err(db_err)?;
        let rows = statement.query_map([], project_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    /// Unregisters the project named `name` and returns it. The project
    /// directory is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProjectNotFound`] if no project has this name, or
    /// [`Error::Database`] on storage failure.
    pub fn remove_project(&mut self, name: &ProjectName) -> Result<Project> {
        let project = self
            .conn
            .query_row(
                "DELETE FROM projects WHERE name = ?1 RETURNING name, path, added_at",
                [name.as_str()],
                project_from_row,
            )
            .optional()
            .map_err(|source| Error::Database {
                operation: "removing a project",
                source,
            })?
            .ok_or_else(|| Error::ProjectNotFound {
                name: name.0.clone(),
            })?;
        tracing::info!(project = %project.name, path = %project.path.display(), "project removed");
        Ok(project)
    }
}

fn resolve_project_dir(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(Error::RelativeProjectPath {
            path: path.to_path_buf(),
        });
    }
    let canonical =
        devspace_platform::fs::canonicalize(path).map_err(|source| Error::ProjectPath {
            path: path.to_path_buf(),
            source,
        })?;
    if !canonical.is_dir() {
        return Err(Error::ProjectNotDirectory { path: canonical });
    }
    Ok(canonical)
}

fn default_name(dir: &Path) -> Result<ProjectName> {
    if let Some(name) = Manifest::load(dir)?.and_then(|manifest| manifest.project.name) {
        return Ok(name);
    }
    dir.file_name()
        .and_then(|name| name.to_str())
        .and_then(ProjectName::from_directory_name)
        .ok_or_else(|| Error::ProjectNameUnavailable {
            path: dir.to_path_buf(),
        })
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    let name: String = row.get(0)?;
    let name = ProjectName::parse(&name).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let path: String = row.get(1)?;
    let added_at: i64 = row.get(2)?;
    Ok(Project {
        name,
        path: PathBuf::from(path),
        added_at: u64::try_from(added_at).unwrap_or(0),
    })
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn to_sql_time(secs: u64) -> i64 {
    i64::try_from(secs).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> ProjectName {
        ProjectName::parse(text).unwrap()
    }

    fn project_dir(parent: &Path, dir_name: &str) -> PathBuf {
        let dir = parent.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        devspace_platform::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn name_rules() {
        for valid in ["a", "my-app", "app2", "0", &"a".repeat(63)] {
            assert!(ProjectName::parse(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "-app",
            "app-",
            "My-App",
            "my_app",
            "my.app",
            "é",
            &"a".repeat(64),
        ] {
            assert!(
                matches!(
                    ProjectName::parse(invalid),
                    Err(Error::InvalidProjectName { .. })
                ),
                "{invalid}"
            );
        }
    }

    #[test]
    fn directory_names_are_normalised() {
        let derive = |dir: &str| ProjectName::from_directory_name(dir).map(|n| n.0);
        assert_eq!(derive("blog").as_deref(), Some("blog"));
        assert_eq!(derive("My App").as_deref(), Some("my-app"));
        assert_eq!(derive("--api__v2--").as_deref(), Some("api-v2"));
        assert_eq!(derive("café-shop").as_deref(), Some("caf-shop"));
        assert_eq!(derive("日本").as_deref(), None);
        assert_eq!(derive("").as_deref(), None);
        let long = format!("{}-b", "a".repeat(62));
        assert_eq!(derive(&long), Some("a".repeat(62)));
    }

    #[test]
    fn add_list_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        assert!(db.list_projects().unwrap().is_empty());

        let blog_dir = project_dir(tmp.path(), "My Blog");
        let blog = db.add_project(&blog_dir, None).unwrap();
        assert_eq!(blog.name, name("my-blog"));
        assert_eq!(blog.path, blog_dir);
        assert!(blog.added_at > 0);

        let api_dir = project_dir(tmp.path(), "api");
        let api = db.add_project(&api_dir, Some(name("backend"))).unwrap();
        assert_eq!(api.name, name("backend"));

        let listed = db.list_projects().unwrap();
        assert_eq!(listed, vec![api.clone(), blog.clone()]);

        assert_eq!(db.remove_project(&name("my-blog")).unwrap(), blog);
        assert_eq!(db.list_projects().unwrap(), vec![api]);
        assert!(blog_dir.is_dir(), "the directory must be left untouched");
        assert!(matches!(
            db.remove_project(&name("my-blog")),
            Err(Error::ProjectNotFound { .. })
        ));
    }

    #[test]
    fn name_precedence_is_explicit_then_manifest_then_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let dir = project_dir(tmp.path(), "shop");
        std::fs::write(
            dir.join(MANIFEST_FILE_NAME),
            "[project]\nname = \"storefront\"\n",
        )
        .unwrap();

        let from_manifest = db.add_project(&dir, None).unwrap();
        assert_eq!(from_manifest.name, name("storefront"));
        db.remove_project(&from_manifest.name).unwrap();

        let explicit = db.add_project(&dir, Some(name("custom"))).unwrap();
        assert_eq!(explicit.name, name("custom"));
    }

    #[test]
    fn invalid_manifest_prevents_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let dir = project_dir(tmp.path(), "broken");
        std::fs::write(dir.join(MANIFEST_FILE_NAME), "schema_version = 9\n").unwrap();
        assert!(matches!(
            db.add_project(&dir, None),
            Err(Error::UnsupportedManifestVersion { .. })
        ));
        assert!(db.list_projects().unwrap().is_empty());
    }

    #[test]
    fn duplicates_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let first = project_dir(tmp.path(), "app");
        db.add_project(&first, None).unwrap();

        // Same directory through a non-canonical path.
        let alias = tmp.path().join("app").join("..").join("app");
        assert!(matches!(
            db.add_project(&alias, Some(name("other"))),
            Err(Error::ProjectPathTaken { name, .. }) if name == "app"
        ));

        let second = project_dir(tmp.path(), "nested/app");
        assert!(matches!(
            db.add_project(&second, None),
            Err(Error::ProjectNameTaken { path, .. }) if path == first
        ));
        assert_eq!(db.list_projects().unwrap().len(), 1);
    }

    #[test]
    fn unusable_paths_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        assert!(matches!(
            db.add_project(Path::new("relative/dir"), None),
            Err(Error::RelativeProjectPath { .. })
        ));
        assert!(matches!(
            db.add_project(&tmp.path().join("absent"), None),
            Err(Error::ProjectPath { .. })
        ));
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, "").unwrap();
        assert!(matches!(
            db.add_project(&file, None),
            Err(Error::ProjectNotDirectory { .. })
        ));
        let unnamed = project_dir(tmp.path(), "___");
        assert!(matches!(
            db.add_project(&unnamed, None),
            Err(Error::ProjectNameUnavailable { .. })
        ));
    }

    #[test]
    fn registry_persists_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("devspace.db");
        let dir = project_dir(tmp.path(), "persisted");
        let added = Database::open(&db_path)
            .unwrap()
            .add_project(&dir, None)
            .unwrap();
        assert_eq!(
            Database::open(&db_path).unwrap().list_projects().unwrap(),
            vec![added]
        );
    }
}
