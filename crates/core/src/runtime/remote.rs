//! Package index published with the DevSpace releases.
//!
//! The maintainer publishes a newer signed index, with a higher `serial`, as
//! the assets `index.json` and `index.json.minisig` of the GitHub release
//! `index`. The daemon downloads both, checks the signature with the public
//! key embedded in the build (the same as the embedded index), and only
//! accepts an index whose serial is higher than the one it uses: an older
//! index, even correctly signed, is never accepted again (no rollback).
//! The accepted index is kept in `<data>/index/` and used at the next
//! startup when it is newer than the embedded one.

use std::fs;
use std::path::{Path, PathBuf};

use devspace_platform::fs::{FileMode, write_atomic};

use super::index::{Index, PUBLIC_KEY};
use crate::error::{Error, Result};
use crate::{AppPaths, download};

/// URL of the published index.
pub const REMOTE_INDEX_URL: &str =
    "https://github.com/romainalcon/devspace/releases/download/index/index.json";

/// URL of its minisign signature.
pub const REMOTE_SIGNATURE_URL: &str =
    "https://github.com/romainalcon/devspace/releases/download/index/index.json.minisig";

/// Largest index downloaded.
const MAX_INDEX_LEN: u64 = 8 * 1024 * 1024;

/// Largest signature downloaded.
const MAX_SIGNATURE_LEN: u64 = 4 * 1024;

const INDEX_FILE: &str = "index.json";
const SIGNATURE_FILE: &str = "index.json.minisig";

/// Where the index in use comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexSource {
    /// Embedded in this build.
    Embedded,
    /// Downloaded earlier and kept in `<data>/index/`.
    Downloaded,
}

impl IndexSource {
    /// Name used in logs and in the protocol.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Downloaded => "downloaded",
        }
    }
}

/// Directory of the downloaded index.
#[derive(Debug, Clone)]
pub struct IndexStore {
    dir: PathBuf,
    public_key: String,
}

impl IndexStore {
    /// Store in `<data>/index`.
    #[must_use]
    pub fn new(paths: &AppPaths) -> Self {
        Self::at(paths.data_dir.join("index"), PUBLIC_KEY)
    }

    fn at(dir: PathBuf, public_key: &str) -> Self {
        Self {
            dir,
            public_key: public_key.to_owned(),
        }
    }

    /// The newest of `embedded` and the kept index. A kept index that is
    /// unreadable or whose signature does not match is ignored (and
    /// logged).
    #[must_use]
    pub fn newest(&self, embedded: Index) -> (Index, IndexSource) {
        match self.load_kept() {
            Ok(Some(kept)) if kept.serial > embedded.serial => (kept, IndexSource::Downloaded),
            Ok(_) => (embedded, IndexSource::Embedded),
            Err(err) => {
                tracing::warn!(
                    error = %crate::error::report(&err),
                    dir = %self.dir.display(),
                    "ignoring the downloaded package index"
                );
                (embedded, IndexSource::Embedded)
            }
        }
    }

