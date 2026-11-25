//! The local certificate authority.
//!
//! Caddy creates the authority (root certificate and key) in its storage on
//! its first start, and issues the site certificates with it. DevSpace only
//! reads the root certificate, to show its fingerprint and to have it
//! trusted by the privileged helper after approval.
//!
//! The private key stays in the DevSpace data directory, readable by the
//! current user only: like any local development CA (mkcert, Herd, …),
//! anyone able to run code as this user could issue certificates trusted
//! by the machine.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::PathBuf;

use devspace_helper::Certificate;
use devspace_helper::certificate::MAX_PEM_LEN;

use super::WebLayout;
use crate::{Error, Result};

/// Root certificate of the local certificate authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCa {
    /// File holding the certificate (PEM).
    pub path: PathBuf,
    /// The certificate.
    pub certificate: Certificate,
}

/// Loads the root certificate, or `None` while Caddy has not created it.
///
/// # Errors
///
/// Returns [`Error::CaRead`] if the file cannot be read, or
/// [`Error::CaInvalid`] if it does not hold one PEM certificate.
pub fn load(layout: &WebLayout) -> Result<Option<LocalCa>> {
    let path = layout.ca_root_certificate();
    let read_err = |source| Error::CaRead {
        path: path.clone(),
        source,
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(read_err(err)),
    };
    let mut text = String::new();
    file.take(u64::try_from(MAX_PEM_LEN + 1).unwrap_or(u64::MAX))
        .read_to_string(&mut text)
        .map_err(read_err)?;
    let certificate = Certificate::from_pem(&text).map_err(|source| Error::CaInvalid {
        path: path.clone(),
        source,
    })?;
    Ok(Some(LocalCa { path, certificate }))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::AppPaths;

    #[test]
    fn missing_valid_and_invalid_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = WebLayout::new(&AppPaths::from_root(tmp.path()));
        assert_eq!(load(&layout).unwrap(), None);

        let path = layout.ca_root_certificate();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "garbage").unwrap();
        assert!(matches!(load(&layout), Err(Error::CaInvalid { .. })));

        let der: Vec<u8> = std::iter::once(0x30).chain([0x5a; 40]).collect();
        let pem = Certificate::from_der(der.clone()).unwrap().to_pem();
        fs::write(&path, &pem).unwrap();
        let ca = load(&layout).unwrap().unwrap();
        assert_eq!(ca.path, path);
        assert_eq!(ca.certificate.der(), der.as_slice());
    }
}
