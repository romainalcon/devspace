//! The pinned Caddy release and its installation.
//!
//! DevSpace uses one Caddy version, [`CADDY_VERSION`]. The official release
//! archives are downloaded over HTTPS from GitHub and verified against the
//! SHA-256 checksums embedded in [`ASSETS`] before anything is extracted.
//! The checksums were computed from the published archives and
//! cross-checked with the SHA-512 values of the release's
//! `caddy_<version>_checksums.txt` when the version was pinned; updating
//! the version means updating every checksum. (Runtimes come from the
//! signed package index of [`crate::runtime::index`]; Caddy stays pinned
//! here, as approved for M3.)
//!
//! Installation is atomic: the executable appears at
//! [`WebLayout::caddy_binary`] only once it has been verified, extracted
//! and checked to report the expected version.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use devspace_platform::fs::{FileMode, create_new, set_mode, temporary_sibling};
use devspace_platform::paths::OsFamily;
use devspace_platform::target::{Arch, Target, executable_name};

use super::WebLayout;
use crate::archive::{ArchiveKind, extract_file};
use crate::download::{self, verify_sha256};
use crate::{Error, Result};

/// Pinned Caddy version.
pub const CADDY_VERSION: &str = "2.11.9";

/// Largest archive accepted (the archives are under 20 MB).
pub const MAX_DOWNLOAD_LEN: u64 = 100 * 1024 * 1024;

/// Largest executable accepted (the executables are about 50 MB).
pub const MAX_BINARY_LEN: u64 = 200 * 1024 * 1024;

/// Time limit of `caddy version`.
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// A release archive for one platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// Operating system.
    pub os: OsFamily,
    /// Architecture.
    pub arch: Arch,
    /// End of the file name, after `caddy_<version>_`.
    pub suffix: &'static str,
    /// Archive format.
    pub kind: ArchiveKind,
    /// SHA-256 of the archive, lowercase hexadecimal.
    pub sha256: &'static str,
}

/// Archives of [`CADDY_VERSION`] for the supported platforms.
pub const ASSETS: [Asset; 5] = [
    Asset {
        os: OsFamily::MacOs,
        arch: Arch::Aarch64,
        suffix: "mac_arm64.tar.gz",
        kind: ArchiveKind::TarGz,
        sha256: "d075afc620376679a77b453399bf7ada264cab3280b2833e4ed1372abc0c1c13",
    },
    Asset {
        os: OsFamily::MacOs,
        arch: Arch::X86_64,
        suffix: "mac_amd64.tar.gz",
        kind: ArchiveKind::TarGz,
        sha256: "22af256a2cc45606f6bfa8786cd89130529331319665926f4e972234a658fb30",
    },
    Asset {
        os: OsFamily::Linux,
        arch: Arch::X86_64,
        suffix: "linux_amd64.tar.gz",
        kind: ArchiveKind::TarGz,
        sha256: "5b8cb7faf8726a2028ad96a66e54e6e81e8f361bc6175e5d22bbd997cc5177fb",
    },
    Asset {
        os: OsFamily::Linux,
        arch: Arch::Aarch64,
        suffix: "linux_arm64.tar.gz",
        kind: ArchiveKind::TarGz,
        sha256: "5e377cff994fc9362a6fcf695c53f226cfca3a3ef85bd3a3687cf252d6310165",
    },
    Asset {
        os: OsFamily::Windows,
        arch: Arch::X86_64,
        suffix: "windows_amd64.zip",
        kind: ArchiveKind::Zip,
        sha256: "ad93112d335ee687100c4c9f11bced16ca590377976b7fe7883882b9e797e472",
    },
];

impl Asset {
    /// Archive of `target`, if Caddy publishes one.
    #[must_use]
    pub fn for_target(target: Target) -> Option<&'static Self> {
        ASSETS
            .iter()
            .find(|asset| asset.os == target.os && asset.arch == target.arch)
    }

    /// Archive for the running platform.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CaddyUnavailable`] on a platform without one.
    pub fn current() -> Result<&'static Self> {
        Target::current()
            .and_then(Self::for_target)
            .ok_or(Error::CaddyUnavailable)
    }

    /// File name of the archive.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("caddy_{CADDY_VERSION}_{}", self.suffix)
    }

    /// Download URL of the archive.
    #[must_use]
    pub fn url(&self) -> String {
        format!(
            "https://github.com/caddyserver/caddy/releases/download/v{CADDY_VERSION}/{}",
            self.file_name()
        )
    }
}

/// Whether the pinned Caddy is installed.
#[must_use]
pub fn is_installed(layout: &WebLayout) -> bool {
    layout.caddy_binary.is_file()
}

/// Downloads, verifies and installs the pinned Caddy unless it is already
/// installed. Returns `true` if it was installed by this call. Blocking;
/// may take minutes on a slow connection.
///
/// # Errors
///
/// [`Error::CaddyUnavailable`], [`Error::Download`],
/// [`Error::ChecksumMismatch`], [`Error::Archive`],
/// [`Error::InstallFile`] or [`Error::CaddyVersion`]. Nothing is left at
/// the installation path on failure.
pub fn install(layout: &WebLayout) -> Result<bool> {
    if is_installed(layout) {
        return Ok(false);
    }
    let asset = Asset::current()?;
    create_dir(&layout.download_dir)?;
    let archive = layout
        .download_dir
        .join(format!("{}.part", asset.file_name()));
    let url = asset.url();
    tracing::info!(%url, "downloading Caddy");
    let result = download::fetch(&url, &archive, MAX_DOWNLOAD_LEN)
        .and_then(|()| install_archive(&archive, asset, &layout.caddy_binary));
    let _ = fs::remove_file(&archive);
    result?;
    if let Err(err) = check_version(&layout.caddy_binary) {
        let _ = fs::remove_file(&layout.caddy_binary);
        return Err(err);
    }
    tracing::info!(
        version = CADDY_VERSION,
        path = %layout.caddy_binary.display(),
        "Caddy installed"
    );
    Ok(true)
}

