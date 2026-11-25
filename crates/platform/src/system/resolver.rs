//! Resolution of a local top-level domain by the DevSpace DNS server.
//!
//! macOS: a resolver file `/etc/resolver/<tld>` sends every query for
//! `*.<tld>` to `127.0.0.1:<port>` (see `man 5 resolver`).

use std::path::{Path, PathBuf};

use super::{IntegrationState, MANAGED_MARKER, managed};
use crate::PlatformError;

/// Directory of the per-domain resolver files (macOS).
const RESOLVER_DIR: &str = "/etc/resolver";

/// Content of the resolver file for a DNS server on `127.0.0.1:port`.
#[must_use]
pub fn contents(port: u16) -> String {
    format!(
        "# {MANAGED_MARKER}. Removed by `devspace system teardown`.\n\
         nameserver 127.0.0.1\n\
         port {port}\n"
    )
}

/// Validates a top-level domain: 1 to 63 lowercase ASCII letters. This
/// also guarantees that it is a plain file name.
fn check_tld(tld: &str) -> Result<(), PlatformError> {
    if tld.is_empty() || tld.len() > 63 || !tld.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(PlatformError::InvalidArgument {
            detail: format!("`{tld}` is not a valid top-level domain"),
        });
    }
    Ok(())
}

fn file_in(dir: &Path, tld: &str) -> Result<PathBuf, PlatformError> {
    check_tld(tld)?;
    Ok(dir.join(tld))
}

/// Path of the resolver file for `tld`.
///
/// # Errors
///
/// [`PlatformError::InvalidArgument`] for an invalid domain,
/// [`PlatformError::Unsupported`] outside macOS.
pub fn file(tld: &str) -> Result<PathBuf, PlatformError> {
    ensure_supported()?;
    file_in(Path::new(RESOLVER_DIR), tld)
}

/// Whether `*.<tld>` is resolved by a DNS server on `127.0.0.1:port`.
/// Does not need privileges.
///
/// # Errors
///
/// Those of [`file`], or [`PlatformError::SystemFile`] if the file cannot
/// be read.
pub fn state(tld: &str, port: u16) -> Result<IntegrationState, PlatformError> {
    managed::state(&file(tld)?, &contents(port))
}

/// Describes what [`install`] changes, for the approval prompt.
///
/// # Errors
///
/// Those of [`file`].
pub fn describe_install(tld: &str, port: u16) -> Result<String, PlatformError> {
    Ok(format!(
        "resolve *.{tld} with the DevSpace DNS server: write `{}` \
         (nameserver 127.0.0.1, port {port})",
        file(tld)?.display()
    ))
}

/// Describes what [`remove`] changes, for the approval prompt.
///
/// # Errors
///
/// Those of [`file`].
pub fn describe_remove(tld: &str) -> Result<String, PlatformError> {
    Ok(format!(
        "stop resolving *.{tld} with DevSpace: delete `{}`",
        file(tld)?.display()
    ))
}

/// Installs or updates the resolver file. Requires administrator rights.
///
/// # Errors
///
/// Those of [`file`], [`PlatformError::ForeignSystemFile`] if the file
/// exists without the DevSpace marker, or [`PlatformError::SystemFile`].
pub fn install(tld: &str, port: u16) -> Result<(), PlatformError> {
    let path = file(tld)?;
    managed::ensure_dir(Path::new(RESOLVER_DIR))?;
    managed::install(&path, &contents(port))
}

/// Removes the resolver file; returns `false` if it was absent. Requires
/// administrator rights.
///
/// # Errors
///
/// Those of [`file`], [`PlatformError::ForeignSystemFile`] or
/// [`PlatformError::SystemFile`].
pub fn remove(tld: &str) -> Result<bool, PlatformError> {
    managed::remove(&file(tld)?)
}

fn ensure_supported() -> Result<(), PlatformError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(PlatformError::Unsupported {
            feature: "local domain resolution",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_point_to_loopback_port() {
        let text = contents(47053);
        assert!(text.contains(MANAGED_MARKER));
        assert!(text.contains("\nnameserver 127.0.0.1\n"));
        assert!(text.ends_with("\nport 47053\n"));
    }

    #[test]
    fn tld_must_be_a_plain_label() {
        let dir = Path::new("/etc/resolver");
        assert_eq!(file_in(dir, "test").unwrap(), dir.join("test"));
        for invalid in ["", "../sudoers", "a/b", "Test", "te.st", "t3st"] {
            assert!(
                matches!(
                    file_in(dir, invalid),
                    Err(PlatformError::InvalidArgument { .. })
                ),
                "{invalid}"
            );
        }
    }

    #[test]
    fn support_matches_platform() {
        assert_eq!(file("test").is_ok(), cfg!(target_os = "macos"));
    }
}
