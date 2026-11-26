//! Package manager: language runtimes (PHP, Node.js) and the programs of
//! the services (MySQL, PostgreSQL), several versions side by side.
//!
//! - [`index`]: the signed package index embedded in DevSpace, listing the
//!   archives of every runtime version per platform with their checksums;
//! - [`install`]: download, verification, extraction and atomic
//!   installation of a version, and its removal;
//! - [`php`]: configuration of the PHP FastCGI process manager serving
//!   the PHP sites.
//!
//! Installed versions live in `<data>/runtimes/<name>/<version>/`, each
//! with a receipt (`devspace-runtime.json`) written last: a directory
//! without a valid receipt is not an installation. Projects choose their
//! versions in their manifest (`[runtimes]`, [`VersionReq`]); a runtime
//! the manifest does not mention resolves to the newest installed version.

pub mod index;
pub mod install;
pub mod php;
pub mod remote;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::AppPaths;

/// File describing an installation, inside its directory.
pub const RECEIPT_FILE_NAME: &str = "devspace-runtime.json";

/// A package DevSpace can install: a language runtime, or the programs
/// of a service (server, initialisation tool and clients).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeName {
    /// MySQL Community Server (service).
    Mysql,
    /// Node.js.
    Node,
    /// PHP (command line and FastCGI process manager).
    Php,
    /// PostgreSQL (service).
    Postgresql,
    /// Valkey (service), built by DevSpace from the official sources.
    Valkey,
}

impl RuntimeName {
    /// Every package, in display order.
    pub const ALL: [Self; 5] = [
        Self::Mysql,
        Self::Node,
        Self::Php,
        Self::Postgresql,
        Self::Valkey,
    ];

    /// Lowercase name, as used in manifests, paths and commands.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Node => "node",
            Self::Php => "php",
            Self::Postgresql => "postgresql",
            Self::Valkey => "valkey",
        }
    }

    /// Whether the package provides a service run by the daemon
    /// ([`crate::service`]).
    #[must_use]
    pub const fn is_service(self) -> bool {
        matches!(self, Self::Mysql | Self::Postgresql | Self::Valkey)
    }

    /// Parses a package name.
    ///
    /// # Errors
    ///
    /// Returns a description listing the known packages.
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|name| name.as_str() == text)
            .ok_or_else(|| {
                format!(
                    "unknown runtime `{text}`; expected `mysql`, `node`, `php`, `postgresql` or \
                     `valkey`"
                )
            })
    }
}

impl fmt::Display for RuntimeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Exact version of a package, `MAJOR.MINOR.PATCH`, or
/// `MAJOR.MINOR.PATCH.BUILD` for the rebuilds some projects publish (MySQL
/// `8.4.13.1`). A build number is never `0`, so every version has one
/// spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
    /// Rebuild of the patch version; `0` when absent.
    pub build: u32,
}

impl Version {
    /// A `MAJOR.MINOR.PATCH` version.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            build: 0,
        }
    }

    /// Parses `MAJOR.MINOR.PATCH` or `MAJOR.MINOR.PATCH.BUILD` (decimal
    /// numbers, nothing else; `BUILD` at least 1).
    ///
    /// # Errors
    ///
    /// Returns a description of the expected form.
    pub fn parse(text: &str) -> Result<Self, String> {
        match parse_numbers(text).as_deref() {
            Some(&[major, minor, patch]) => Ok(Self::new(major, minor, patch)),
            Some(&[major, minor, patch, build]) if build > 0 => Ok(Self {
                major,
                minor,
                patch,
                build,
            }),
            _ => Err(format!(
                "invalid version `{text}`; expected MAJOR.MINOR.PATCH, such as `8.4.23`"
            )),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.build > 0 {
            write!(f, ".{}", self.build)?;
        }
        Ok(())
    }
}

impl FromStr for Version {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::parse(text)
    }
}

