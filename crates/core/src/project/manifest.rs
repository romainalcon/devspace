//! Per-project manifest, `devspace.toml` at the root of a project.
//!
//! The manifest is optional and meant to be committed with the project. In
//! schema version 1 it only names the project:
//!
//! ```toml
//! schema_version = 1
//!
//! [project]
//! name = "my-app"   # lowercase letters, digits and hyphens; 1–63 characters
//! ```
//!
//! Every key is optional and unknown keys are rejected. DevSpace only reads
//! the manifest; it never writes or executes anything from it.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use serde::Deserialize;

use super::ProjectName;
use crate::{Error, Result};

/// File name of the manifest at the root of a project.
pub const MANIFEST_FILE_NAME: &str = "devspace.toml";

/// Schema version supported by this build.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Maximum size of a manifest file.
pub const MAX_MANIFEST_LEN: u64 = 64 * 1024;

/// Content of a project manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Manifest {
    /// Version of the manifest schema.
    pub schema_version: u32,
    /// `[project]` table.
    pub project: ManifestProject,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            project: ManifestProject::default(),
        }
    }
}

/// `[project]` table of the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManifestProject {
    /// Preferred project name.
    pub name: Option<ProjectName>,
}

impl Manifest {
    /// Parses a manifest from TOML text. `origin` is only used in errors.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ManifestParse`] for invalid TOML, an invalid project
    /// name or an unknown key, and [`Error::UnsupportedManifestVersion`] for
    /// an unknown `schema_version`.
    pub fn from_toml(text: &str, origin: &Path) -> Result<Self> {
        let manifest: Self = toml::from_str(text).map_err(|source| Error::ManifestParse {
            path: origin.to_path_buf(),
            source: Box::new(source),
        })?;
        if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(Error::UnsupportedManifestVersion {
                path: origin.to_path_buf(),
                found: manifest.schema_version,
                supported: MANIFEST_SCHEMA_VERSION,
            });
        }
        Ok(manifest)
    }

    /// Reads the manifest of the project in `dir`, or returns `None` when the
    /// project has none.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ManifestRead`] if the file exists but cannot be read,
    /// [`Error::ManifestTooLarge`] above [`MAX_MANIFEST_LEN`], and the errors
    /// of [`Manifest::from_toml`].
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(MANIFEST_FILE_NAME);
        let read_err = |source| Error::ManifestRead {
            path: path.clone(),
            source,
        };
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(read_err(err)),
        };
        let mut text = String::new();
        file.take(MAX_MANIFEST_LEN + 1)
            .read_to_string(&mut text)
            .map_err(read_err)?;
        if text.len() as u64 > MAX_MANIFEST_LEN {
            return Err(Error::ManifestTooLarge {
                path,
                max: MAX_MANIFEST_LEN,
            });
        }
        Self::from_toml(&text, &path).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> &'static Path {
        Path::new("devspace.toml")
    }

    #[test]
    fn empty_manifest_is_valid() {
        let manifest = Manifest::from_toml("", origin()).unwrap();
        assert_eq!(manifest, Manifest::default());
        assert_eq!(manifest.project.name, None);
    }

    #[test]
    fn name_is_read_and_validated() {
        let manifest = Manifest::from_toml(
            "schema_version = 1\n[project]\nname = \"my-app\"\n",
            origin(),
        )
        .unwrap();
        assert_eq!(manifest.project.name.unwrap().as_str(), "my-app");

        let err = Manifest::from_toml("[project]\nname = \"My App\"\n", origin()).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }));
        let report = crate::error::report(&err);
        assert!(report.contains("lowercase"), "{report}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = Manifest::from_toml("[project]\nnmae = \"x\"\n", origin()).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }));
        let err = Manifest::from_toml("[scripts]\nstart = \"rm -rf /\"\n", origin()).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }));
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let err = Manifest::from_toml("schema_version = 2\n", origin()).unwrap_err();
        assert!(matches!(
            err,
            Error::UnsupportedManifestVersion {
                found: 2,
                supported: MANIFEST_SCHEMA_VERSION,
                ..
            }
        ));
    }

    #[test]
    fn load_handles_missing_present_and_oversized_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(tmp.path()).unwrap(), None);

        let file = tmp.path().join(MANIFEST_FILE_NAME);
        std::fs::write(&file, "[project]\nname = \"blog\"\n").unwrap();
        let manifest = Manifest::load(tmp.path()).unwrap().unwrap();
        assert_eq!(manifest.project.name.unwrap().as_str(), "blog");

        let padding = "#".repeat(usize::try_from(MAX_MANIFEST_LEN).unwrap() + 1);
        std::fs::write(&file, padding).unwrap();
        assert!(matches!(
            Manifest::load(tmp.path()),
            Err(Error::ManifestTooLarge { .. })
        ));
    }

    #[test]
    fn unreadable_manifest_is_a_read_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(MANIFEST_FILE_NAME)).unwrap();
        assert!(matches!(
            Manifest::load(tmp.path()),
            Err(Error::ManifestRead { .. })
        ));
    }
}
