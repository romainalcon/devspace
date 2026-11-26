//! Extraction of a whole archive into a directory.
//!
//! Rules enforced on every entry, whatever the archive claims:
//!
//! - names are split on `/` only; absolute names, `..`, empty names and
//!   components holding `\`, `:` or NUL are refused ([`ArchiveError::UnsafePath`]);
//! - the first `strip_components` components are removed (entries left
//!   without a name, such as the top directory, are skipped);
//! - regular files are created with [`create_new`] (an existing path is a
//!   [`ArchiveError::Duplicate`]) and only keep the executable bit: the
//!   owner, setuid/setgid bits and timestamps of the archive are ignored;
//! - symbolic links (tar only) are created **after** every file, so no file
//!   is ever written through a link of the archive. Their target must be
//!   relative and stay lexically inside the destination; once all links
//!   exist, each one is resolved and must lead to an existing path inside
//!   the destination, which also rules out chains of links escaping it;
//! - hard links (tar only) must designate a regular file extracted before
//!   them by the same archive; they become independent copies of it, so
//!   their target is never looked up outside the files of this extraction;
//! - devices, FIFOs and other entry types are refused;
//! - the total size and number of entries are limited ([`TreeLimits`]).
//!
//! The destination is expected to be a fresh directory that nothing uses
//! yet: on error the caller discards it.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use devspace_platform::fs::{FileMode, canonicalize, create_new, set_mode, symlink};

use super::{ArchiveError, ArchiveKind};

/// Limits applied to [`extract_tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLimits {
    /// Total size of the extracted files, in bytes.
    pub max_total_len: u64,
    /// Number of entries (files, directories and links).
    pub max_entries: usize,
}

impl Default for TreeLimits {
    /// 2 GiB and 100 000 entries: far above the runtimes DevSpace installs
    /// (a Node.js release has about 6 000 entries and 200 MB).
    fn default() -> Self {
        Self {
            max_total_len: 2 * 1024 * 1024 * 1024,
            max_entries: 100_000,
        }
    }
}

/// What [`extract_tree`] created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeStats {
    /// Regular files, including copies of hard links.
    pub files: usize,
    /// Symbolic links.
    pub links: usize,
    /// Bytes written.
    pub bytes: u64,
}

/// A symbolic link waiting to be created.
struct PendingLink {
    name: String,
    path: PathBuf,
    target: PathBuf,
    target_text: String,
}

/// Kind of an archive entry, independent of the format.
enum EntryKind {
    Directory,
    File { executable: bool },
    Symlink { target: String },
    HardLink { target: String },
    Other,
}

/// Extracts the archive at `archive` into `dest`, an existing directory.
///
/// # Errors
///
/// Returns an [`ArchiveError`] on an unreadable archive, an entry breaking
/// a rule of this module, or a filesystem failure. Whatever was extracted
/// before the error stays in `dest`.
pub fn extract_tree(
    archive: &Path,
    kind: ArchiveKind,
    dest: &Path,
    strip_components: usize,
    limits: TreeLimits,
) -> Result<TreeStats, ArchiveError> {
    let root = canonicalize(dest).map_err(|source| ArchiveError::Create {
        path: dest.to_path_buf(),
        source,
    })?;
    let mut extractor = Extractor {
        root,
        strip_components,
        limits,
        entries: 0,
        stats: TreeStats::default(),
        links: Vec::new(),
        files: HashMap::new(),
    };
    let file = File::open(archive).map_err(ArchiveError::Read)?;
    match kind {
        ArchiveKind::TarGz => {
            extract_tar(flate2::read::GzDecoder::new(file), &mut extractor)?;
        }
        ArchiveKind::TarXz => extract_tar(super::xz_decoder(file), &mut extractor)?,
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(file).map_err(ArchiveError::Zip)?;
            for index in 0..zip.len() {
                let item = zip.by_index(index).map_err(ArchiveError::Zip)?;
                let name = item.name().map_err(ArchiveError::Zip)?.into_owned();
                let kind = if item.is_symlink() {
                    // Never needed by the packages DevSpace installs, and
                    // not creatable by regular users on Windows.
                    EntryKind::Other
                } else if item.is_dir() {
                    EntryKind::Directory
                } else if item.is_file() {
                    EntryKind::File {
                        executable: item.unix_mode().is_some_and(|mode| mode & 0o111 != 0),
                    }
                } else {
                    EntryKind::Other
                };
                extractor.entry(&name, kind, item)?;
            }
        }
    }
    extractor.finish()
}

