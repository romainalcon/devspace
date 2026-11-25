//! The signed package index.
//!
//! `registry/index.json` lists, for every runtime version and platform,
//! the archives to download with their size and SHA-256, and the
//! directories holding the programs once extracted:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "serial": 1,
//!   "runtimes": {
//!     "node": {
//!       "24.22.3": {
//!         "macos-aarch64": {
//!           "archives": [{
//!             "url": "https://nodejs.org/dist/v24.22.3/node-v24.22.3-darwin-arm64.tar.gz",
//!             "sha256": "ff6e…",
//!             "size": 53084299,
//!             "format": "tar.gz",
//!             "strip_components": 1
//!           }],
//!           "bin": ["bin"]
//!         }
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! The index is embedded in DevSpace together with its minisign signature
//! (`index.json.minisig`) and the public key (`devspace.pub`). The signature
//! is checked before the index is parsed, so that an index modified without
//! the maintainer's secret key is never used; each archive is then checked
//! against its SHA-256 before extraction. Parsing is strict: unknown keys,
//! plain HTTP URLs, malformed checksums and paths leaving the installation
//! directory are refused.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use devspace_platform::paths::OsFamily;
use devspace_platform::target::{Arch, Target};
use serde::Deserialize;

use super::{RuntimeName, Version};
use crate::archive::ArchiveKind;
use crate::{Error, Result};

/// Index embedded at build time.
const EMBEDDED_INDEX: &[u8] = include_bytes!("../../../../registry/index.json");

/// Signature of [`EMBEDDED_INDEX`].
const EMBEDDED_SIGNATURE: &str = include_str!("../../../../registry/index.json.minisig");

/// Public key of the DevSpace package index (minisign format).
pub const PUBLIC_KEY: &str = include_str!("../../../../registry/devspace.pub");

/// Index format supported by this build.
pub const INDEX_SCHEMA_VERSION: u32 = 1;

/// Largest archive an index may declare.
pub const MAX_ARCHIVE_LEN: u64 = 512 * 1024 * 1024;

/// Most leading components an archive may strip.
const MAX_STRIP_COMPONENTS: usize = 4;

/// A verified package index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    /// Format version.
    pub schema_version: u32,
    /// Increases with every published index.
    pub serial: u64,
    /// Versions of every runtime, then packages per platform key
    /// ([`platform_key`]).
    pub runtimes: BTreeMap<RuntimeName, BTreeMap<Version, BTreeMap<String, Package>>>,
}

/// What to install for one runtime version on one platform.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Archives extracted into the installation directory, in order.
    pub archives: Vec<PackageArchive>,
    /// Directories holding the programs, relative to the installation.
    pub bin: Vec<RelativePath>,
}

/// One downloadable archive.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageArchive {
    /// HTTPS download URL.
    pub url: HttpsUrl,
    /// SHA-256 of the archive, lowercase hexadecimal.
    pub sha256: Sha256Hex,
    /// Size in bytes; larger downloads are refused.
    pub size: u64,
    /// Archive format.
    pub format: ArchiveFormat,
    /// Leading path components removed from every entry.
    #[serde(default)]
    pub strip_components: usize,
    /// Directory of the installation receiving the entries (default: its
    /// root).
    #[serde(default)]
    pub destination: Option<RelativePath>,
}

impl PackageArchive {
    /// File name of the archive (last segment of its URL).
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.url
            .0
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("archive")
    }
}

/// Archive format named in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ArchiveFormat {
    /// `.tar.gz`.
    #[serde(rename = "tar.gz")]
    TarGz,
    /// `.zip`.
    #[serde(rename = "zip")]
    Zip,
}

impl From<ArchiveFormat> for ArchiveKind {
    fn from(format: ArchiveFormat) -> Self {
        match format {
            ArchiveFormat::TarGz => Self::TarGz,
            ArchiveFormat::Zip => Self::Zip,
        }
    }
}

/// An `https://` URL without whitespace or control characters.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct HttpsUrl(String);

impl HttpsUrl {
    /// The URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HttpsUrl {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, String> {
        let host = text.strip_prefix("https://").unwrap_or_default();
        if host.is_empty()
            || host.starts_with('/')
            || text.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(format!("`{text}` is not an https:// URL"));
        }
        Ok(Self(text))
    }
}

/// A SHA-256 digest: 64 lowercase hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Sha256Hex(String);

impl Sha256Hex {
    /// The digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256Hex {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, String> {
        if text.len() == 64
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(text))
        } else {
            Err(format!("`{text}` is not a lowercase hexadecimal SHA-256"))
        }
    }
}

/// A relative path inside an installation: `/`-separated plain names or
/// `.`, never `..`, absolute paths, drives or backslashes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct RelativePath(String);