/// Verifies `archive` against `asset` and extracts the executable to
/// `dest` atomically.
fn install_archive(archive: &Path, asset: &Asset, dest: &Path) -> Result<()> {
    verify_sha256(archive, &asset.file_name(), asset.sha256)?;
    if let Some(parent) = dest.parent() {
        create_dir(parent)?;
    }
    let temp = temporary_sibling(dest).map_err(install_error(dest))?;
    let _ = fs::remove_file(&temp);
    let result = (|| {
        let mut file = create_new(&temp, FileMode::Executable).map_err(install_error(&temp))?;
        extract_file(
            archive,
            asset.kind,
            &executable_name("caddy"),
            &mut file,
            MAX_BINARY_LEN,
        )
        .map_err(|source| Error::Archive {
            path: archive.to_path_buf(),
            source,
        })?;
        file.sync_all().map_err(install_error(&temp))?;
        drop(file);
        // The umask may have removed bits at creation.
        set_mode(&temp, FileMode::Executable).map_err(install_error(&temp))?;
        fs::rename(&temp, dest).map_err(install_error(dest))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Runs `caddy version` and checks that it reports [`CADDY_VERSION`].
fn check_version(binary: &Path) -> Result<()> {
    let version_error = |detail: String| Error::CaddyVersion {
        path: binary.to_path_buf(),
        detail,
    };
    let mut child = Command::new(binary)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| version_error(err.to_string()))?;
    let deadline = std::time::Instant::now() + VERSION_CHECK_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(version_error("`caddy version` did not answer".to_owned()));
            }
            Err(err) => return Err(version_error(err.to_string())),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|err| version_error(err.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!("v{CADDY_VERSION}");
    let reported = stdout.split_whitespace().next().unwrap_or_default();
    if output.status.success() && reported == expected {
        Ok(())
    } else {
        Err(version_error(format!(
            "expected {expected}, got `{}`",
            stdout.trim()
        )))
    }
}

fn create_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(install_error(dir))
}

fn install_error(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    |source| Error::InstallFile {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::tar_gz;
    use crate::download::sha256_file;

    #[test]
    fn every_supported_platform_has_an_asset() {
        for (os, arch) in [
            (OsFamily::MacOs, Arch::Aarch64),
            (OsFamily::MacOs, Arch::X86_64),
            (OsFamily::Linux, Arch::X86_64),
            (OsFamily::Linux, Arch::Aarch64),
            (OsFamily::Windows, Arch::X86_64),
        ] {
            let asset = Asset::for_target(Target { os, arch }).unwrap();
            assert_eq!(asset.sha256.len(), 64);
            assert!(
                asset
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
            assert!(asset.url().starts_with(
                "https://github.com/caddyserver/caddy/releases/download/v2.11.9/caddy_2.11.9_"
            ));
        }
        assert!(
            Asset::for_target(Target {
                os: OsFamily::Windows,
                arch: Arch::Aarch64
            })
            .is_none()
        );
        assert!(Asset::current().is_ok());
    }

    fn fake_asset(sha256: &'static str) -> Asset {
        Asset {
            os: OsFamily::current(),
            arch: Arch::X86_64,
            suffix: "test.tar.gz",
            kind: ArchiveKind::TarGz,
            sha256,
        }
    }

    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    #[test]
    fn verified_archive_is_installed_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("caddy.tar.gz");
        tar_gz(
            &archive,
            &[(executable_name("caddy").as_str(), b"#!/bin/sh\n", false)],
        );
        let asset = fake_asset(leak(sha256_file(&archive).unwrap()));
        let dest = tmp.path().join("tools").join("caddy").join("caddy");
        install_archive(&archive, &asset, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        let entries: Vec<_> = fs::read_dir(dest.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1, "no temporary file must remain");
    }

    #[test]
    fn tampered_archive_is_refused_before_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("caddy.tar.gz");
        tar_gz(&archive, &[("caddy", b"evil", false)]);
        let asset = fake_asset("0000000000000000000000000000000000000000000000000000000000000000");
        let dest = tmp.path().join("bin").join("caddy");
        assert!(matches!(
            install_archive(&archive, &asset, &dest),
            Err(Error::ChecksumMismatch { .. })
        ));
        assert!(!dest.exists());
        assert!(!dest.parent().unwrap().exists());
    }

    #[test]
    fn archive_without_executable_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("caddy.tar.gz");
        tar_gz(&archive, &[("README.md", b"readme", false)]);
        let asset = fake_asset(leak(sha256_file(&archive).unwrap()));
        let dest = tmp.path().join("bin").join("caddy");
        assert!(matches!(
            install_archive(&archive, &asset, &dest),
            Err(Error::Archive { .. })
        ));
        assert_eq!(fs::read_dir(dest.parent().unwrap()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn version_check_rejects_other_programs() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("caddy");
        for (body, ok) in [
            (format!("echo 'v{CADDY_VERSION} h1:abc'"), true),
            ("echo 'v2.0.0 h1:abc'".to_owned(), false),
            ("exit 1".to_owned(), false),
        ] {
            fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            set_mode(&script, FileMode::Executable).unwrap();
            assert_eq!(check_version(&script).is_ok(), ok, "{body}");
        }
    }
}