/// Feeds every entry of a tar stream to `extractor`.
fn extract_tar(reader: impl Read, extractor: &mut Extractor) -> Result<(), ArchiveError> {
    let mut tar = tar::Archive::new(reader);
    for item in tar.entries().map_err(ArchiveError::Read)? {
        let item = item.map_err(ArchiveError::Read)?;
        let name = text(&item.path_bytes())?;
        let header = item.header();
        let entry_type = header.entry_type();
        let link_target = || {
            item.link_name_bytes()
                .ok_or_else(|| ArchiveError::UnsupportedEntry(name.clone()))
                .and_then(|target| text(&target))
        };
        let kind = if entry_type.is_dir() {
            EntryKind::Directory
        } else if entry_type.is_file() {
            let mode = header.mode().map_err(ArchiveError::Read)?;
            EntryKind::File {
                executable: mode & 0o111 != 0,
            }
        } else if entry_type.is_symlink() {
            EntryKind::Symlink {
                target: link_target()?,
            }
        } else if entry_type.is_hard_link() {
            EntryKind::HardLink {
                target: link_target()?,
            }
        } else if matches!(
            entry_type,
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader
        ) {
            // Metadata records (pax headers), not files.
            continue;
        } else {
            EntryKind::Other
        };
        extractor.entry(&name, kind, item)?;
    }
    Ok(())
}

struct Extractor {
    root: PathBuf,
    strip_components: usize,
    limits: TreeLimits,
    entries: usize,
    stats: TreeStats,
    links: Vec<PendingLink>,
    /// Regular files created so far, with their executable bit: the only
    /// possible targets of hard links.
    files: HashMap<PathBuf, bool>,
}

