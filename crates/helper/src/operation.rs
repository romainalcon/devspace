//! Operations of the privileged helper and their command-line encoding.
//!
//! ```text
//! devspace-helper install-resolver --tld <TLD> --port <PORT>
//! devspace-helper remove-resolver --tld <TLD>
//! devspace-helper install-port-forwarding --address <IP> --http-port <PORT> --https-port <PORT>
//! devspace-helper remove-port-forwarding --address <IP>
//! devspace-helper trust-certificate --sha256 <FINGERPRINT>    (PEM on stdin)
//! devspace-helper untrust-certificate --sha256 <FINGERPRINT>  (PEM on stdin)
//! devspace-helper install-helper --sha256 <SHA-256>
//! devspace-helper remove-helper
//! devspace-helper link-cli --target <PATH>
//! devspace-helper unlink-cli
//! ```
//!
//! The encoding is strict: one operation, its options in this order, no
//! other argument.

use std::fmt;
use std::path::PathBuf;

use crate::certificate::Fingerprint;
use crate::local::{LoopbackAddress, Tld};

/// One privileged change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Resolve `*.<tld>` with the DevSpace DNS server on `127.0.0.1:port`.
    InstallResolver {
        /// Top-level domain of the sites.
        tld: Tld,
        /// Port of the DNS server.
        port: u16,
    },
    /// Stop resolving `*.<tld>` with DevSpace.
    RemoveResolver {
        /// Top-level domain of the sites.
        tld: Tld,
    },
    /// Configure `address` on the loopback interface (when needed) and
    /// forward its ports 80 and 443 to the web server ports.
    InstallPortForwarding {
        /// Loopback address of the sites.
        address: LoopbackAddress,
        /// Target of port 80.
        http_port: u16,
        /// Target of port 443.
        https_port: u16,
    },
    /// Remove the port forwarding and the loopback alias of `address`.
    RemovePortForwarding {
        /// Loopback address of the sites.
        address: LoopbackAddress,
    },
    /// Trust the certificate given on standard input, which must have this
    /// fingerprint.
    TrustCertificate {
        /// Fingerprint approved by the user.
        sha256: Fingerprint,
    },
    /// Stop trusting the certificate given on standard input, which must
    /// have this fingerprint.
    UntrustCertificate {
        /// Fingerprint approved by the user.
        sha256: Fingerprint,
    },
    /// Copy the running helper, whose SHA-256 must be this one, to the
    /// administrator-only location of the installed helper.
    InstallHelper {
        /// SHA-256 of the helper approved by the user.
        sha256: Fingerprint,
    },
    /// Remove the installed helper.
    RemoveHelper,
    /// Make `devspace` available in every shell by linking it to `target`,
    /// the CLI of an application bundle.
    LinkCli {
        /// Absolute path of the bundled CLI.
        target: PathBuf,
    },
    /// Remove the link created by [`Operation::LinkCli`].
    UnlinkCli,
}

/// Malformed helper command line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid arguments: {0}; see the usage of devspace-helper")]
pub struct OperationError(String);

impl Operation {
    /// Usage text listing every operation.
    pub const USAGE: &'static str = "usage:
  devspace-helper install-resolver --tld <TLD> --port <PORT>
  devspace-helper remove-resolver --tld <TLD>
  devspace-helper install-port-forwarding --address <IP> --http-port <PORT> --https-port <PORT>
  devspace-helper remove-port-forwarding --address <IP>
  devspace-helper trust-certificate --sha256 <FINGERPRINT>    (PEM certificate on stdin)
  devspace-helper untrust-certificate --sha256 <FINGERPRINT>  (PEM certificate on stdin)
  devspace-helper install-helper --sha256 <SHA-256>
  devspace-helper remove-helper
  devspace-helper link-cli --target <PATH>
  devspace-helper unlink-cli";