impl TryFrom<String> for Version {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text)
    }
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Versions accepted by a project or an installation request: `MAJOR`,
/// `MAJOR.MINOR`, `MAJOR.MINOR.PATCH` (which also accepts the rebuilds of
/// that patch version) or an exact rebuild `MAJOR.MINOR.PATCH.BUILD`. The
/// newest matching version is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionReq {
    /// Required major version.
    pub major: u32,
    /// Required minor version, if any.
    pub minor: Option<u32>,
    /// Required patch version, if any (only with a minor version).
    pub patch: Option<u32>,
    /// Required rebuild, if any (only with a patch version).
    pub build: Option<u32>,
}

impl VersionReq {
    /// Parses a requirement.
    ///
    /// # Errors
    ///
    /// Returns a description of the expected forms.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (major, minor, patch, build) = match parse_numbers(text).as_deref() {
            Some(&[major]) => (major, None, None, None),
            Some(&[major, minor]) => (major, Some(minor), None, None),
            Some(&[major, minor, patch]) => (major, Some(minor), Some(patch), None),
            // Build numbers are never 0 (see [`Version`]).
            Some(&[major, minor, patch, build]) if build > 0 => {
                (major, Some(minor), Some(patch), Some(build))
            }
            _ => {
                return Err(format!(
                    "invalid version `{text}`; expected MAJOR, MAJOR.MINOR, MAJOR.MINOR.PATCH \
                     or MAJOR.MINOR.PATCH.BUILD, such as `8.4`"
                ));
            }
        };
        Ok(Self {
            major,
            minor,
            patch,
            build,
        })
    }

    /// Whether `version` satisfies the requirement.
    #[must_use]
    pub fn matches(&self, version: Version) -> bool {
        self.major == version.major
            && self.minor.is_none_or(|minor| minor == version.minor)
            && self.patch.is_none_or(|patch| patch == version.patch)
            && self.build.is_none_or(|build| build == version.build)
    }

    /// Newest of `versions` satisfying the requirement.
    pub fn best<I: IntoIterator<Item = Version>>(&self, versions: I) -> Option<Version> {
        versions.into_iter().filter(|v| self.matches(*v)).max()
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.major)?;
        if let Some(minor) = self.minor {
            write!(f, ".{minor}")?;
        }
        if let Some(patch) = self.patch {
            write!(f, ".{patch}")?;
        }
        if let Some(build) = self.build {
            write!(f, ".{build}")?;
        }
        Ok(())
    }
}

impl TryFrom<String> for VersionReq {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text)
    }
}

impl<'de> Deserialize<'de> for VersionReq {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Dot-separated decimal numbers without signs, spaces or empty parts.
fn parse_numbers(text: &str) -> Option<Vec<u32>> {
    text.split('.')
        .map(|part| {
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect()
}

/// Directories of the installed runtimes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLayout {
    /// `<data>/runtimes`.
    pub root: PathBuf,
    /// Partial downloads.
    pub download_dir: PathBuf,
    /// Configuration generated for the PHP FastCGI process managers.
    pub php_config_dir: PathBuf,
    /// Sockets and pid files of the PHP FastCGI process managers.
    pub php_run_dir: PathBuf,
    /// Logs of the PHP FastCGI process managers.
    pub log_dir: PathBuf,
}

impl RuntimeLayout {
    /// Layout inside the DevSpace directories.
    #[must_use]
    pub fn new(paths: &AppPaths) -> Self {
        Self {
            root: paths.data_dir.join("runtimes"),
            download_dir: paths.cache_dir.join("downloads"),
            php_config_dir: paths.data_dir.join("php"),
            php_run_dir: paths.runtime_dir.join("php"),
            log_dir: paths.log_dir.clone(),
        }
    }

    /// Installation directory of a version.
    #[must_use]
    pub fn version_dir(&self, name: RuntimeName, version: Version) -> PathBuf {
        self.root.join(name.as_str()).join(version.to_string())
    }

    /// Directory of the installations being built or removed, on the same
    /// filesystem as the installations so that renames are atomic.
    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.root.join(".staging")
    }
}

/// Description of an installation, stored in its directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Format version of the receipt.
    pub schema_version: u32,
    /// Runtime.
    pub name: RuntimeName,
    /// Version.
    pub version: Version,
    /// Directories holding the programs, relative to the installation.
    pub bin: Vec<String>,
    /// Archives the installation was built from.
    pub archives: Vec<ReceiptArchive>,
    /// Serial of the package index used.
    pub index_serial: u64,
    /// Installation time, in seconds since the Unix epoch.
    pub installed_at: u64,
}

