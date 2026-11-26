//! Per-project manifest, `devspace.toml` at the root of a project.
//!
//! The manifest is optional and meant to be committed with the project:
//!
//! ```toml
//! schema_version = 1
//!
//! [project]
//! name = "my-app"   # lowercase letters, digits and hyphens; 1–63 characters
//!
//! [web]             # what https://<name>.internal serves; at most one of:
//! root = "public"   # files of this directory of the project (PHP when it
//!                   # has an index.php and no index.html)
//! # proxy = "127.0.0.1:3000"  # or a development server on the loopback
//!
//! [runtimes]        # versions used by the project: MAJOR, MAJOR.MINOR
//! php = "8.4"       # or MAJOR.MINOR.PATCH; the newest installed match
//! node = "24"       # is used (the newest installed version if absent)
//!
//! [services]        # services the project uses, started by the daemon:
//! postgresql = "18" # MAJOR (PostgreSQL) or MAJOR.MINOR (MySQL series),
//! mysql = "8.4"     # matched like the runtimes
//! ```
//!
//! Every key is optional and unknown keys are rejected. DevSpace only reads
//! the manifest; it never writes or executes anything from it.

use std::fmt;
use std::fs::File;
use std::io::{self, Read as _};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::ProjectName;
use crate::runtime::{RuntimeName, VersionReq};
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
    /// `[web]` table.
    pub web: ManifestWeb,
    /// `[runtimes]` table.
    pub runtimes: ManifestRuntimes,
    /// `[services]` table.
    pub services: ManifestServices,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            project: ManifestProject::default(),
            web: ManifestWeb::default(),
            runtimes: ManifestRuntimes::default(),
            services: ManifestServices::default(),
        }
    }
}

/// `[runtimes]` table of the manifest: the versions the project needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManifestRuntimes {
    /// Node.js versions accepted.
    pub node: Option<VersionReq>,
    /// PHP versions accepted.
    pub php: Option<VersionReq>,
}

impl ManifestRuntimes {
    /// Requirement for `name`, if the manifest sets one (never for the
    /// services, see [`ManifestServices`]).
    #[must_use]
    pub const fn get(&self, name: RuntimeName) -> Option<VersionReq> {
        match name {
            RuntimeName::Node => self.node,
            RuntimeName::Php => self.php,
            RuntimeName::Mysql | RuntimeName::Postgresql => None,
        }
    }
}

/// `[services]` table of the manifest: the services the project uses. The
/// daemon runs the instance of the newest installed version matching each
/// requirement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManifestServices {
    /// MySQL versions accepted.
    pub mysql: Option<VersionReq>,
    /// PostgreSQL versions accepted.
    pub postgresql: Option<VersionReq>,
}

impl ManifestServices {
    /// Requirement for the service packaged as `name`, if the manifest
    /// sets one.
    #[must_use]
    pub const fn get(&self, name: RuntimeName) -> Option<VersionReq> {
        match name {
            RuntimeName::Mysql => self.mysql,
            RuntimeName::Postgresql => self.postgresql,
            RuntimeName::Node | RuntimeName::Php => None,
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

/// `[web]` table of the manifest: what the local site of the project
/// serves. `root` and `proxy` are mutually exclusive; without either, the
/// `public` directory of the project is served if it exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManifestWeb {
    /// Directory of static files, relative to the project.
    pub root: Option<WebRoot>,
    /// Development server receiving the requests.
    pub proxy: Option<ProxyUpstream>,
}

/// Directory relative to the project root, without `..` components: the
/// site never serves files from outside the project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct WebRoot(PathBuf);

impl WebRoot {
    /// Validates a relative directory.
    ///
    /// # Errors
    ///
    /// Returns a description of the broken rule.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let path = Path::new(text);
        if text.is_empty() {
            return Err("web.root cannot be empty".to_owned());
        }
        if !path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!(
                "web.root `{text}` must be a directory inside the project \
                 (relative, without `..`)"
            ));
        }
        Ok(Self(path.to_path_buf()))
    }

    /// The relative path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl TryFrom<String> for WebRoot {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, String> {
        Self::parse(&text)
    }
}

/// Address of a development server on the loopback interface:
/// `127.0.0.1:<port>`, `[::1]:<port>` or `localhost:<port>`. Other hosts are
/// refused so that a local site never relays traffic to the network.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ProxyUpstream(String);

