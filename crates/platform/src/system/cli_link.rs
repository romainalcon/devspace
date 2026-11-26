//! Command `devspace` in the `PATH` of every shell.
//!
//! macOS: a symbolic link `/usr/local/bin/devspace` to the CLI shipped in
//! the application bundle (`…/DevSpace.app/Contents/MacOS/devspace`). A
//! symbolic link has no room for the "Managed by DevSpace" marker, so a link
//! is DevSpace's only when it points to the CLI of an application bundle;
//! anything else at that path is never modified. Other platforms put the
//! CLI in the `PATH` with their installer and return
//! [`PlatformError::Unsupported`].

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::IntegrationState;
use crate::PlatformError;

/// Directory of the link (macOS).
const LINK_DIR: &str = "/usr/local/bin";

/// Name of the link and of the CLI.
const CLI_NAME: &str = "devspace";

/// Path of the link.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS.
pub fn link_path() -> Result<PathBuf, PlatformError> {
    ensure_supported()?;
    Ok(Path::new(LINK_DIR).join(CLI_NAME))
}

/// Whether `exe` is the CLI of an application bundle
/// (`/…/<name>.app/Contents/MacOS/devspace`, absolute, without `.` or
/// `..`): only such a CLI is linked.
#[must_use]
pub fn is_bundled_cli(exe: &Path) -> bool {
    if !exe.is_absolute()
        || exe
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return false;
    }
    let names: Vec<&OsStr> = exe.iter().collect();
    let [.., app, contents, macos, cli] = names.as_slice() else {
        return false;
    };
    *cli == CLI_NAME
        && *macos == "MacOS"
        && *contents == "Contents"
        && Path::new(app).extension().is_some_and(|ext| ext == "app")
}

/// State of the link compared with `target` (the running bundled CLI).
/// Does not need privileges.
///
/// # Errors
///
/// Those of [`link_path`], or [`PlatformError::SystemFile`] if the link
/// cannot be read.
pub fn state(target: &Path) -> Result<IntegrationState, PlatformError> {
    state_at(&link_path()?, target)
}

fn state_at(link: &Path, target: &Path) -> Result<IntegrationState, PlatformError> {
    Ok(match current_target(link)? {
        Current::Missing => IntegrationState::Missing,
        Current::Foreign => IntegrationState::Conflict,
        Current::Bundled(current) if current == target => IntegrationState::Installed,
        Current::Bundled(_) => IntegrationState::Outdated,
    })
}

enum Current {
    Missing,
    /// A file, a directory or a link to anything else than a bundled CLI.
    Foreign,
    Bundled(PathBuf),
}

fn current_target(link: &Path) -> Result<Current, PlatformError> {
    let failed = |source| PlatformError::SystemFile {
        path: link.to_path_buf(),
        source,
    };
    match fs::symlink_metadata(link) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Current::Missing),
        Err(err) => return Err(failed(err)),
        Ok(metadata) if !metadata.file_type().is_symlink() => return Ok(Current::Foreign),
        Ok(_) => {}
    }
    let target = fs::read_link(link).map_err(failed)?;
    Ok(if is_bundled_cli(&target) {
        Current::Bundled(target)
    } else {
        Current::Foreign
    })
}

/// Describes what [`install`] changes, for the approval prompt.
///
/// # Errors
///
/// Those of [`link_path`].
pub fn describe_install(target: &Path) -> Result<String, PlatformError> {
    Ok(format!(
        "make `devspace` available in every shell: link `{}` to `{}`",
        link_path()?.display(),
        target.display()
    ))
}

/// Describes what [`remove`] changes, for the approval prompt.
///
/// # Errors
///
/// Those of [`link_path`].
pub fn describe_remove() -> Result<String, PlatformError> {
    Ok(format!(
        "remove the `devspace` command: delete the link `{}`",
        link_path()?.display()
    ))
}

/// Creates or replaces the link so that it points to `target`, which must
/// be an existing bundled CLI ([`is_bundled_cli`]). Requires administrator
/// rights.
///
/// # Errors
///
/// Those of [`link_path`], [`PlatformError::InvalidArgument`] for another
/// target, [`PlatformError::ForeignSystemFile`] if the path is used by
/// something else, or [`PlatformError::SystemFile`].
pub fn install(target: &Path) -> Result<(), PlatformError> {
    install_at(&link_path()?, target)
}