/// Archive of an installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptArchive {
    /// Download URL.
    pub url: String,
    /// Verified SHA-256.
    pub sha256: String,
}

/// Current receipt format.
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// An installed runtime version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntime {
    /// Runtime.
    pub name: RuntimeName,
    /// Version.
    pub version: Version,
    /// Installation directory.
    pub dir: PathBuf,
    /// Directories holding its programs, to put in `PATH`.
    pub bin_dirs: Vec<PathBuf>,
}

impl InstalledRuntime {
    /// Path of the program `base` (without the `.exe` suffix) in the first
    /// directory of [`Self::bin_dirs`] holding it.
    #[must_use]
    pub fn program(&self, base: &str) -> Option<PathBuf> {
        let file = devspace_platform::target::executable_name(base);
        self.bin_dirs
            .iter()
            .map(|dir| dir.join(&file))
            .find(|path| path.is_file())
    }
}

/// Installed versions of every runtime, sorted by name then version.
/// Directories without a valid receipt (interrupted or foreign) are
/// ignored.
#[must_use]
pub fn list_installed(layout: &RuntimeLayout) -> Vec<InstalledRuntime> {
    let mut installed = Vec::new();
    for name in RuntimeName::ALL {
        let Ok(entries) = fs::read_dir(layout.root.join(name.as_str())) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(version) = entry
                .file_name()
                .to_str()
                .and_then(|text| Version::parse(text).ok())
            else {
                continue;
            };
            if let Some(runtime) = load_installed(&entry.path(), name, version) {
                installed.push(runtime);
            }
        }
    }
    installed.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    installed
}

/// Reads the installation in `dir`, if its receipt matches.
fn load_installed(dir: &Path, name: RuntimeName, version: Version) -> Option<InstalledRuntime> {
    let text = fs::read(dir.join(RECEIPT_FILE_NAME)).ok()?;
    let receipt: Receipt = match serde_json::from_slice(&text) {
        Ok(receipt) => receipt,
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "ignoring an invalid runtime receipt");
            return None;
        }
    };
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION
        || receipt.name != name
        || receipt.version != version
    {
        tracing::warn!(dir = %dir.display(), "ignoring a runtime receipt that does not match its directory");
        return None;
    }
    let bin_dirs = receipt
        .bin
        .iter()
        .map(|relative| index::RelativePath::parse(relative).map(|path| path.under(dir)))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(InstalledRuntime {
        name,
        version,
        dir: dir.to_path_buf(),
        bin_dirs,
    })
}

/// How a project's runtime version was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Required by the project manifest.
    Manifest(VersionReq),
    /// Not mentioned by the manifest: the newest installed version.
    Newest,
}

/// Outcome of [`select`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selected {
    /// An installed version satisfies the selection.
    Installed(InstalledRuntime),
    /// No installed version satisfies the selection.
    Missing,
}