impl Extractor {
    fn entry(&mut self, name: &str, kind: EntryKind, data: impl Read) -> Result<(), ArchiveError> {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Err(ArchiveError::LimitExceeded(format!(
                "{} entries",
                self.limits.max_entries
            )));
        }
        let components = safe_components(name)?;
        let Some(relative) = components.get(self.strip_components..) else {
            return Ok(());
        };
        if relative.is_empty() {
            return Ok(());
        }
        let path = self.path_of(relative);
        match kind {
            EntryKind::Directory => create_dirs(&path),
            EntryKind::File { executable } => {
                if let Some(parent) = path.parent() {
                    create_dirs(parent)?;
                }
                self.write_file(name, &path, executable, data)
            }
            EntryKind::Symlink { target } => {
                let target_path = self.link_target(name, relative, &target)?;
                self.links.push(PendingLink {
                    name: name.to_owned(),
                    path,
                    target: target_path,
                    target_text: target,
                });
                Ok(())
            }
            EntryKind::HardLink { target } => self.copy_hard_link(name, &path, &target),
            EntryKind::Other => Err(ArchiveError::UnsupportedEntry(name.to_owned())),
        }
    }

    fn path_of(&self, relative: &[&str]) -> PathBuf {
        relative
            .iter()
            .fold(self.root.clone(), |path, component| path.join(component))
    }

    /// Creates the hard link `name` at `path` as a copy of `target`, an
    /// entry name of the archive designating a regular file already
    /// extracted.
    fn copy_hard_link(
        &mut self,
        name: &str,
        path: &Path,
        target: &str,
    ) -> Result<(), ArchiveError> {
        let unsafe_link = || ArchiveError::UnsafeHardLink {
            link: name.to_owned(),
            target: target.to_owned(),
        };
        let components = safe_components(target).map_err(|_| unsafe_link())?;
        let source = components
            .get(self.strip_components..)
            .filter(|relative| !relative.is_empty())
            .map(|relative| self.path_of(relative))
            .ok_or_else(unsafe_link)?;
        let executable = *self.files.get(&source).ok_or_else(unsafe_link)?;
        if let Some(parent) = path.parent() {
            create_dirs(parent)?;
        }
        let data = File::open(&source).map_err(|source_err| ArchiveError::Create {
            path: source.clone(),
            source: source_err,
        })?;
        self.write_file(name, path, executable, data)
    }

    fn write_file(
        &mut self,
        name: &str,
        path: &Path,
        executable: bool,
        data: impl Read,
    ) -> Result<(), ArchiveError> {
        let mode = if executable {
            FileMode::Executable
        } else {
            FileMode::Readable
        };
        let mut file = create_new(path, mode).map_err(|source| {
            if source.kind() == io::ErrorKind::AlreadyExists {
                ArchiveError::Duplicate(name.to_owned())
            } else {
                ArchiveError::Create {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        let remaining = self.limits.max_total_len - self.stats.bytes;
        let copied = super::copy_limited(data, name, &mut file, remaining).map_err(|err| {
            if matches!(err, ArchiveError::TooLarge { .. }) {
                ArchiveError::LimitExceeded(format!("{} bytes", self.limits.max_total_len))
            } else {
                err
            }
        })?;
        drop(file);
        // The umask may have removed bits at creation.
        set_mode(path, mode).map_err(|source| ArchiveError::Create {
            path: path.to_path_buf(),
            source,
        })?;
        self.stats.files += 1;
        self.stats.bytes += copied;
        self.files.insert(path.to_path_buf(), executable);
        Ok(())
    }

    /// Validates the target of the link `relative` (components below the
    /// root) lexically and returns it as a path.
    fn link_target(
        &self,
        name: &str,
        relative: &[&str],
        target: &str,
    ) -> Result<PathBuf, ArchiveError> {
        let unsafe_link = || ArchiveError::UnsafeLink {
            link: name.to_owned(),
            target: target.to_owned(),
        };
        if target.is_empty() || target.starts_with('/') {
            return Err(unsafe_link());
        }
        // Depth of the directory holding the link, below the root.
        let mut depth = relative.len() - 1;
        let mut path = PathBuf::new();
        for component in target.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    depth = depth.checked_sub(1).ok_or_else(unsafe_link)?;
                    path.push("..");
                }
                other if is_safe_component(other) => {
                    depth += 1;
                    path.push(other);
                }
                _ => return Err(unsafe_link()),
            }
        }
        Ok(path)
    }

    fn finish(self) -> Result<TreeStats, ArchiveError> {
        let mut stats = self.stats;
        for link in &self.links {
            if let Some(parent) = link.path.parent() {
                create_dirs(parent)?;
            }
            symlink(&link.target, &link.path).map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    ArchiveError::Duplicate(link.name.clone())
                } else {
                    ArchiveError::Create {
                        path: link.path.clone(),
                        source,
                    }
                }
            })?;
            stats.links += 1;
        }
        // Every link exists now: resolving them also follows chains.
        for link in &self.links {
            match canonicalize(&link.path) {
                Ok(resolved) if resolved.starts_with(&self.root) => {}
                _ => {
                    return Err(ArchiveError::UnsafeLink {
                        link: link.name.clone(),
                        target: link.target_text.clone(),
                    });
                }
            }
        }
        Ok(stats)
    }
}

fn create_dirs(path: &Path) -> Result<(), ArchiveError> {
    fs::create_dir_all(path).map_err(|source| ArchiveError::Create {
        path: path.to_path_buf(),
        source,
    })
}

/// Entry names and link targets must be UTF-8.
fn text(bytes: &[u8]) -> Result<String, ArchiveError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|err| ArchiveError::UnsafePath(String::from_utf8_lossy(err.as_bytes()).into()))
}