    /// Name of the operation on the command line.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::InstallResolver { .. } => "install-resolver",
            Self::RemoveResolver { .. } => "remove-resolver",
            Self::InstallPortForwarding { .. } => "install-port-forwarding",
            Self::RemovePortForwarding { .. } => "remove-port-forwarding",
            Self::TrustCertificate { .. } => "trust-certificate",
            Self::UntrustCertificate { .. } => "untrust-certificate",
            Self::InstallHelper { .. } => "install-helper",
            Self::RemoveHelper => "remove-helper",
            Self::LinkCli { .. } => "link-cli",
            Self::UnlinkCli => "unlink-cli",
        }
    }

    /// Whether the operation reads a PEM certificate on standard input.
    #[must_use]
    pub const fn reads_certificate(&self) -> bool {
        matches!(
            self,
            Self::TrustCertificate { .. } | Self::UntrustCertificate { .. }
        )
    }

    /// Command-line arguments encoding this operation.
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![self.name().to_owned()];
        match self {
            Self::InstallResolver { tld, port } => args.extend([
                "--tld".to_owned(),
                tld.to_string(),
                "--port".to_owned(),
                port.to_string(),
            ]),
            Self::RemoveResolver { tld } => args.extend(["--tld".to_owned(), tld.to_string()]),
            Self::InstallPortForwarding {
                address,
                http_port,
                https_port,
            } => args.extend([
                "--address".to_owned(),
                address.to_string(),
                "--http-port".to_owned(),
                http_port.to_string(),
                "--https-port".to_owned(),
                https_port.to_string(),
            ]),
            Self::TrustCertificate { sha256 }
            | Self::UntrustCertificate { sha256 }
            | Self::InstallHelper { sha256 } => {
                args.extend(["--sha256".to_owned(), sha256.to_string()]);
            }
            Self::LinkCli { target } => {
                args.extend(["--target".to_owned(), target.to_string_lossy().into_owned()]);
            }
            Self::RemoveHelper | Self::UnlinkCli => {}
            Self::RemovePortForwarding { address } => {
                args.extend(["--address".to_owned(), address.to_string()]);
            }
        }
        args
    }

    /// Decodes the arguments produced by [`Operation::to_args`].
    ///
    /// # Errors
    ///
    /// Returns [`OperationError`] for an unknown operation, a missing,
    /// misplaced or extra argument, or an invalid value.
    pub fn from_args(args: &[String]) -> Result<Self, OperationError> {
        let (name, options) = args
            .split_first()
            .ok_or_else(|| OperationError("no operation given".to_owned()))?;
        let operation = match name.as_str() {
            "install-resolver" => {
                let values = parse_options(options, &["--tld", "--port"])?;
                Self::InstallResolver {
                    tld: values[0].parse_value()?,
                    port: values[1].parse_port()?,
                }
            }
            "remove-resolver" => Self::RemoveResolver {
                tld: parse_options(options, &["--tld"])?[0].parse_value()?,
            },
            "install-port-forwarding" => {
                let values = parse_options(options, &["--address", "--http-port", "--https-port"])?;
                Self::InstallPortForwarding {
                    address: values[0].parse_value()?,
                    http_port: values[1].parse_port()?,
                    https_port: values[2].parse_port()?,
                }
            }
            "remove-port-forwarding" => Self::RemovePortForwarding {
                address: parse_options(options, &["--address"])?[0].parse_value()?,
            },
            "trust-certificate" => Self::TrustCertificate {
                sha256: parse_options(options, &["--sha256"])?[0].parse_fingerprint()?,
            },
            "untrust-certificate" => Self::UntrustCertificate {
                sha256: parse_options(options, &["--sha256"])?[0].parse_fingerprint()?,
            },
            "install-helper" => Self::InstallHelper {
                sha256: parse_options(options, &["--sha256"])?[0].parse_fingerprint()?,
            },
            "remove-helper" => {
                parse_options(options, &[])?;
                Self::RemoveHelper
            }
            "link-cli" => Self::LinkCli {
                target: parse_options(options, &["--target"])?[0].parse_absolute_path()?,
            },
            "unlink-cli" => {
                parse_options(options, &[])?;
                Self::UnlinkCli
            }
            other => return Err(OperationError(format!("unknown operation `{other}`"))),
        };
        Ok(operation)
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_args().join(" "))
    }
}

/// Value of an option, with the option name for error messages.
struct OptionValue<'a> {
    option: &'static str,
    value: &'a str,
}

