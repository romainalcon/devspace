//! `devspace-helper`, the minimal privileged helper of DevSpace.
//!
//! Started by `devspace system setup|teardown` through `sudo` after the user
//! approved the change. Performs exactly one [`Operation`] given on the
//! command line, prints what it did and exits. It refuses to run without
//! administrator rights, never writes outside the files of the requested
//! integration, never touches a file that DevSpace does not manage, and only
//! trusts a certificate whose fingerprint matches the one on its command
//! line, which the user approved.

use std::fmt::Write as _;
use std::io::{self, Read as _};
use std::process::ExitCode;

use devspace_helper::{Certificate, CertificateError, Fingerprint, Operation, OperationError};
use devspace_platform::PlatformError;
use devspace_platform::fs::{FileMode, create_new};
use devspace_platform::system::port_forwarding::Forwarding;
use devspace_platform::system::{port_forwarding, resolver, trust_store};

/// Exit status for an invalid command line.
const USAGE_EXIT_CODE: u8 = 2;

#[derive(Debug, thiserror::Error)]
enum HelperError {
    #[error(transparent)]
    Usage(#[from] OperationError),

    #[error("devspace-helper must run with administrator rights (through sudo)")]
    NotElevated,

    #[error(transparent)]
    Platform(#[from] PlatformError),

    #[error("failed to read the certificate from standard input")]
    Stdin(#[source] io::Error),

    #[error("invalid certificate on standard input")]
    Certificate(#[source] CertificateError),

    #[error(
        "the certificate on standard input has fingerprint {actual}, not the approved {expected}"
    )]
    FingerprintMismatch {
        expected: Fingerprint,
        actual: Fingerprint,
    },

    #[error("failed to prepare a temporary copy of the certificate")]
    TempFile(#[source] io::Error),
}

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect()
    {
        Ok(args) => args,
        Err(_) => {
            eprintln!("devspace-helper: arguments must be valid Unicode");
            return ExitCode::from(USAGE_EXIT_CODE);
        }
    };
    match run(&args) {
        Ok(message) => {
            println!("devspace-helper: {message}");
            ExitCode::SUCCESS
        }
        Err(err @ HelperError::Usage(_)) => {
            eprintln!("devspace-helper: {err}\n{}", Operation::USAGE);
            ExitCode::from(USAGE_EXIT_CODE)
        }
        Err(err) => {
            eprintln!("devspace-helper: error: {}", report(&err));
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<String, HelperError> {
    let operation = Operation::from_args(args)?;
    if !devspace_platform::privileges::is_elevated()? {
        return Err(HelperError::NotElevated);
    }
    match operation {
        Operation::InstallResolver { tld, port } => {
            resolver::install(tld.as_str(), port)?;
            Ok(format!(
                "*.{tld} is now resolved by 127.0.0.1:{port} ({})",
                resolver::file(tld.as_str())?.display()
            ))
        }
        Operation::RemoveResolver { tld } => Ok(if resolver::remove(tld.as_str())? {
            format!("*.{tld} is no longer resolved by DevSpace")
        } else {
            "the resolver was not installed".to_owned()
        }),
        Operation::InstallPortForwarding {
            address,
            http_port,
            https_port,
        } => {
            port_forwarding::install(Forwarding::new(address.ip(), http_port, https_port)?)?;
            Ok(format!(
                "ports 80 and 443 of {address} are now forwarded to {http_port} and {https_port}"
            ))
        }
        Operation::RemovePortForwarding { address } => {
            Ok(if port_forwarding::remove(address.ip())? {
                format!("ports 80 and 443 of {address} are no longer forwarded")
            } else {
                "the port forwarding was not installed".to_owned()
            })
        }
        Operation::TrustCertificate { sha256 } => {
            let certificate = read_certificate(sha256)?;
            with_certificate_file(&certificate, trust_store::add)?;
            Ok(format!("certificate {sha256} is now trusted"))
        }
        Operation::UntrustCertificate { sha256 } => {
            let certificate = read_certificate(sha256)?;
            let hex = sha256.to_string();
            with_certificate_file(&certificate, |path| trust_store::remove(path, &hex))?;
            Ok(format!("certificate {sha256} is no longer trusted"))
        }
    }
}

/// Reads the PEM certificate from standard input and checks that it is the
/// one the user approved.
fn read_certificate(expected: Fingerprint) -> Result<Certificate, HelperError> {
    let mut text = String::new();
    let limit = u64::try_from(devspace_helper::certificate::MAX_PEM_LEN + 1).unwrap_or(u64::MAX);
    io::stdin()
        .take(limit)
        .read_to_string(&mut text)
        .map_err(HelperError::Stdin)?;
    let certificate = Certificate::from_pem(&text).map_err(HelperError::Certificate)?;
    let actual = certificate.fingerprint();
    if actual != expected {
        return Err(HelperError::FingerprintMismatch { expected, actual });
    }
    Ok(certificate)
}

/// Runs `action` on a copy of `certificate` written in a new directory only
/// accessible to the administrator, so that nobody can swap the file
/// between the fingerprint check and its use.
fn with_certificate_file(
    certificate: &Certificate,
    action: impl FnOnce(&std::path::Path) -> Result<(), PlatformError>,
) -> Result<(), HelperError> {
    let dir = tempfile::Builder::new()
        .prefix("devspace-helper-")
        .tempdir()
        .map_err(HelperError::TempFile)?;
    let path = dir.path().join("certificate.pem");
    let mut file = create_new(&path, FileMode::Private).map_err(HelperError::TempFile)?;
    io::Write::write_all(&mut file, certificate.to_pem().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(HelperError::TempFile)?;
    drop(file);
    action(&path)?;
    Ok(())
}

fn report(error: &(dyn std::error::Error + 'static)) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let _ = write!(out, "\n  caused by: {cause}");
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_errors_come_before_the_privilege_check() {
        let err = run(&["format-disk".to_owned()]).unwrap_err();
        assert!(matches!(err, HelperError::Usage(_)));
    }

    #[test]
    fn operations_require_elevation() {
        if devspace_platform::privileges::is_elevated().unwrap() {
            return;
        }
        let operation = Operation::RemoveResolver {
            tld: devspace_helper::Tld::default(),
        };
        let err = run(&operation.to_args()).unwrap_err();
        assert!(matches!(err, HelperError::NotElevated));
    }
}
