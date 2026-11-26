//! Extraction of downloaded archives.
//!
//! Formats: `.tar.gz`, `.tar.xz` and `.zip`. Two modes, both safe against
//! hostile archives (no path traversal):
//!
//! - [`extract_file`] copies one entry into a destination chosen by the
//!   caller: names, paths, links and permissions stored in the archive are
//!   never used to create files;
//! - [`extract_tree`] (module [`tree`]) recreates a whole archive inside a
//!   fresh directory, refusing any entry or link that could reach outside
//!   of it.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path};

pub mod tree;

pub use tree::{TreeLimits, extract_tree};

/// Format of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// Gzip-compressed tar archive (`.tar.gz`).
    TarGz,
    /// Xz-compressed tar archive (`.tar.xz`).
    TarXz,
    /// Zip archive (`.zip`).
    Zip,
}

/// Failure to extract a file from an archive.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The archive could not be read or is corrupt.
    #[error("the archive is unreadable or corrupt")]
    Read(#[source] io::Error),
    /// The zip archive is corrupt.
    #[error("the zip archive is corrupt")]
    Zip(#[source] zip::result::ZipError),
    /// The requested entry is absent.
    #[error("the archive does not contain `{0}`")]
    MissingEntry(String),
    /// The requested entry is not a regular file (directory, link, …).
    #[error("`{0}` is not a regular file in the archive")]
    NotAFile(String),
    /// The entry exceeds the size limit.
    #[error("`{entry}` is larger than {max} bytes")]
    TooLarge {
        /// Entry name.
        entry: String,
        /// Limit in bytes.
        max: u64,
    },
    /// The destination could not be written.
    #[error("failed to write the extracted file")]
    Write(#[source] io::Error),
    /// An entry name could designate a location outside the destination
    /// (absolute, `..`, drive or stream separators, …).
    #[error("unsafe entry name `{0}`")]
    UnsafePath(String),
    /// A symbolic link points outside the destination, or nowhere.
    #[error("symbolic link `{link}` → `{target}` points outside the archive")]
    UnsafeLink {
        /// Link entry.
        link: String,
        /// Its target.
        target: String,
    },
    /// A hard link does not designate a regular file extracted before it.
    #[error("hard link `{link}` → `{target}` does not designate a file of the archive")]
    UnsafeHardLink {
        /// Link entry.
        link: String,
        /// Its target.
        target: String,
    },
    /// An entry type that is never extracted (device, FIFO, …).
    #[error("entry `{0}` has an unsupported type")]
    UnsupportedEntry(String),
    /// Two entries have the same path.
    #[error("entry `{0}` appears twice")]
    Duplicate(String),
    /// The archive holds more data or entries than allowed.
    #[error("the archive exceeds the limit of {0}")]
    LimitExceeded(String),
    /// A file, directory or link could not be created.
    #[error("failed to create `{}`", path.display())]
    Create {
        /// Path being created.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
}

/// Copies the regular file `entry` (a plain file name at the root of the
/// archive, optionally prefixed by `./`) of the archive at `archive` into
/// `dest`, and returns its size.
///
/// # Errors
///
/// Returns an [`ArchiveError`] if the archive is unreadable, the entry is
/// missing, not a regular file, or larger than `max_len`.
pub fn extract_file(
    archive: &Path,
    kind: ArchiveKind,
    entry: &str,
    dest: &mut impl Write,
    max_len: u64,
) -> Result<u64, ArchiveError> {
    let file = File::open(archive).map_err(ArchiveError::Read)?;
    match kind {
        ArchiveKind::TarGz => {
            extract_from_tar(flate2::read::GzDecoder::new(file), entry, dest, max_len)
        }
        ArchiveKind::TarXz => extract_from_tar(xz_decoder(file), entry, dest, max_len),
        ArchiveKind::Zip => extract_from_zip(file, entry, dest, max_len),
    }
}

/// Decoder of an xz stream (a single stream, as produced by `xz`).
pub(crate) fn xz_decoder(file: File) -> impl Read {
    liblzma::read::XzDecoder::new(file)
}

fn extract_from_tar(
    reader: impl Read,
    entry: &str,
    dest: &mut impl Write,
    max_len: u64,
) -> Result<u64, ArchiveError> {
    let mut archive = tar::Archive::new(reader);
    for item in archive.entries().map_err(ArchiveError::Read)? {
        let item = item.map_err(ArchiveError::Read)?;
        let path = item.path().map_err(ArchiveError::Read)?;
        if !is_root_entry(&path, entry) {
            continue;
        }
        if !item.header().entry_type().is_file() {
            return Err(ArchiveError::NotAFile(entry.to_owned()));
        }
        return copy_limited(item, entry, dest, max_len);
    }
    Err(ArchiveError::MissingEntry(entry.to_owned()))
}

fn extract_from_zip(
    file: File,
    entry: &str,
    dest: &mut impl Write,
    max_len: u64,
) -> Result<u64, ArchiveError> {
    let mut archive = zip::ZipArchive::new(file).map_err(ArchiveError::Zip)?;
    let item = match archive.by_name(entry) {
        Ok(item) => item,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(ArchiveError::MissingEntry(entry.to_owned()));
        }
        Err(err) => return Err(ArchiveError::Zip(err)),
    };
    if !item.is_file() || item.is_symlink() {
        return Err(ArchiveError::NotAFile(entry.to_owned()));
    }
    if item.size() > max_len {
        return Err(ArchiveError::TooLarge {
            entry: entry.to_owned(),
            max: max_len,
        });
    }
    copy_limited(item, entry, dest, max_len)
}

/// Whether `path` designates `name` at the root of the archive.
fn is_root_entry(path: &Path, name: &str) -> bool {
    let mut components = path
        .components()
        .filter(|component| *component != Component::CurDir);
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(first)), None) if first == name
    )
}

