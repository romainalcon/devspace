//! HTTPS downloads and SHA-256 checksums.
//!
//! Downloads only use HTTPS (rustls, no plain HTTP, redirects to HTTP
//! refused), have a size and time limit, and go to a file chosen by the
//! caller, which verifies the checksum before using the data.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;
use std::time::Duration;

use sha2::{Digest as _, Sha256};

use crate::{Error, Result};

/// Time limit of a whole download.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Downloads `url` into `dest` (created or truncated), refusing more than
/// `max_len` bytes. Blocking.
///
/// # Errors
///
/// [`Error::Download`] if the request fails or is refused,
/// [`Error::DownloadRead`] if the transfer is interrupted or too large,
/// [`Error::InstallFile`] if `dest` cannot be written.
pub fn fetch(url: &str, dest: &Path, max_len: u64) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|source| Error::Download {
        url: url.to_owned(),
        source: Box::new(source),
    })?;
    let mut reader = response.body_mut().with_config().limit(max_len).reader();
    let mut file = File::create(dest).map_err(file_error(dest))?;
    io::copy(&mut reader, &mut file).map_err(|source| Error::DownloadRead {
        url: url.to_owned(),
        source,
    })?;
    file.sync_all().map_err(file_error(dest))
}

/// SHA-256 of a file, in lowercase hexadecimal.
///
/// # Errors
///
/// [`Error::InstallFile`] if the file cannot be read.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(file_error(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(file_error(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Fails unless the file at `path` has the SHA-256 `expected` (lowercase
/// hexadecimal). `name` designates the file in the error.
///
/// # Errors
///
/// [`Error::ChecksumMismatch`] or the errors of [`sha256_file`].
pub fn verify_sha256(path: &Path, name: &str, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(Error::ChecksumMismatch {
            file: name.to_owned(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn file_error(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    |source| Error::InstallFile {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_are_compared_in_full() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("data");
        std::fs::write(&file, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&file).unwrap(), abc);
        verify_sha256(&file, "data", abc).unwrap();
        assert!(matches!(
            verify_sha256(&file, "data", &abc.replace('b', "c")),
            Err(Error::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn plain_http_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("file");
        assert!(matches!(
            fetch("http://127.0.0.1:9/file", &dest, 10),
            Err(Error::Download { .. })
        ));
    }
}