impl ProxyUpstream {
    /// Validates a loopback address with a port.
    ///
    /// # Errors
    ///
    /// Returns a description of the broken rule.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let invalid = || {
            format!(
                "web.proxy `{text}` must be a loopback address with a port, such as \
                 `127.0.0.1:3000`, `[::1]:3000` or `localhost:3000`"
            )
        };
        let valid = match text.strip_prefix("localhost:") {
            Some(port) => port.parse::<u16>().is_ok_and(|port| port > 0),
            None => text
                .parse::<SocketAddr>()
                .is_ok_and(|addr| addr.ip().is_loopback() && addr.port() > 0),
        };
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(invalid())
        }
    }

    /// The address, in the `host:port` form understood by the web server.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProxyUpstream {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, String> {
        Self::parse(&text)
    }
}

impl fmt::Display for ProxyUpstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Manifest {
    /// Parses a manifest from TOML text. `origin` is only used in errors.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ManifestParse`] for invalid TOML, an invalid value
    /// or an unknown key, [`Error::UnsupportedManifestVersion`] for an
    /// unknown `schema_version`, and [`Error::InvalidManifest`] for
    /// conflicting settings.
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
        if manifest.web.root.is_some() && manifest.web.proxy.is_some() {
            return Err(Error::InvalidManifest {
                path: origin.to_path_buf(),
                reason: "web.root and web.proxy cannot both be set",
            });
        }
        Ok(manifest)
    }

    /// Requirement of the project for the package `name`: `[runtimes]`
    /// for the language runtimes, `[services]` for the services.
    #[must_use]
    pub const fn requirement(&self, name: RuntimeName) -> Option<VersionReq> {
        if name.is_service() {
            self.services.get(name)
        } else {
            self.runtimes.get(name)
        }
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
    fn web_settings_are_validated() {
        let manifest = Manifest::from_toml("[web]\nroot = \"dist/site\"\n", origin()).unwrap();
        assert_eq!(manifest.web.root.unwrap().as_path(), Path::new("dist/site"));
        for proxy in [
            "127.0.0.1:3000",
            "[::1]:5173",
            "localhost:8000",
            "127.0.0.2:1",
        ] {
            let text = format!("[web]\nproxy = \"{proxy}\"\n");
            let manifest = Manifest::from_toml(&text, origin()).unwrap();
            assert_eq!(manifest.web.proxy.unwrap().as_str(), proxy);
        }
        for invalid in [
            "root = \"\"",
            "root = \"../other\"",
            "root = \"public/../../x\"",
            "root = \"/var/www\"",
            "proxy = \"192.168.1.10:80\"",
            "proxy = \"example.com:80\"",
            "proxy = \"127.0.0.1\"",
            "proxy = \"localhost:0\"",
            "proxy = \"localhost:http\"",
            "index = \"index.php\"",
        ] {
            let text = format!("[web]\n{invalid}\n");
            assert!(
                matches!(
                    Manifest::from_toml(&text, origin()),
                    Err(Error::ManifestParse { .. })
                ),
                "{invalid}"
            );
        }
        let both = "[web]\nroot = \"public\"\nproxy = \"127.0.0.1:3000\"\n";
        assert!(matches!(
            Manifest::from_toml(both, origin()),
            Err(Error::InvalidManifest { .. })
        ));
    }

    #[test]
    fn runtime_requirements_are_validated() {
        let manifest =
            Manifest::from_toml("[runtimes]\nphp = \"8.4\"\nnode = \"24\"\n", origin()).unwrap();
        assert_eq!(
            manifest.runtimes.get(RuntimeName::Php),
            Some(VersionReq::parse("8.4").unwrap())
        );
        assert_eq!(
            manifest.runtimes.get(RuntimeName::Node),
            Some(VersionReq::parse("24").unwrap())
        );
        for bad in [
            "[runtimes]\nphp = \"latest\"\n",
            "[runtimes]\nphp = 8.4\n",
            "[runtimes]\npython = \"3\"\n",
        ] {
            assert!(
                matches!(
                    Manifest::from_toml(bad, origin()),
                    Err(Error::ManifestParse { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn service_requirements_are_validated() {
        let manifest = Manifest::from_toml(
            "[services]\npostgresql = \"18\"\nmysql = \"8.4\"\n",
            origin(),
        )
        .unwrap();
        assert_eq!(
            manifest.requirement(RuntimeName::Postgresql),
            Some(VersionReq::parse("18").unwrap())
        );
        assert_eq!(
            manifest.requirement(RuntimeName::Mysql),
            Some(VersionReq::parse("8.4").unwrap())
        );
        assert_eq!(manifest.requirement(RuntimeName::Php), None);
        assert_eq!(manifest.runtimes.get(RuntimeName::Mysql), None);
        for bad in [
            "[services]\nredis = \"7\"\n",
            "[services]\nmysql = \"latest\"\n",
            "[runtimes]\nmysql = \"8.4\"\n",
        ] {
            assert!(
                matches!(
                    Manifest::from_toml(bad, origin()),
                    Err(Error::ManifestParse { .. })
                ),
                "{bad}"
            );
        }
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