impl RelativePath {
    /// Validates a relative path.
    ///
    /// # Errors
    ///
    /// Returns a description of the broken rule.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let valid = !text.is_empty()
            && !text.starts_with('/')
            && text
                .split('/')
                .all(|part| !part.is_empty() && part != ".." && !part.contains(['\\', ':', '\0']))
            && Path::new(text)
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(format!(
                "`{text}` is not a relative path inside the installation"
            ))
        }
    }

    /// The path as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path below `dir`.
    #[must_use]
    pub fn under(&self, dir: &Path) -> PathBuf {
        self.0
            .split('/')
            .filter(|part| *part != ".")
            .fold(dir.to_path_buf(), |path, part| path.join(part))
    }
}

impl TryFrom<String> for RelativePath {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, String> {
        Self::parse(&text)
    }
}

/// Key of a platform in the index, e.g. `macos-aarch64`.
#[must_use]
pub const fn platform_key(target: Target) -> &'static str {
    match (target.os, target.arch) {
        (OsFamily::MacOs, Arch::Aarch64) => "macos-aarch64",
        (OsFamily::MacOs, Arch::X86_64) => "macos-x86_64",
        (OsFamily::Linux, Arch::Aarch64) => "linux-aarch64",
        (OsFamily::Linux, Arch::X86_64) => "linux-x86_64",
        (OsFamily::Windows, Arch::Aarch64) => "windows-aarch64",
        (OsFamily::Windows, Arch::X86_64) => "windows-x86_64",
    }
}

impl Index {
    /// The index embedded in this build, after checking its signature.
    ///
    /// # Errors
    ///
    /// See [`Index::verify`]; an error here means the build is broken.
    pub fn embedded() -> Result<Self> {
        Self::verify(EMBEDDED_INDEX, EMBEDDED_SIGNATURE, PUBLIC_KEY)
    }