    fn load_kept(&self) -> Result<Option<Index>> {
        let index_path = self.dir.join(INDEX_FILE);
        let bytes = match fs::read(&index_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(Error::InstallFile {
                    path: index_path,
                    source,
                });
            }
        };
        let signature_path = self.dir.join(SIGNATURE_FILE);
        let signature =
            fs::read_to_string(&signature_path).map_err(|source| Error::InstallFile {
                path: signature_path,
                source,
            })?;
        Index::verify(&bytes, &signature, &self.public_key).map(Some)
    }

    /// Downloads the published index; returns it when its signature is
    /// valid and its serial is higher than `current_serial`, after keeping
    /// it. Blocking.
    ///
    /// # Errors
    ///
    /// Download errors, the errors of [`Index::verify`] (nothing is kept
    /// then), or [`Error::InstallFile`] if it cannot be kept.
    pub fn refresh(&self, current_serial: u64) -> Result<Option<Index>> {
        fs::create_dir_all(&self.dir).map_err(|source| Error::InstallFile {
            path: self.dir.clone(),
            source,
        })?;
        let bytes = self.fetch(REMOTE_INDEX_URL, MAX_INDEX_LEN)?;
        let signature = self.fetch(REMOTE_SIGNATURE_URL, MAX_SIGNATURE_LEN)?;
        let signature = String::from_utf8(signature)
            .map_err(|_| Error::IndexSignature("the signature file is not text".to_owned()))?;
        self.accept(&bytes, &signature, current_serial)
    }

    fn fetch(&self, url: &str, max_len: u64) -> Result<Vec<u8>> {
        let temp = self.dir.join(format!(".download-{}", std::process::id()));
        let result = download::fetch(url, &temp, max_len).and_then(|()| {
            fs::read(&temp).map_err(|source| Error::InstallFile {
                path: temp.clone(),
                source,
            })
        });
        let _ = fs::remove_file(&temp);
        result
    }

    /// Checks a downloaded index and keeps it if it is newer.
    fn accept(&self, bytes: &[u8], signature: &str, current_serial: u64) -> Result<Option<Index>> {
        let index = Index::verify(bytes, signature, &self.public_key)?;
        if index.serial <= current_serial {
            return Ok(None);
        }
        let keep = |name: &str, data: &[u8]| {
            let path = self.dir.join(name);
            write_atomic(&path, data, FileMode::Private)
                .map_err(|source| Error::InstallFile { path, source })
        };
        // The signature last: until then the pair does not verify, so a crash
        // in between leaves the previous index or none, never a wrong one.
        let _ = fs::remove_file(self.dir.join(SIGNATURE_FILE));
        keep(INDEX_FILE, bytes)?;
        keep(SIGNATURE_FILE, signature.as_bytes())?;
        Ok(Some(index))
    }

    /// Directory of the kept index.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::index::tests::{SAMPLE, TestSigner};

    fn with_serial(serial: u64) -> String {
        SAMPLE.replace("\"serial\": 7", &format!("\"serial\": {serial}"))
    }

    fn setup() -> (tempfile::TempDir, TestSigner, IndexStore) {
        let tmp = tempfile::tempdir().unwrap();
        let signer = TestSigner::new();
        let store = IndexStore::at(tmp.path().join("index"), &signer.public_key());
        fs::create_dir_all(store.dir()).unwrap();
        (tmp, signer, store)
    }

    fn embedded(signer: &TestSigner, serial: u64) -> Index {
        let bytes = with_serial(serial);
        Index::verify(
            bytes.as_bytes(),
            &signer.sign(bytes.as_bytes()),
            &signer.public_key(),
        )
        .unwrap()
    }

    #[test]
    fn only_a_newer_signed_index_is_kept_and_used() {
        let (_tmp, signer, store) = setup();
        assert_eq!(store.newest(embedded(&signer, 7)).1, IndexSource::Embedded);

        let older = with_serial(6);
        let same = with_serial(7);
        let newer = with_serial(9);
        for bytes in [&older, &same] {
            assert!(
                store
                    .accept(bytes.as_bytes(), &signer.sign(bytes.as_bytes()), 7)
                    .unwrap()
                    .is_none()
            );
        }
        let accepted = store
            .accept(newer.as_bytes(), &signer.sign(newer.as_bytes()), 7)
            .unwrap()
            .unwrap();
        assert_eq!(accepted.serial, 9);

        let (index, source) = store.newest(embedded(&signer, 7));
        assert_eq!((index.serial, source), (9, IndexSource::Downloaded));
        // A build embedding a newer index ignores the kept one.
        let (index, source) = store.newest(embedded(&signer, 10));
        assert_eq!((index.serial, source), (10, IndexSource::Embedded));
    }

    #[test]
    fn indexes_signed_by_another_key_are_refused() {
        let (_tmp, _signer, store) = setup();
        let other = TestSigner::new();
        let newer = with_serial(9);
        assert!(matches!(
            store.accept(newer.as_bytes(), &other.sign(newer.as_bytes()), 7),
            Err(Error::IndexSignature(_))
        ));
        assert!(!store.dir().join(INDEX_FILE).exists());
    }

    #[test]
    fn a_tampered_kept_index_is_ignored() {
        let (_tmp, signer, store) = setup();
        let newer = with_serial(9);
        store
            .accept(newer.as_bytes(), &signer.sign(newer.as_bytes()), 7)
            .unwrap();
        fs::write(store.dir().join(INDEX_FILE), with_serial(99)).unwrap();
        let (index, source) = store.newest(embedded(&signer, 7));
        assert_eq!((index.serial, source), (7, IndexSource::Embedded));
    }
}