impl OptionValue<'_> {
    fn parse_port(&self) -> Result<u16, OperationError> {
        match self.value.parse::<u16>() {
            Ok(port) if port > 0 => Ok(port),
            _ => Err(OperationError(format!(
                "{} expects a port number, got `{}`",
                self.option, self.value
            ))),
        }
    }

    fn parse_value<T: std::str::FromStr<Err = String>>(&self) -> Result<T, OperationError> {
        self.value
            .parse()
            .map_err(|err| OperationError(format!("{}: {err}", self.option)))
    }

    fn parse_absolute_path(&self) -> Result<PathBuf, OperationError> {
        let path = PathBuf::from(self.value);
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(OperationError(format!(
                "{} expects an absolute path, got `{}`",
                self.option, self.value
            )))
        }
    }

    fn parse_fingerprint(&self) -> Result<Fingerprint, OperationError> {
        self.value
            .parse()
            .map_err(|err| OperationError(format!("{}: {err}", self.option)))
    }
}

/// Checks that `options` is exactly `--name value` for each expected name,
/// in order, and returns the values.
fn parse_options<'a>(
    options: &'a [String],
    expected: &[&'static str],
) -> Result<Vec<OptionValue<'a>>, OperationError> {
    if options.len() != expected.len() * 2 {
        return Err(OperationError(format!(
            "expected {} option(s): {}",
            expected.len(),
            if expected.is_empty() {
                "none".to_owned()
            } else {
                expected.join(", ")
            }
        )));
    }
    expected
        .iter()
        // The length was checked above: no option is left over.
        .zip(options.as_chunks::<2>().0)
        .map(|(option, [name, value])| {
            if name == option {
                Ok(OptionValue { option, value })
            } else {
                Err(OperationError(format!("expected `{option}`, got `{name}`")))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn fingerprint() -> Fingerprint {
        "AB".repeat(32).parse().unwrap()
    }

    #[test]
    fn every_operation_roundtrips() {
        for operation in [
            Operation::InstallResolver {
                tld: Tld::default(),
                port: 47053,
            },
            Operation::RemoveResolver {
                tld: Tld::default(),
            },
            Operation::InstallPortForwarding {
                address: LoopbackAddress::default(),
                http_port: 47080,
                https_port: 47443,
            },
            Operation::RemovePortForwarding {
                address: LoopbackAddress::default(),
            },
            Operation::TrustCertificate {
                sha256: fingerprint(),
            },
            Operation::UntrustCertificate {
                sha256: fingerprint(),
            },
            Operation::InstallHelper {
                sha256: fingerprint(),
            },
            Operation::RemoveHelper,
            Operation::LinkCli {
                // Absolute on every platform.
                target: std::env::temp_dir()
                    .join("DevSpace.app")
                    .join("Contents")
                    .join("MacOS")
                    .join("devspace"),
            },
            Operation::UnlinkCli,
        ] {
            assert_eq!(
                Operation::from_args(&operation.to_args()).unwrap(),
                operation
            );
            assert!(operation.to_string().starts_with(operation.name()));
        }
    }

    #[test]
    fn encoding_is_stable() {
        assert_eq!(
            Operation::InstallPortForwarding {
                address: LoopbackAddress::default(),
                http_port: 1,
                https_port: 2
            }
            .to_args(),
            args("install-port-forwarding --address 127.0.0.2 --http-port 1 --https-port 2")
        );
    }

    #[test]
    fn malformed_command_lines_are_rejected() {
        for bad in [
            "",
            "format-disk",
            "install-resolver",
            "install-resolver --tld internal --port",
            "install-resolver --tld internal --port 0",
            "install-resolver --tld internal --port 70000",
            "install-resolver --port 53 --tld internal",
            "install-resolver --tld ../etc --port 53",
            "install-resolver --tld local --port 53",
            "remove-resolver",
            "remove-resolver --force yes",
            "install-port-forwarding --address 127.0.0.2 --https-port 2 --http-port 1",
            "install-port-forwarding --address 10.0.0.1 --http-port 1 --https-port 2",
            "remove-port-forwarding --address ::1",
            "trust-certificate --sha256 abc",
            "trust-certificate /etc/passwd",
            "install-helper",
            "remove-helper --force yes",
            "link-cli --target relative/devspace",
            "link-cli",
            "unlink-cli /usr/local/bin/devspace",
        ] {
            assert!(Operation::from_args(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_certificate_operations_read_stdin() {
        assert!(
            Operation::TrustCertificate {
                sha256: fingerprint()
            }
            .reads_certificate()
        );
        assert!(
            !Operation::RemoveResolver {
                tld: Tld::default()
            }
            .reads_certificate()
        );
    }
}