    /// Checks the minisign `signature` of `bytes` with `public_key`, then
    /// parses and validates the index.
    ///
    /// # Errors
    ///
    /// [`Error::IndexSignature`] if the signature does not match (nothing
    /// is parsed then), [`Error::IndexParse`] or [`Error::InvalidIndex`]
    /// otherwise.
    pub fn verify(bytes: &[u8], signature: &str, public_key: &str) -> Result<Self> {
        let key = minisign_verify::PublicKey::decode(public_key.trim())
            .map_err(|err| Error::IndexSignature(format!("invalid public key: {err}")))?;
        let signature = minisign_verify::Signature::decode(signature)
            .map_err(|err| Error::IndexSignature(format!("invalid signature file: {err}")))?;
        // Legacy (non-prehashed) signatures are refused.
        key.verify(bytes, &signature, false)
            .map_err(|err| Error::IndexSignature(err.to_string()))?;
        let index: Self =
            serde_json::from_slice(bytes).map_err(|err| Error::IndexParse(Box::new(err)))?;
        index.validate()?;
        Ok(index)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != INDEX_SCHEMA_VERSION {
            return Err(Error::InvalidIndex(format!(
                "schema version {} is not supported (expected {INDEX_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        for (name, versions) in &self.runtimes {
            for (version, platforms) in versions {
                for (platform, package) in platforms {
                    let at = || format!("{name} {version} ({platform})");
                    if package.archives.is_empty() || package.bin.is_empty() {
                        return Err(Error::InvalidIndex(format!(
                            "{}: a package needs archives and bin directories",
                            at()
                        )));
                    }
                    for archive in &package.archives {
                        if archive.size == 0 || archive.size > MAX_ARCHIVE_LEN {
                            return Err(Error::InvalidIndex(format!(
                                "{}: invalid size {} for `{}`",
                                at(),
                                archive.size,
                                archive.url.as_str()
                            )));
                        }
                        if archive.strip_components > MAX_STRIP_COMPONENTS {
                            return Err(Error::InvalidIndex(format!(
                                "{}: strip_components is at most {MAX_STRIP_COMPONENTS}",
                                at()
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Package of `name` `version` for `target`, if published.
    #[must_use]
    pub fn package(&self, name: RuntimeName, version: Version, target: Target) -> Option<&Package> {
        self.runtimes
            .get(&name)?
            .get(&version)?
            .get(platform_key(target))
    }

    /// Versions of `name` published for `target`, oldest first.
    #[must_use]
    pub fn versions(&self, name: RuntimeName, target: Target) -> Vec<Version> {
        self.runtimes
            .get(&name)
            .map(|versions| {
                versions
                    .iter()
                    .filter(|(_, platforms)| platforms.contains_key(platform_key(target)))
                    .map(|(version, _)| *version)
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A key pair for tests and a function signing with it.
    pub(crate) struct TestSigner {
        pk: minisign::PublicKey,
        sk: minisign::SecretKey,
    }

    impl TestSigner {
        pub(crate) fn new() -> Self {
            let minisign::KeyPair { pk, sk } =
                minisign::KeyPair::generate_unencrypted_keypair().unwrap();
            Self { pk, sk }
        }

        pub(crate) fn public_key(&self) -> String {
            self.pk.to_box().unwrap().to_string()
        }

        pub(crate) fn sign(&self, bytes: &[u8]) -> String {
            minisign::sign(Some(&self.pk), &self.sk, bytes, None, None)
                .unwrap()
                .to_string()
        }
    }

    pub(crate) const SAMPLE: &str = r#"{
        "schema_version": 1,
        "serial": 7,
        "runtimes": {
            "php": {
                "8.4.23": {
                    "macos-aarch64": {
                        "archives": [{
                            "url": "https://example.org/php-8.4.23.tar.gz",
                            "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                            "size": 10,
                            "format": "tar.gz",
                            "destination": "bin"
                        }],
                        "bin": ["bin"]
                    }
                }
            }
        }
    }"#;

    fn macos_arm() -> Target {
        Target {
            os: OsFamily::MacOs,
            arch: Arch::Aarch64,
        }
    }

    #[test]
    fn signed_index_is_parsed() {
        let signer = TestSigner::new();
        let index = Index::verify(
            SAMPLE.as_bytes(),
            &signer.sign(SAMPLE.as_bytes()),
            &signer.public_key(),
        )
        .unwrap();
        assert_eq!(index.serial, 7);
        let version = Version::parse("8.4.23").unwrap();
        let package = index
            .package(RuntimeName::Php, version, macos_arm())
            .unwrap();
        assert_eq!(package.archives[0].file_name(), "php-8.4.23.tar.gz");
        assert_eq!(
            package.archives[0].destination.as_ref().unwrap().as_str(),
            "bin"
        );
        assert_eq!(index.versions(RuntimeName::Php, macos_arm()), [version]);
        let linux = Target {
            os: OsFamily::Linux,
            arch: Arch::X86_64,
        };
        assert!(index.versions(RuntimeName::Php, linux).is_empty());
        assert!(index.versions(RuntimeName::Node, macos_arm()).is_empty());
    }

    #[test]
    fn tampered_or_foreign_signatures_are_refused() {
        let signer = TestSigner::new();
        let signature = signer.sign(SAMPLE.as_bytes());
        let tampered = SAMPLE.replace("\"serial\": 7", "\"serial\": 8");
        assert!(matches!(
            Index::verify(tampered.as_bytes(), &signature, &signer.public_key()),
            Err(Error::IndexSignature(_))
        ));
        let other = TestSigner::new();
        assert!(matches!(
            Index::verify(SAMPLE.as_bytes(), &signature, &other.public_key()),
            Err(Error::IndexSignature(_))
        ));
        assert!(matches!(
            Index::verify(SAMPLE.as_bytes(), "garbage", &signer.public_key()),
            Err(Error::IndexSignature(_))
        ));
    }

    #[test]
    fn invalid_contents_are_refused_even_when_signed() {
        let signer = TestSigner::new();
        for (from, to) in [
            ("https://example.org", "http://example.org"),
            ("\"0000", "\"ABCD"),
            ("\"destination\": \"bin\"", "\"destination\": \"../bin\""),
            ("\"bin\": [\"bin\"]", "\"bin\": [\"/usr/bin\"]"),
            ("\"size\": 10", "\"size\": 0"),
            ("\"format\": \"tar.gz\"", "\"format\": \"rar\""),
            ("\"8.4.23\"", "\"8.4\""),
            ("\"php\"", "\"python\""),
            ("\"serial\": 7", "\"serial\": 7, \"extra\": true"),
            ("\"schema_version\": 1", "\"schema_version\": 2"),
        ] {
            let text = SAMPLE.replace(from, to);
            assert_ne!(text, SAMPLE, "{from}");
            let result = Index::verify(
                text.as_bytes(),
                &signer.sign(text.as_bytes()),
                &signer.public_key(),
            );
            assert!(
                matches!(result, Err(Error::IndexParse(_) | Error::InvalidIndex(_))),
                "{from} → {to}: {result:?}"
            );
        }
    }

    #[test]
    fn relative_paths_stay_inside() {
        for good in ["bin", ".", "a/b", "./bin"] {
            assert!(RelativePath::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "/bin", "..", "a/../..", "a//b", "c:\\bin", "a\\b", "C:"] {
            assert!(RelativePath::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            RelativePath::parse("./bin").unwrap().under(Path::new("/r")),
            Path::new("/r/bin")
        );
        assert_eq!(
            RelativePath::parse(".").unwrap().under(Path::new("/r")),
            Path::new("/r")
        );
    }

    #[test]
    fn embedded_index_is_signed_and_covers_every_platform() {
        let index = Index::embedded().unwrap();
        for os in [OsFamily::MacOs, OsFamily::Linux] {
            for arch in [Arch::Aarch64, Arch::X86_64] {
                let target = Target { os, arch };
                for name in RuntimeName::ALL {
                    assert!(
                        !index.versions(name, target).is_empty(),
                        "{name} {target:?}"
                    );
                }
            }
        }
        let windows = Target {
            os: OsFamily::Windows,
            arch: Arch::X86_64,
        };
        for name in RuntimeName::ALL {
            assert!(
                !index.versions(name, windows).is_empty(),
                "{name} on Windows"
            );
        }
    }
}
