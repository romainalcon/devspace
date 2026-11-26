//! X.509 certificates in PEM form and their SHA-256 fingerprints.
//!
//! Only the envelope is checked (a single PEM `CERTIFICATE` block holding
//! DER data); the operating system parses the certificate itself.

use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

/// Largest PEM text accepted.
pub const MAX_PEM_LEN: usize = 64 * 1024;

const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const END: &str = "-----END CERTIFICATE-----";

/// Error parsing a certificate or a fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CertificateError {
    /// The PEM text exceeds [`MAX_PEM_LEN`].
    #[error("the certificate exceeds {MAX_PEM_LEN} bytes")]
    TooLarge,
    /// The text does not contain exactly one PEM certificate, or contains
    /// something else.
    #[error("expected exactly one PEM certificate and nothing else")]
    NotSingleCertificate,
    /// The PEM body is not valid base64.
    #[error("the certificate is not valid base64")]
    InvalidBase64,
    /// The decoded data is not a DER structure.
    #[error("the certificate is not DER-encoded")]
    NotDer,
    /// A fingerprint is not 64 hexadecimal digits.
    #[error("`{0}` is not a SHA-256 fingerprint (64 hexadecimal digits)")]
    InvalidFingerprint(String),
}

/// A certificate, kept in DER form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    der: Vec<u8>,
}

impl Certificate {
    /// Parses a text holding exactly one PEM `CERTIFICATE` block (blank
    /// lines around it are allowed).
    ///
    /// # Errors
    ///
    /// Returns a [`CertificateError`] describing the problem.
    pub fn from_pem(text: &str) -> Result<Self, CertificateError> {
        if text.len() > MAX_PEM_LEN {
            return Err(CertificateError::TooLarge);
        }
        let text = text.trim();
        let body = text
            .strip_prefix(BEGIN)
            .and_then(|rest| rest.strip_suffix(END))
            .ok_or(CertificateError::NotSingleCertificate)?;
        if body.contains("-----") {
            return Err(CertificateError::NotSingleCertificate);
        }
        let base64: String = body.split_ascii_whitespace().collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(base64)
            .map_err(|_| CertificateError::InvalidBase64)?;
        Self::from_der(der)
    }

    /// Wraps DER data.
    ///
    /// # Errors
    ///
    /// Returns [`CertificateError::NotDer`] if the data does not start like
    /// a DER `SEQUENCE`, and [`CertificateError::TooLarge`] if its PEM form
    /// would exceed [`MAX_PEM_LEN`].
    pub fn from_der(der: Vec<u8>) -> Result<Self, CertificateError> {
        // A certificate is a DER SEQUENCE.
        if der.first() != Some(&0x30) || der.len() < 16 {
            return Err(CertificateError::NotDer);
        }
        if der.len() > MAX_PEM_LEN / 2 {
            return Err(CertificateError::TooLarge);
        }
        Ok(Self { der })
    }

    /// DER encoding.
    #[must_use]
    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// SHA-256 fingerprint of the DER encoding.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.der)
    }

    /// PEM encoding, with 64-character lines.
    #[must_use]
    pub fn to_pem(&self) -> String {
        let base64 = base64::engine::general_purpose::STANDARD.encode(&self.der);
        let mut pem = String::with_capacity(base64.len() + base64.len() / 64 + 64);
        pem.push_str(BEGIN);
        pem.push('\n');
        // Base64 output is ASCII, so splitting on byte boundaries is safe.
        for line in base64.as_bytes().chunks(64) {
            pem.push_str(&String::from_utf8_lossy(line));
            pem.push('\n');
        }
        pem.push_str(END);
        pem.push('\n');
        pem
    }
}

/// SHA-256 fingerprint of a certificate.
///
/// Displayed and parsed as 64 uppercase hexadecimal digits without
/// separators (the format of `security -Z` on macOS); parsing also accepts
/// lowercase digits and `:` separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// SHA-256 of `data` (also used for the helper program itself).
    #[must_use]
    pub fn of(data: &[u8]) -> Self {
        Self(Sha256::digest(data).into())
    }

    /// Raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Form with `:` between bytes, easier to compare by eye.
    #[must_use]
    pub fn to_colon_hex(&self) -> String {
        self.0
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl FromStr for Fingerprint {
    type Err = CertificateError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || CertificateError::InvalidFingerprint(text.to_owned());
        let digits: Vec<u8> = text.bytes().filter(|b| *b != b':').collect();
        if digits.len() != 64 {
            return Err(invalid());
        }
        let mut bytes = [0u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            let pair = std::str::from_utf8(pair).map_err(|_| invalid())?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
        }
        Ok(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny DER SEQUENCE; the envelope checks do not parse X.509.
    fn sample() -> Certificate {
        Certificate {
            der: [0x30, 0x81, 0x20]
                .into_iter()
                .chain((0u8..0x20).map(|i| i.wrapping_mul(7)))
                .collect(),
        }
    }

    #[test]
    fn pem_roundtrip_and_fingerprint() {
        let cert = sample();
        let pem = cert.to_pem();
        assert!(pem.starts_with(BEGIN));
        let parsed = Certificate::from_pem(&format!("\n{pem}\n\n")).unwrap();
        assert_eq!(parsed, cert);
        let expected: [u8; 32] = Sha256::digest(cert.der()).into();
        assert_eq!(parsed.fingerprint().as_bytes(), &expected);
    }

    #[test]
    fn long_bodies_are_wrapped() {
        let cert = Certificate {
            der: std::iter::once(0x30).chain([0xAB; 200]).collect(),
        };
        let pem = cert.to_pem();
        assert!(pem.lines().all(|line| line.len() <= 64));
        assert_eq!(Certificate::from_pem(&pem).unwrap(), cert);
    }

    #[test]
    fn anything_but_one_certificate_is_rejected() {
        let pem = sample().to_pem();
        let key = "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n";
        for (text, expected) in [
            (String::new(), CertificateError::NotSingleCertificate),
            (
                format!("{pem}{pem}"),
                CertificateError::NotSingleCertificate,
            ),
            (
                format!("{key}{pem}"),
                CertificateError::NotSingleCertificate,
            ),
            (
                format!("{pem}{key}"),
                CertificateError::NotSingleCertificate,
            ),
            (
                format!("{BEGIN}\n!!!!\n{END}\n"),
                CertificateError::InvalidBase64,
            ),
            (format!("{BEGIN}\nAAAA\n{END}\n"), CertificateError::NotDer),
            ("A".repeat(MAX_PEM_LEN + 1), CertificateError::TooLarge),
        ] {
            assert_eq!(Certificate::from_pem(&text).unwrap_err(), expected);
        }
    }

    #[test]
    fn fingerprint_text_forms() {
        let fingerprint = sample().fingerprint();
        let hex = fingerprint.to_string();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        );
        assert_eq!(hex.parse::<Fingerprint>().unwrap(), fingerprint);
        assert_eq!(
            hex.to_lowercase().parse::<Fingerprint>().unwrap(),
            fingerprint
        );
        assert_eq!(
            fingerprint.to_colon_hex().parse::<Fingerprint>().unwrap(),
            fingerprint
        );
        for bad in [
            "",
            "AB",
            &"G".repeat(64),
            &"A".repeat(66),
            "é".repeat(32).as_str(),
        ] {
            assert!(bad.parse::<Fingerprint>().is_err(), "{bad}");
        }
    }
}