/// Components of an entry name, refusing anything that could leave the
/// destination on any platform.
fn safe_components(name: &str) -> Result<Vec<&str>, ArchiveError> {
    if name.starts_with('/') {
        return Err(ArchiveError::UnsafePath(name.to_owned()));
    }
    let mut components = Vec::new();
    for component in name.split('/') {
        match component {
            "" | "." => {}
            other if is_safe_component(other) => components.push(other),
            _ => return Err(ArchiveError::UnsafePath(name.to_owned())),
        }
    }
    if components.is_empty() {
        return Err(ArchiveError::UnsafePath(name.to_owned()));
    }
    Ok(components)
}

/// A plain file name: not `..`, no separator, drive or stream marker.
fn is_safe_component(component: &str) -> bool {
    component != ".." && !component.contains(['\\', ':', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::zip_file;

    /// Entry of a test tar archive.
    enum Item<'a> {
        Dir(&'a str),
        File(&'a str, &'a [u8], u32),
        Link(&'a str, &'a str),
        HardLink(&'a str, &'a str),
    }

    fn tar_gz(path: &Path, items: &[Item<'_>]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for item in items {
            let mut header = tar::Header::new_gnu();
            let (name, link) = match item {
                Item::Dir(name) => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_mode(0o755);
                    header.set_size(0);
                    (*name, None)
                }
                Item::File(name, data, mode) => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_mode(*mode);
                    header.set_size(data.len() as u64);
                    (*name, None)
                }
                Item::Link(name, target) => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    (*name, Some(*target))
                }
                Item::HardLink(name, target) => {
                    header.set_entry_type(tar::EntryType::Link);
                    header.set_size(0);
                    (*name, Some(*target))
                }
            };
            // Raw names: the `tar` builder would refuse hostile ones.
            let gnu = header.as_gnu_mut().unwrap();
            gnu.name[..name.len()].copy_from_slice(name.as_bytes());
            if let Some(link) = link {
                gnu.linkname[..link.len()].copy_from_slice(link.as_bytes());
            }
            header.set_cksum();
            let data: &[u8] = match item {
                Item::File(_, data, _) => data,
                _ => &[],
            };
            builder.append(&header, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    fn extract(
        items: &[Item<'_>],
        strip: usize,
    ) -> (tempfile::TempDir, Result<TreeStats, ArchiveError>) {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("archive.tar.gz");
        tar_gz(&archive, items);
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let result = extract_tree(
            &archive,
            ArchiveKind::TarGz,
            &dest,
            strip,
            TreeLimits::default(),
        );
        (tmp, result)
    }

    #[test]
    fn release_layout_is_recreated() {
        let (tmp, result) = extract(
            &[
                Item::Dir("node-v1/"),
                Item::Dir("node-v1/bin/"),
                Item::File("node-v1/bin/node", b"binary", 0o755),
                Item::File("node-v1/lib/node_modules/npm/bin/npm-cli.js", b"js", 0o644),
                Item::Link("node-v1/bin/npm", "../lib/node_modules/npm/bin/npm-cli.js"),
                Item::File("node-v1/LICENSE", b"license", 0o4755),
            ],
            1,
        );
        let stats = result.unwrap();
        assert_eq!(
            stats,
            TreeStats {
                files: 3,
                links: 1,
                bytes: 15
            }
        );
        let dest = tmp.path().join("dest");
        assert_eq!(fs::read(dest.join("bin/node")).unwrap(), b"binary");
        assert_eq!(fs::read(dest.join("bin/npm")).unwrap(), b"js");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode =
                |path: &str| fs::metadata(dest.join(path)).unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode("bin/node"), 0o755);
            assert_eq!(mode("lib/node_modules/npm/bin/npm-cli.js"), 0o644);
            assert_eq!(mode("LICENSE"), 0o755, "setuid is never kept");
        }
    }

    #[test]
    fn traversal_names_are_refused() {
        for name in [
            "../evil",
            "a/../../evil",
            "/etc/evil",
            "a/..",
            "c:evil",
            "a\\..\\evil",
            "a/b:stream",
        ] {
            let (tmp, result) = extract(&[Item::File(name, b"x", 0o644)], 0);
            assert!(
                matches!(result, Err(ArchiveError::UnsafePath(_))),
                "{name}: {result:?}"
            );
            assert!(!tmp.path().join("evil").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn escaping_links_are_refused() {
        for (link, target) in [
            ("pkg/evil", "/etc/passwd"),
            ("pkg/evil", "../../outside"),
            ("pkg/a/evil", "../../../outside"),
            ("pkg/evil", "missing"),
        ] {
            let (_tmp, result) = extract(&[Item::Link(link, target)], 1);
            assert!(
                matches!(result, Err(ArchiveError::UnsafeLink { .. })),
                "{link} -> {target}: {result:?}"
            );
        }
        // Each link stays inside lexically, but the chain leads outside.
        let (_tmp, result) = extract(
            &[
                Item::Dir("pkg/a/"),
                Item::Link("pkg/a/up", ".."),
                Item::Link("pkg/escape", "a/up/a/up/a/up/.."),
            ],
            1,
        );
        assert!(
            matches!(result, Err(ArchiveError::UnsafeLink { .. })),
            "{result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn files_are_never_written_through_links() {
        // Links are created after the files: `dir/file` lands in a real
        // directory, and the link then collides with it.
        let (tmp, result) = extract(
            &[
                Item::Dir("pkg/sub/"),
                Item::Link("pkg/dir", "sub"),
                Item::File("pkg/dir/file", b"x", 0o644),
            ],
            1,
        );
        assert!(
            matches!(result, Err(ArchiveError::Duplicate(_))),
            "{result:?}"
        );
        let dir = tmp.path().join("dest/dir");
        assert!(fs::symlink_metadata(&dir).unwrap().is_dir());
        assert!(!tmp.path().join("dest/sub/file").exists());
    }

    #[test]
    fn hard_links_become_copies_of_extracted_files() {
        let (tmp, result) = extract(
            &[
                Item::File("pkg/zone/Greenwich", b"tzdata", 0o644),
                Item::File("pkg/bin/tool", b"exe", 0o755),
                Item::HardLink("pkg/zone/GMT", "pkg/zone/Greenwich"),
                Item::HardLink("pkg/other/tool", "./pkg/bin/tool"),
            ],
            1,
        );
        let stats = result.unwrap();
        assert_eq!(stats.files, 4);
        assert_eq!(stats.bytes, 18);
        let dest = tmp.path().join("dest");
        assert_eq!(fs::read(dest.join("zone/GMT")).unwrap(), b"tzdata");
        // An independent copy: changing it leaves the original intact.
        fs::write(dest.join("zone/GMT"), b"changed").unwrap();
        assert_eq!(fs::read(dest.join("zone/Greenwich")).unwrap(), b"tzdata");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(dest.join("other/tool"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn hard_links_outside_the_extracted_files_are_refused() {
        for (link, target) in [
            // Not extracted (yet): later entries, absent, or stripped.
            ("pkg/b", "pkg/a"),
            ("pkg/b", "pkg"),
            // Outside the destination.
            ("pkg/b", "/etc/passwd"),
            ("pkg/b", "pkg/../../etc/passwd"),
            ("pkg/b", "../outside"),
            // A directory, not a file.
            ("pkg/b", "pkg/dir"),
        ] {
            let (tmp, result) = extract(
                &[
                    Item::Dir("pkg/dir/"),
                    Item::HardLink(link, target),
                    Item::File("pkg/a", b"x", 0o644),
                ],
                1,
            );
            assert!(
                matches!(result, Err(ArchiveError::UnsafeHardLink { .. })),
                "{link} -> {target}: {result:?}"
            );
            assert!(!tmp.path().join("dest/b").exists());
        }
        // A hard link to a symbolic link of the archive is refused too.
        let (_tmp, result) = extract(
            &[
                Item::File("pkg/a", b"x", 0o644),
                Item::Link("pkg/l", "a"),
                Item::HardLink("pkg/b", "pkg/l"),
            ],
            1,
        );
        assert!(
            matches!(result, Err(ArchiveError::UnsafeHardLink { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn tar_xz_trees_are_extracted() {
        let tmp = tempfile::tempdir().unwrap();
        let tar_path = tmp.path().join("plain.tar");
        {
            let mut builder = tar::Builder::new(File::create(&tar_path).unwrap());
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(6);
            builder
                .append_data(&mut header, "mysql-1/bin/mysqld", &b"server"[..])
                .unwrap();
            builder.into_inner().unwrap();
        }
        let archive = tmp.path().join("mysql.tar.xz");
        let mut encoder = liblzma::write::XzEncoder::new(File::create(&archive).unwrap(), 1);
        io::copy(&mut File::open(&tar_path).unwrap(), &mut encoder).unwrap();
        encoder.finish().unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let stats = extract_tree(
            &archive,
            ArchiveKind::TarXz,
            &dest,
            1,
            TreeLimits::default(),
        )
        .unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(fs::read(dest.join("bin/mysqld")).unwrap(), b"server");

        // Gzip data announced as xz.
        let wrong = tmp.path().join("wrong.tar.xz");
        tar_gz(&wrong, &[Item::File("a", b"x", 0o644)]);
        let dest = tmp.path().join("dest-wrong");
        fs::create_dir(&dest).unwrap();
        assert!(matches!(
            extract_tree(&wrong, ArchiveKind::TarXz, &dest, 0, TreeLimits::default()),
            Err(ArchiveError::Read(_))
        ));
    }

    #[test]
    fn duplicates_and_limits_are_refused() {
        let (_tmp, result) = extract(
            &[
                Item::File("pkg/a", b"x", 0o644),
                Item::File("pkg/a", b"y", 0o644),
            ],
            1,
        );
        assert!(matches!(result, Err(ArchiveError::Duplicate(_))));

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("big.tar.gz");
        tar_gz(
            &archive,
            &[
                Item::File("a", b"12345", 0o644),
                Item::File("b", b"67890", 0o644),
            ],
        );
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let small = TreeLimits {
            max_total_len: 8,
            max_entries: 10,
        };
        assert!(matches!(
            extract_tree(&archive, ArchiveKind::TarGz, &dest, 0, small),
            Err(ArchiveError::LimitExceeded(_))
        ));
        let few = TreeLimits {
            max_total_len: 100,
            max_entries: 1,
        };
        let dest = tmp.path().join("dest2");
        fs::create_dir(&dest).unwrap();
        assert!(matches!(
            extract_tree(&archive, ArchiveKind::TarGz, &dest, 0, few),
            Err(ArchiveError::LimitExceeded(_))
        ));
    }

    #[test]
    fn zip_trees_are_extracted_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("php.zip");
        zip_file(
            &archive,
            &[("php.exe", b"exe"), ("ext/php_curl.dll", b"dll")],
        );
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let stats =
            extract_tree(&archive, ArchiveKind::Zip, &dest, 0, TreeLimits::default()).unwrap();
        assert_eq!(stats.files, 2);
        assert_eq!(fs::read(dest.join("ext/php_curl.dll")).unwrap(), b"dll");

        let evil = tmp.path().join("evil.zip");
        zip_file(&evil, &[("../evil.dll", b"x")]);
        let dest = tmp.path().join("dest-evil");
        fs::create_dir(&dest).unwrap();
        assert!(matches!(
            extract_tree(&evil, ArchiveKind::Zip, &dest, 0, TreeLimits::default()),
            Err(ArchiveError::UnsafePath(_))
        ));
        assert!(!tmp.path().join("evil.dll").exists());
    }
}