/// Picks the installed version of `name` satisfying `selection`.
#[must_use]
pub fn select(installed: &[InstalledRuntime], name: RuntimeName, selection: Selection) -> Selected {
    let candidates = installed.iter().filter(|runtime| runtime.name == name);
    let best = match selection {
        Selection::Manifest(req) => candidates
            .filter(|runtime| req.matches(runtime.version))
            .max_by_key(|runtime| runtime.version),
        Selection::Newest => candidates.max_by_key(|runtime| runtime.version),
    };
    best.cloned().map_or(Selected::Missing, Selected::Installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_parse_and_order_numerically() {
        assert_eq!(v("8.4.23").to_string(), "8.4.23");
        assert!(v("8.4.9") < v("8.4.23"));
        assert!(v("22.23.5") < v("24.1.0"));
        assert_eq!(v("8.4.13.1").to_string(), "8.4.13.1");
        assert!(v("8.4.13") < v("8.4.13.1"));
        assert!(v("8.4.13.1") < v("8.4.14"));
        for bad in [
            "",
            "8",
            "8.4",
            "8.4.x",
            "v8.4.1",
            "8.4.1.0",
            "8.4.1.2.3",
            "8..1",
            "+8.4.1",
            " 8.4.1",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn requirements_match_prefixes() {
        let req = |text| VersionReq::parse(text).unwrap();
        assert!(req("8").matches(v("8.5.8")));
        assert!(req("8.4").matches(v("8.4.23")));
        assert!(!req("8.4").matches(v("8.5.8")));
        assert!(req("8.4.23").matches(v("8.4.23")));
        assert!(!req("8.4.23").matches(v("8.4.28")));
        assert!(req("8.4.13").matches(v("8.4.13.1")));
        // Exact rebuilds, as published in the index (MySQL `9.7.4.1`).
        assert!(req("9.7.4.1").matches(v("9.7.4.1")));
        assert!(!req("9.7.4.1").matches(v("9.7.4.2")));
        assert!(!req("9.7.4.1").matches(v("9.7.4")));
        assert_eq!(req("9.7.4.1").to_string(), "9.7.4.1");
        assert_eq!(
            req("8.4").best([v("8.4.9"), v("8.4.23"), v("8.5.8")]),
            Some(v("8.4.23"))
        );
        assert_eq!(req("7").best([v("8.4.9")]), None);
        assert_eq!(req("8.4").to_string(), "8.4");
        for bad in ["", "latest", "8.x", "^8.4", "8.4.1.0", "8.4.1.1.1"] {
            assert!(VersionReq::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn runtime_names_round_trip() {
        for name in RuntimeName::ALL {
            assert_eq!(RuntimeName::parse(name.as_str()).unwrap(), name);
        }
        assert!(RuntimeName::parse("python").is_err());
    }

    fn write_receipt(layout: &RuntimeLayout, name: RuntimeName, version: &str, bin: &[&str]) {
        let dir = layout.version_dir(name, v(version));
        fs::create_dir_all(&dir).unwrap();
        let receipt = Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            name,
            version: v(version),
            bin: bin.iter().map(|b| (*b).to_owned()).collect(),
            archives: Vec::new(),
            index_serial: 1,
            installed_at: 0,
        };
        fs::write(
            dir.join(RECEIPT_FILE_NAME),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn only_complete_installations_are_listed_and_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(&AppPaths::from_root(tmp.path()));
        write_receipt(&layout, RuntimeName::Php, "8.4.23", &["bin"]);
        write_receipt(&layout, RuntimeName::Php, "8.5.8", &["bin"]);
        write_receipt(&layout, RuntimeName::Node, "24.22.3", &["bin"]);
        // Interrupted installation: no receipt.
        fs::create_dir_all(layout.version_dir(RuntimeName::Node, v("26.13.1"))).unwrap();
        // Receipt of another version.
        write_receipt(&layout, RuntimeName::Node, "22.23.5", &["bin"]);
        fs::rename(
            layout.version_dir(RuntimeName::Node, v("22.23.5")),
            layout.version_dir(RuntimeName::Node, v("22.23.6")),
        )
        .unwrap();
        // Unsafe receipt.
        write_receipt(&layout, RuntimeName::Node, "20.0.0", &["../../outside"]);

        let installed = list_installed(&layout);
        let names: Vec<String> = installed
            .iter()
            .map(|r| format!("{} {}", r.name, r.version))
            .collect();
        assert_eq!(names, ["node 24.22.3", "php 8.4.23", "php 8.5.8"]);
        assert_eq!(
            installed[1].bin_dirs,
            [layout
                .version_dir(RuntimeName::Php, v("8.4.23"))
                .join("bin")]
        );

        let pick = |selection| match select(&installed, RuntimeName::Php, selection) {
            Selected::Installed(runtime) => Some(runtime.version.to_string()),
            Selected::Missing => None,
        };
        assert_eq!(pick(Selection::Newest).as_deref(), Some("8.5.8"));
        assert_eq!(
            pick(Selection::Manifest(VersionReq::parse("8.4").unwrap())).as_deref(),
            Some("8.4.23")
        );
        assert_eq!(
            pick(Selection::Manifest(VersionReq::parse("8.3").unwrap())),
            None
        );
    }
}