/// Copies at most `max_len` bytes; fails if the reader holds more, whatever
/// size the archive headers announced.
fn copy_limited(
    reader: impl Read,
    entry: &str,
    dest: &mut impl Write,
    max_len: u64,
) -> Result<u64, ArchiveError> {
    let mut limited = reader.take(max_len.saturating_add(1));
    let mut copied = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = limited.read(&mut buffer).map_err(ArchiveError::Read)?;
        if read == 0 {
            break;
        }
        copied += read as u64;
        if copied > max_len {
            return Err(ArchiveError::TooLarge {
                entry: entry.to_owned(),
                max: max_len,
            });
        }
        dest.write_all(&buffer[..read])
            .map_err(ArchiveError::Write)?;
    }
    dest.flush().map_err(ArchiveError::Write)?;
    Ok(copied)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;

    use super::*;

    /// Builds a `.tar.gz` file holding `entries` (path, data, is_symlink);
    /// symbolic links point to `/etc/passwd`.
    pub(crate) fn tar_gz(path: &Path, entries: &[(&str, &[u8], bool)]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (name, data, symlink) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            if *symlink {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder
                    .append_link(&mut header, name, "/etc/passwd")
                    .unwrap();
            } else {
                header.set_size(data.len() as u64);
                // `append_data` refuses `..`; write the raw name to test
                // hostile archives.
                header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            }
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    pub(crate) fn zip_file(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    fn extract(
        path: &Path,
        kind: ArchiveKind,
        entry: &str,
        max: u64,
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut out = Cursor::new(Vec::new());
        extract_file(path, kind, entry, &mut out, max).map(|_| out.into_inner())
    }

    #[test]
    fn tar_gz_entry_is_extracted() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("caddy.tar.gz");
        tar_gz(
            &archive,
            &[
                ("LICENSE", b"license", false),
                ("./caddy", b"binary", false),
            ],
        );
        assert_eq!(
            extract(&archive, ArchiveKind::TarGz, "caddy", 100).unwrap(),
            b"binary"
        );
        assert!(matches!(
            extract(&archive, ArchiveKind::TarGz, "caddy", 3),
            Err(ArchiveError::TooLarge { .. })
        ));
        assert!(matches!(
            extract(&archive, ArchiveKind::TarGz, "README.md", 100),
            Err(ArchiveError::MissingEntry(_))
        ));
    }

    #[test]
    fn hostile_tar_entries_are_ignored_or_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("evil.tar.gz");
        tar_gz(
            &archive,
            &[
                ("../caddy", b"escape", false),
                ("bin/caddy", b"nested", false),
            ],
        );
        assert!(matches!(
            extract(&archive, ArchiveKind::TarGz, "caddy", 100),
            Err(ArchiveError::MissingEntry(_))
        ));

        let archive = tmp.path().join("link.tar.gz");
        tar_gz(&archive, &[("caddy", b"", true)]);
        assert!(matches!(
            extract(&archive, ArchiveKind::TarGz, "caddy", 100),
            Err(ArchiveError::NotAFile(_))
        ));

        let garbage = tmp.path().join("garbage.tar.gz");
        std::fs::write(&garbage, b"not gzip at all").unwrap();
        assert!(matches!(
            extract(&garbage, ArchiveKind::TarGz, "caddy", 100),
            Err(ArchiveError::Read(_))
        ));
    }

    #[test]
    fn zip_entry_is_extracted() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("caddy.zip");
        zip_file(
            &archive,
            &[("LICENSE", b"license"), ("caddy.exe", b"windows binary")],
        );
        assert_eq!(
            extract(&archive, ArchiveKind::Zip, "caddy.exe", 100).unwrap(),
            b"windows binary"
        );
        assert!(matches!(
            extract(&archive, ArchiveKind::Zip, "caddy.exe", 4),
            Err(ArchiveError::TooLarge { .. })
        ));
        assert!(matches!(
            extract(&archive, ArchiveKind::Zip, "caddy", 100),
            Err(ArchiveError::MissingEntry(_))
        ));
        let garbage = tmp.path().join("garbage.zip");
        std::fs::write(&garbage, b"PK but not really").unwrap();
        assert!(matches!(
            extract(&garbage, ArchiveKind::Zip, "caddy.exe", 100),
            Err(ArchiveError::Zip(_))
        ));
    }

    #[test]
    fn root_entry_matching() {
        assert!(is_root_entry(Path::new("caddy"), "caddy"));
        assert!(is_root_entry(Path::new("./caddy"), "caddy"));
        assert!(!is_root_entry(Path::new("x/caddy"), "caddy"));
        assert!(!is_root_entry(Path::new("../caddy"), "caddy"));
        assert!(!is_root_entry(Path::new("/caddy"), "caddy"));
        assert!(!is_root_entry(Path::new("caddy2"), "caddy"));
    }
}
