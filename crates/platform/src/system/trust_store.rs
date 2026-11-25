//! Machine-wide trust of the DevSpace local certificate authority.
//!
//! macOS: the certificate is added to the System keychain with an
//! administrator trust setting for SSL (`security add-trusted-cert -d`).

use std::path::Path;

use super::command;
use crate::PlatformError;

const SECURITY: &str = "/usr/bin/security";
const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

/// Whether the certificate in `cert_file` (PEM) is a trusted root for SSL
/// for the current user. Does not need privileges.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS, or
/// [`PlatformError::SystemCommand`] if the trust cannot be evaluated.
pub fn is_trusted(cert_file: &Path) -> Result<bool, PlatformError> {
    ensure_supported()?;
    // Exit status 0: trusted; other statuses mean that evaluation failed,
    // for example CSSMERR_TP_NOT_TRUSTED.
    let output = command::output(
        SECURITY,
        &[
            "verify-cert".as_ref(),
            "-c".as_ref(),
            cert_file.as_os_str(),
            // The trust setting is added for the SSL policy only.
            "-p".as_ref(),
            "ssl".as_ref(),
            "-L".as_ref(),
            "-l".as_ref(),
        ],
    )?;
    Ok(output.status.success())
}

/// Describes what [`add`] changes, for the approval prompt.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS.
pub fn describe_add(sha256: &str) -> Result<String, PlatformError> {
    ensure_supported()?;
    Ok(format!(
        "trust the DevSpace local certificate authority (SHA-256 {sha256}) for HTTPS: add it \
         to `{SYSTEM_KEYCHAIN}`"
    ))
}

/// Describes what [`remove`] changes, for the approval prompt.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS.
pub fn describe_remove(sha256: &str) -> Result<String, PlatformError> {
    ensure_supported()?;
    Ok(format!(
        "stop trusting the DevSpace local certificate authority (SHA-256 {sha256}): delete it \
         from `{SYSTEM_KEYCHAIN}`"
    ))
}

/// Trusts the certificate in `cert_file` (PEM) machine-wide. Requires
/// administrator rights. The caller must have verified the certificate.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS, or
/// [`PlatformError::SystemCommand`].
pub fn add(cert_file: &Path) -> Result<(), PlatformError> {
    ensure_supported()?;
    command::run(
        SECURITY,
        &[
            "add-trusted-cert".as_ref(),
            "-d".as_ref(),
            "-r".as_ref(),
            "trustRoot".as_ref(),
            "-p".as_ref(),
            "ssl".as_ref(),
            "-k".as_ref(),
            SYSTEM_KEYCHAIN.as_ref(),
            cert_file.as_os_str(),
        ],
    )?;
    Ok(())
}

/// Removes the trust setting of the certificate in `cert_file` and deletes
/// it, identified by its SHA-256 fingerprint (64 hexadecimal digits), from
/// the System keychain. Requires administrator rights.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS,
/// [`PlatformError::InvalidArgument`] for a malformed fingerprint, or
/// [`PlatformError::SystemCommand`].
pub fn remove(cert_file: &Path, sha256: &str) -> Result<(), PlatformError> {
    ensure_supported()?;
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PlatformError::InvalidArgument {
            detail: format!("`{sha256}` is not a SHA-256 fingerprint"),
        });
    }
    // Fails when no trust setting exists, which is fine: the certificate
    // may have been distrusted by hand.
    let _ = command::output(
        SECURITY,
        &[
            "remove-trusted-cert".as_ref(),
            "-d".as_ref(),
            cert_file.as_os_str(),
        ],
    );
    command::run(
        SECURITY,
        &["delete-certificate", "-Z", sha256, SYSTEM_KEYCHAIN],
    )?;
    Ok(())
}

/// Whether this platform supports trusting the local certificate
/// authority.
#[must_use]
pub const fn is_supported() -> bool {
    cfg!(target_os = "macos")
}

fn ensure_supported() -> Result<(), PlatformError> {
    if is_supported() {
        Ok(())
    } else {
        Err(PlatformError::Unsupported {
            feature: "trusting the local certificate authority",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_fingerprints_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let cert = tmp.path().join("root.crt");
        for bad in ["", "abc", &"g".repeat(64), &"a".repeat(63)] {
            let err = remove(&cert, bad).unwrap_err();
            assert!(
                matches!(
                    err,
                    PlatformError::InvalidArgument { .. } | PlatformError::Unsupported { .. }
                ),
                "{bad}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unknown_certificate_is_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let cert = tmp.path().join("root.crt");
        std::fs::write(&cert, "not a certificate").unwrap();
        assert!(!is_trusted(&cert).unwrap());
    }
}