fn install_at(link: &Path, target: &Path) -> Result<(), PlatformError> {
    if !is_bundled_cli(target) || !target.is_file() {
        return Err(PlatformError::InvalidArgument {
            detail: format!(
                "`{}` is not the `devspace` program of an application bundle",
                target.display()
            ),
        });
    }
    if matches!(current_target(link)?, Current::Foreign) {
        return Err(PlatformError::ForeignSystemFile {
            path: link.to_path_buf(),
        });
    }
    let failed = |path: &Path| {
        let path = path.to_path_buf();
        move |source| PlatformError::SystemFile { path, source }
    };
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir).map_err(failed(dir))?;
    }
    // A new link renamed over the old one: never a moment without a link.
    let temp = crate::fs::temporary_sibling(link).map_err(failed(link))?;
    let _ = fs::remove_file(&temp);
    crate::fs::symlink(target, &temp).map_err(failed(&temp))?;
    fs::rename(&temp, link).map_err(|source| {
        let _ = fs::remove_file(&temp);
        PlatformError::SystemFile {
            path: link.to_path_buf(),
            source,
        }
    })
}

/// Removes the link if it is DevSpace's; returns `false` if it was absent.
/// Requires administrator rights.
///
/// # Errors
///
/// Those of [`link_path`], [`PlatformError::ForeignSystemFile`] if the
/// path is used by something else, or [`PlatformError::SystemFile`].
pub fn remove() -> Result<bool, PlatformError> {
    remove_at(&link_path()?)
}

fn remove_at(link: &Path) -> Result<bool, PlatformError> {
    match current_target(link)? {
        Current::Missing => Ok(false),
        Current::Foreign => Err(PlatformError::ForeignSystemFile {
            path: link.to_path_buf(),
        }),
        Current::Bundled(_) => {
            fs::remove_file(link).map_err(|source| PlatformError::SystemFile {
                path: link.to_path_buf(),
                source,
            })?;
            Ok(true)
        }
    }
}

fn ensure_supported() -> Result<(), PlatformError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(PlatformError::Unsupported {
            feature: "linking the `devspace` command",
        })
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn bundle(root: &Path, app: &str) -> PathBuf {
        let dir = root.join(app).join("Contents").join("MacOS");
        fs::create_dir_all(&dir).unwrap();
        let cli = dir.join("devspace");
        fs::write(&cli, b"cli").unwrap();
        cli
    }

    #[test]
    fn only_the_cli_of_a_bundle_is_linked() {
        assert!(is_bundled_cli(Path::new(
            "/Applications/DevSpace.app/Contents/MacOS/devspace"
        )));
        for other in [
            "Applications/DevSpace.app/Contents/MacOS/devspace",
            "/Applications/DevSpace.app/Contents/MacOS/devspaced",
            "/Applications/DevSpace/Contents/MacOS/devspace",
            "/Applications/DevSpace.app/Contents/../MacOS/devspace",
            "/usr/bin/devspace",
            "/devspace",
        ] {
            assert!(!is_bundled_cli(Path::new(other)), "{other}");
        }
    }

    #[test]
    fn link_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let first = bundle(tmp.path(), "DevSpace.app");
        let moved = bundle(&tmp.path().join("Applications"), "DevSpace.app");
        let link = tmp.path().join("bin").join("devspace");

        assert_eq!(state_at(&link, &first).unwrap(), IntegrationState::Missing);
        assert!(!remove_at(&link).unwrap());
        install_at(&link, &first).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), first);
        assert_eq!(
            state_at(&link, &first).unwrap(),
            IntegrationState::Installed
        );
        assert_eq!(state_at(&link, &moved).unwrap(), IntegrationState::Outdated);
        install_at(&link, &moved).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), moved);
        assert!(remove_at(&link).unwrap());
        assert!(fs::symlink_metadata(&link).is_err());
    }

    #[test]
    fn foreign_files_and_targets_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = bundle(tmp.path(), "DevSpace.app");
        let link = tmp.path().join("devspace");

        let other = tmp.path().join("other");
        fs::write(&other, b"x").unwrap();
        assert!(matches!(
            install_at(&link, &other),
            Err(PlatformError::InvalidArgument { .. })
        ));

        crate::fs::symlink(&other, &link).unwrap();
        assert_eq!(state_at(&link, &cli).unwrap(), IntegrationState::Conflict);
        assert!(matches!(
            install_at(&link, &cli),
            Err(PlatformError::ForeignSystemFile { .. })
        ));
        assert!(matches!(
            remove_at(&link),
            Err(PlatformError::ForeignSystemFile { .. })
        ));
        assert_eq!(fs::read_link(&link).unwrap(), other);

        fs::remove_file(&link).unwrap();
        fs::write(&link, b"#!/bin/sh\n").unwrap();
        assert_eq!(state_at(&link, &cli).unwrap(), IntegrationState::Conflict);
    }
}
