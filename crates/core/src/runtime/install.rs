//! Installation and removal of runtime versions.
//!
//! An installation is built in a private staging directory next to the
//! installations: every archive of the package is downloaded over HTTPS
//! (at most its declared size), checked against its SHA-256 from the
//! signed index, then extracted with [`extract_tree`]. The programs are run
//! once to check that they report the expected version, the receipt is
//! written, and the directory is renamed into place: an installation
//! appears complete or not at all. A failure removes the staging
//! directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devspace_platform::paths::OsFamily;
use devspace_platform::target::Target;

use super::index::{Index, Package, PackageArchive};
use super::{
    InstalledRuntime, RECEIPT_FILE_NAME, RECEIPT_SCHEMA_VERSION, Receipt, ReceiptArchive,
    RuntimeLayout, RuntimeName, Version, VersionReq, load_installed,
};
use crate::archive::{TreeLimits, extract_tree};
use crate::download::{self, verify_sha256};
use crate::{Error, Result};

/// Time limit of a version check.
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Newest version of `name` in `index` for `target` matching `req` (any
/// version when `None`).
///
/// # Errors
///
/// [`Error::RuntimeUnavailable`] when none matches.
pub fn resolve(
    index: &Index,
    name: RuntimeName,
    req: Option<VersionReq>,
    target: Target,
) -> Result<Version> {
    let versions = index.versions(name, target);
    let best = match req {
        Some(req) => req.best(versions.iter().copied()),
        None => versions.last().copied(),
    };
    best.ok_or_else(|| Error::RuntimeUnavailable {
        name,
        requirement: req.map_or_else(|| "any version".to_owned(), |req| req.to_string()),
        available: versions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// The installation of `name` `version`, if complete.
#[must_use]
pub fn installed(
    layout: &RuntimeLayout,
    name: RuntimeName,
    version: Version,
) -> Option<InstalledRuntime> {
    load_installed(&layout.version_dir(name, version), name, version)
}

/// Installs `name` `version` from `index` unless it is installed. Returns
/// the installation and whether this call created it. Blocking; may take
/// minutes.
///
/// # Errors
///
/// [`Error::RuntimeUnavailable`] if the index has no package for this
/// platform; download, checksum, archive, version check and file errors
/// otherwise. Nothing is left behind on failure.
pub fn install(
    layout: &RuntimeLayout,
    index: &Index,
    name: RuntimeName,
    version: Version,
) -> Result<(InstalledRuntime, bool)> {
    install_with(layout, index, name, version, &|archive, dest| {
        download::fetch(archive.url.as_str(), dest, archive.size)
    })
}

/// Receives an archive of the index into a local file.
type Fetch<'a> = &'a dyn Fn(&PackageArchive, &Path) -> Result<()>;

/// [`install`] with a custom way of receiving the archives (tests).
fn install_with(
    layout: &RuntimeLayout,
    index: &Index,
    name: RuntimeName,
    version: Version,
    fetch: Fetch<'_>,
) -> Result<(InstalledRuntime, bool)> {
    if let Some(runtime) = installed(layout, name, version) {
        return Ok((runtime, false));
    }
    let target = Target::current().ok_or_else(|| Error::RuntimeUnavailable {
        name,
        requirement: version.to_string(),
        available: String::new(),
    })?;
    let package =
        index
            .package(name, version, target)
            .ok_or_else(|| Error::RuntimeUnavailable {
                name,
                requirement: version.to_string(),
                available: index
                    .versions(name, target)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;

    let staging = layout
        .staging_dir()
        .join(format!("{name}-{version}-{}", std::process::id()));
    remove_dir_if_exists(&staging)?;
    create_dir(&staging)?;
    create_dir(&layout.download_dir)?;
    let result = build(layout, index, package, name, version, &staging, fetch).and_then(|()| {
        let dest = layout.version_dir(name, version);
        if let Some(parent) = dest.parent() {
            create_dir(parent)?;
        }
        // A directory without a valid receipt is a leftover, never an
        // installation in use.
        remove_dir_if_exists(&dest)?;
        fs::rename(&staging, &dest).map_err(install_error(&dest))?;
        installed(layout, name, version).ok_or_else(|| Error::InstallFile {
            path: dest.clone(),
            source: io::Error::other("the installation cannot be read back"),
        })
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    let runtime = result?;
    tracing::info!(runtime = %name, %version, dir = %runtime.dir.display(), "runtime installed");
    Ok((runtime, true))
}

/// Downloads, verifies and extracts every archive into `staging`, checks
/// the programs and writes the receipt.
fn build(
    layout: &RuntimeLayout,
    index: &Index,
    package: &Package,
    name: RuntimeName,
    version: Version,
    staging: &Path,
    fetch: Fetch<'_>,
) -> Result<()> {
    let mut archives = Vec::new();
    for archive in &package.archives {
        let file_name = archive.file_name();
        let part = layout
            .download_dir
            .join(format!("{name}-{version}-{file_name}.part"));
        let url = archive.url.as_str();
        tracing::info!(runtime = %name, %version, %url, size = archive.size, "downloading");
        let result = fetch(archive, &part)
            .and_then(|()| verify_sha256(&part, file_name, archive.sha256.as_str()))
            .and_then(|()| {
                let dest = archive
                    .destination
                    .as_ref()
                    .map_or_else(|| staging.to_path_buf(), |dir| dir.under(staging));
                create_dir(&dest)?;
                extract_tree(
                    &part,
                    archive.format.into(),
                    &dest,
                    archive.strip_components,
                    TreeLimits::default(),
                )
                .map_err(|source| Error::Archive {
                    path: part.clone(),
                    source,
                })
            });
        let _ = fs::remove_file(&part);
        let stats = result?;
        tracing::debug!(
            files = stats.files,
            links = stats.links,
            bytes = stats.bytes,
            "extracted"
        );
        archives.push(ReceiptArchive {
            url: url.to_owned(),
            sha256: archive.sha256.as_str().to_owned(),
        });
    }

    let bin_dirs: Vec<PathBuf> = package.bin.iter().map(|dir| dir.under(staging)).collect();
    for check in version_checks(name, OsFamily::current()) {
        let file = devspace_platform::target::executable_name(check.program);
        let program = bin_dirs
            .iter()
            .map(|dir| dir.join(&file))
            .find(|path| path.is_file())
            .ok_or_else(|| Error::RuntimeCheck {
                name,
                version,
                detail: format!("the package does not contain `{file}`"),
            })?;
        check_version(&program, check, name, version)?;
    }

    let receipt = Receipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        name,
        version,
        bin: package
            .bin
            .iter()
            .map(|dir| dir.as_str().to_owned())
            .collect(),
        archives,
        index_serial: index.serial,
        installed_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    };
    let text = serde_json::to_vec_pretty(&receipt).map_err(|err| Error::InstallFile {
        path: staging.join(RECEIPT_FILE_NAME),
        source: io::Error::other(err),
    })?;
    let receipt_path = staging.join(RECEIPT_FILE_NAME);
    fs::write(&receipt_path, text).map_err(install_error(&receipt_path))
}

/// A program of a runtime and how it reports its version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionCheck {
    /// Program name without the `.exe` suffix.
    pub program: &'static str,
    /// Argument printing the version.
    pub arg: &'static str,
    /// Expected start of the output, before the version number.
    pub prefix: &'static str,
    /// Expected text right after the version number.
    pub suffix: &'static str,
}

/// Programs every installation of `name` must provide on `os`.
#[must_use]
pub fn version_checks(name: RuntimeName, os: OsFamily) -> Vec<VersionCheck> {
    match name {
        RuntimeName::Node => vec![VersionCheck {
            program: "node",
            arg: "--version",
            prefix: "v",
            suffix: "",
        }],
        RuntimeName::Php => {
            let cli = VersionCheck {
                program: "php",
                arg: "-v",
                prefix: "PHP ",
                suffix: " (cli)",
            };
            // Windows has no FastCGI process manager; PHP serving is not
            // supported there yet, so only the CLI is required.
            if os == OsFamily::Windows {
                vec![cli]
            } else {
                vec![
                    cli,
                    VersionCheck {
                        program: "php-fpm",
                        arg: "-v",
                        prefix: "PHP ",
                        suffix: " (fpm-fcgi)",
                    },
                ]
            }
        }
    }
}

/// Runs `program` and checks the version it reports.
fn check_version(
    program: &Path,
    check: VersionCheck,
    name: RuntimeName,
    version: Version,
) -> Result<()> {
    let failure = |detail: String| Error::RuntimeCheck {
        name,
        version,
        detail,
    };
    let output = program_output(program, &[check.arg], VERSION_CHECK_TIMEOUT)
        .map_err(|detail| failure(format!("`{}` {detail}", program.display())))?;
    let expected = format!("{}{version}{}", check.prefix, check.suffix);
    let first_line = output.lines().next().unwrap_or_default().trim();
    let matches = first_line.starts_with(&expected)
        && first_line[expected.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_ascii_digit() && next != '.');
    if matches {
        Ok(())
    } else {
        Err(failure(format!(
            "`{} {}` printed `{first_line}`, expected `{expected}`",
            program.display(),
            check.arg
        )))
    }
}

/// Runs `program` with `args`, without input, and returns its standard
/// output if it succeeds within `timeout`.
///
/// # Errors
///
/// Returns a description of the failure.
pub(crate) fn program_output(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> std::result::Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("cannot be run: {err}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("did not answer within {}s", timeout.as_secs()));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    let output = child.wait_with_output().map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!("failed ({})", output.status))
    }
}

/// Removes an installed version. The directory first moves to the staging
/// area, so the version disappears at once even if deleting its files
/// takes time or fails half-way.
///
/// # Errors
///
/// [`Error::RuntimeNotInstalled`] or a file error.
pub fn remove(
    layout: &RuntimeLayout,
    name: RuntimeName,
    version: Version,
) -> Result<InstalledRuntime> {
    let runtime =
        installed(layout, name, version).ok_or(Error::RuntimeNotInstalled { name, version })?;
    create_dir(&layout.staging_dir())?;
    let trash = layout
        .staging_dir()
        .join(format!("removed-{name}-{version}-{}", std::process::id()));
    remove_dir_if_exists(&trash)?;
    fs::rename(&runtime.dir, &trash).map_err(install_error(&runtime.dir))?;
    if let Err(err) = fs::remove_dir_all(&trash) {
        tracing::warn!(dir = %trash.display(), error = %err, "failed to delete a removed runtime; it is deleted at the next daemon start");
    }
    tracing::info!(runtime = %name, %version, "runtime removed");
    Ok(runtime)
}

/// Deletes what interrupted installations and removals left in the
/// staging area. Called by the daemon at startup, before any installation.
pub fn clean_staging(layout: &RuntimeLayout) {
    let staging = layout.staging_dir();
    if staging.exists()
        && let Err(err) = fs::remove_dir_all(&staging)
    {
        tracing::warn!(dir = %staging.display(), error = %err, "failed to clean the runtime staging directory");
    }
}

fn remove_dir_if_exists(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::InstallFile {
            path: dir.to_path_buf(),
            source: err,
        }),
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
    use crate::AppPaths;
    use crate::runtime::index::tests::{SAMPLE, TestSigner};
    use crate::runtime::list_installed;

    fn index() -> Index {
        let signer = TestSigner::new();
        let text = SAMPLE.replace(
            "\"macos-aarch64\"",
            &format!(
                "\"{}\"",
                super::super::index::platform_key(Target::current().unwrap())
            ),
        );
        Index::verify(
            text.as_bytes(),
            &signer.sign(text.as_bytes()),
            &signer.public_key(),
        )
        .unwrap()
    }

    #[test]
    fn versions_resolve_against_the_index() {
        let index = index();
        let target = Target::current().unwrap();
        let v = Version::parse("8.4.23").unwrap();
        assert_eq!(resolve(&index, RuntimeName::Php, None, target).unwrap(), v);
        assert_eq!(
            resolve(
                &index,
                RuntimeName::Php,
                Some(VersionReq::parse("8").unwrap()),
                target
            )
            .unwrap(),
            v
        );
        let err = resolve(
            &index,
            RuntimeName::Php,
            Some(VersionReq::parse("7.4").unwrap()),
            target,
        )
        .unwrap_err();
        assert!(err.to_string().contains("7.4"), "{err}");
        assert!(err.to_string().contains("8.4.23"), "{err}");
        assert!(resolve(&index, RuntimeName::Node, None, target).is_err());
    }

    /// Index of one PHP package whose archive is `archive`.
    fn local_index(archive: &Path) -> Index {
        let sha256 = crate::download::sha256_file(archive).unwrap();
        let size = fs::metadata(archive).unwrap().len();
        let text = SAMPLE
            .replace(
                "\"macos-aarch64\"",
                &format!(
                    "\"{}\"",
                    super::super::index::platform_key(Target::current().unwrap())
                ),
            )
            .replace(&"0".repeat(64), &sha256)
            .replace("\"size\": 10", &format!("\"size\": {size}"));
        let signer = TestSigner::new();
        Index::verify(
            text.as_bytes(),
            &signer.sign(text.as_bytes()),
            &signer.public_key(),
        )
        .unwrap()
    }

    fn copy_from(source: PathBuf) -> impl Fn(&PackageArchive, &Path) -> Result<()> {
        move |_, dest| {
            fs::copy(&source, dest).map_err(install_error(dest))?;
            Ok(())
        }
    }

    fn assert_no_leftovers(layout: &RuntimeLayout) {
        let leftovers: Vec<_> = fs::read_dir(layout.staging_dir()).unwrap().collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let downloads: Vec<_> = fs::read_dir(&layout.download_dir).unwrap().collect();
        assert!(downloads.is_empty(), "{downloads:?}");
    }

    #[cfg(unix)]
    #[test]
    fn verified_package_is_installed_atomically_and_removed() {
        use crate::archive::tests::tar_gz;
        let tmp = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(&AppPaths::from_root(&tmp.path().join("home")));
        let archive = tmp.path().join("php.tar.gz");
        tar_gz(
            &archive,
            &[
                (
                    "php",
                    b"#!/bin/sh\necho 'PHP 8.4.23 (cli) (built: today)'\n",
                    false,
                ),
                (
                    "php-fpm",
                    b"#!/bin/sh\necho 'PHP 8.4.23 (fpm-fcgi)'\n",
                    false,
                ),
            ],
        );
        let index = local_index(&archive);
        let version = Version::parse("8.4.23").unwrap();
        let fetch = copy_from(archive.clone());
        let (runtime, created) =
            install_with(&layout, &index, RuntimeName::Php, version, &fetch).unwrap();
        assert!(created);
        assert_eq!(runtime.dir, layout.version_dir(RuntimeName::Php, version));
        assert_eq!(
            runtime.program("php-fpm"),
            Some(runtime.dir.join("bin/php-fpm"))
        );
        assert_eq!(list_installed(&layout), std::slice::from_ref(&runtime));
        assert_no_leftovers(&layout);
        let receipt: Receipt =
            serde_json::from_slice(&fs::read(runtime.dir.join(RECEIPT_FILE_NAME)).unwrap())
                .unwrap();
        assert_eq!(receipt.index_serial, 7);
        assert_eq!(receipt.archives.len(), 1);

        let (_, created) =
            install_with(&layout, &index, RuntimeName::Php, version, &fetch).unwrap();
        assert!(!created, "already installed");

        remove(&layout, RuntimeName::Php, version).unwrap();
        assert!(list_installed(&layout).is_empty());
        assert!(!runtime.dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn wrong_version_or_tampered_archive_leaves_nothing() {
        use crate::archive::tests::tar_gz;
        let tmp = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(&AppPaths::from_root(&tmp.path().join("home")));
        let version = Version::parse("8.4.23").unwrap();

        let archive = tmp.path().join("php.tar.gz");
        tar_gz(
            &archive,
            &[
                ("php", b"#!/bin/sh\necho 'PHP 8.3.0 (cli)'\n", false),
                (
                    "php-fpm",
                    b"#!/bin/sh\necho 'PHP 8.3.0 (fpm-fcgi)'\n",
                    false,
                ),
            ],
        );
        let index = local_index(&archive);
        let err = install_with(
            &layout,
            &index,
            RuntimeName::Php,
            version,
            &copy_from(archive.clone()),
        )
        .unwrap_err();
        assert!(matches!(err, Error::RuntimeCheck { .. }), "{err}");
        assert_no_leftovers(&layout);
        assert!(!layout.version_dir(RuntimeName::Php, version).exists());

        let other = tmp.path().join("other.tar.gz");
        tar_gz(&other, &[("php", b"evil", false)]);
        let err = install_with(
            &layout,
            &index,
            RuntimeName::Php,
            version,
            &copy_from(other),
        )
        .unwrap_err();
        assert!(matches!(err, Error::ChecksumMismatch { .. }), "{err}");
        assert_no_leftovers(&layout);
        assert!(list_installed(&layout).is_empty());
    }

    #[test]
    fn missing_package_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(&AppPaths::from_root(tmp.path()));
        let err = install(
            &layout,
            &index(),
            RuntimeName::Node,
            Version::parse("24.0.0").unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::RuntimeUnavailable { .. }), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn version_output_is_matched_exactly() {
        use devspace_platform::fs::{FileMode, set_mode};
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("php");
        let v = Version::parse("8.4.2").unwrap();
        let check = version_checks(RuntimeName::Php, OsFamily::MacOs)[0];
        for (output, ok) in [
            ("PHP 8.4.2 (cli) (built: Jul  7 2026)", true),
            ("PHP 8.4.23 (cli)", false),
            ("PHP 8.4.2 (fpm-fcgi)", false),
            ("PHP 8.5.0 (cli)", false),
        ] {
            fs::write(&script, format!("#!/bin/sh\necho '{output}'\n")).unwrap();
            set_mode(&script, FileMode::Executable).unwrap();
            assert_eq!(
                check_version(&script, check, RuntimeName::Php, v).is_ok(),
                ok,
                "{output}"
            );
        }
        fs::write(&script, "#!/bin/sh\nexit 3\n").unwrap();
        assert!(check_version(&script, check, RuntimeName::Php, v).is_err());
    }

    #[test]
    fn php_requires_the_process_manager_except_on_windows() {
        let programs = |os| -> Vec<&str> {
            version_checks(RuntimeName::Php, os)
                .iter()
                .map(|check| check.program)
                .collect()
        };
        assert_eq!(programs(OsFamily::MacOs), ["php", "php-fpm"]);
        assert_eq!(programs(OsFamily::Linux), ["php", "php-fpm"]);
        assert_eq!(programs(OsFamily::Windows), ["php"]);
    }

    #[test]
    fn removal_requires_an_installation() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(&AppPaths::from_root(tmp.path()));
        assert!(matches!(
            remove(&layout, RuntimeName::Php, Version::parse("8.4.23").unwrap()),
            Err(Error::RuntimeNotInstalled { .. })
        ));
    }
}
