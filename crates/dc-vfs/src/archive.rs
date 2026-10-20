//! Archives browsed as directories, read-only for now.
//!
//! Only formats whose contents can be listed without decompressing them:
//! zip (and jar and the like), 7z, rar, iso, cab with a directory of
//! their contents, and plain tar, whose headers are read while the data
//! between them is skipped. Compressed streams (tar.gz, tar.xz, ...) have
//! no directory, listing them means decompressing everything; they stay
//! files (Jirka's decision).
//!
//! Reading goes through the system's libarchive (`compress-tools`). The
//! listing is read once and kept; paths inside are virtual: the archive's
//! own path followed by the path within it (`/home/me/lib.zip/src/a.rs`).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use compress_tools::{ArchiveContents, ArchiveIteratorBuilder};

use crate::{Entry, EntryKind, Error, Result};

/// Extensions of archives that are browsed (lower case).
const BROWSABLE: [&str; 9] = ["zip", "jar", "war", "apk", "7z", "rar", "iso", "cab", "tar"];

/// Whether `name` is an archive the panel can enter, by its extension.
#[must_use]
pub fn is_browsable_archive(name: &OsStr) -> bool {
    Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| BROWSABLE.iter().any(|known| extension.eq_ignore_ascii_case(known)))
}

/// The listing of one archive.
#[derive(Debug)]
pub struct ArchiveSource {
    /// The archive file on disk.
    file: PathBuf,
    /// Size and time of the file when listed, to notice it changed.
    stamp: (u64, Option<SystemTime>),
    /// Every item by its path within the archive (`a/b`, root is empty).
    entries: HashMap<PathBuf, Entry>,
    /// Names in each directory within the archive.
    children: HashMap<PathBuf, Vec<OsString>>,
}

impl ArchiveSource {
    /// Reads the directory of `file`.
    ///
    /// # Errors
    /// [`Error::Archive`] when it cannot be read as an archive.
    pub fn open(file: &Path) -> Result<Self> {
        let error = |message: String| Error::Archive { path: file.to_owned(), message };
        let metadata = std::fs::metadata(file).map_err(|err| error(err.to_string()))?;
        let source = File::open(file).map_err(|err| error(err.to_string()))?;

        // The filter sees every header and keeps none, so no data is read.
        let found = Arc::new(Mutex::new(Vec::new()));
        let collect = Arc::clone(&found);
        let mut iterator = ArchiveIteratorBuilder::new(source)
            .filter(move |name, stat| {
                let mode = stat.st_mode;
                let mtime = u64::try_from(stat.st_mtime).ok();
                let size = u64::try_from(stat.st_size).unwrap_or(0);
                collect.lock().unwrap_or_else(PoisonError::into_inner).push((name.to_owned(), mode, size, mtime));
                false
            })
            .build()
            .map_err(|err| error(err.to_string()))?;
        for content in &mut iterator {
            if let ArchiveContents::Err(err) = content {
                return Err(error(err.to_string()));
            }
        }
        drop(iterator);

        let mut archive = Self {
            file: file.to_owned(),
            stamp: (metadata.len(), metadata.modified().ok()),
            entries: HashMap::new(),
            children: HashMap::new(),
        };
        archive.add_directory(PathBuf::new(), 0o755, None);
        let found = std::mem::take(&mut *found.lock().unwrap_or_else(PoisonError::into_inner));
        for (name, mode, size, mtime) in found {
            let directory = name.ends_with('/') || mode & 0o170_000 == 0o040_000;
            let Some(path) = clean(&name) else { continue };
            let modified = mtime.map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds));
            if directory {
                archive.add_directory(path, mode, modified);
            } else {
                let kind = if mode & 0o170_000 == 0o120_000 { EntryKind::Symlink { to_directory: false } } else { EntryKind::File };
                archive.add(path, kind, size, mode, modified);
            }
        }
        for names in archive.children.values_mut() {
            names.sort();
        }
        Ok(archive)
    }

    /// Whether the file on disk is still the one listed.
    #[must_use]
    pub fn is_current(&self) -> bool {
        std::fs::metadata(&self.file).is_ok_and(|metadata| (metadata.len(), metadata.modified().ok()) == self.stamp)
    }

    fn add(&mut self, path: PathBuf, kind: EntryKind, size: u64, mode: u32, modified: Option<SystemTime>) {
        let Some(name) = path.file_name().map(OsStr::to_owned) else { return };
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
        // Zips often list files without their directories.
        if !self.entries.contains_key(&parent) {
            self.add_directory(parent.clone(), 0o755, None);
        }
        if !self.entries.contains_key(&path) {
            self.children.entry(parent).or_default().push(name.clone());
        }
        self.entries.insert(path, Entry { name, kind, size, modified, mode });
    }

    fn add_directory(&mut self, path: PathBuf, mode: u32, modified: Option<SystemTime>) {
        self.children.entry(path.clone()).or_default();
        if path.as_os_str().is_empty() {
            self.entries.entry(path).or_insert(Entry {
                name: OsString::new(),
                kind: EntryKind::Directory,
                size: 0,
                modified: None,
                mode: 0o040_755,
            });
            return;
        }
        let mode = 0o040_000 | (mode & 0o7777);
        if let Some(existing) = self.entries.get_mut(&path) {
            // Listed again with its own data after being made up for its contents.
            existing.mode = mode;
            existing.modified = modified.or(existing.modified);
            return;
        }
        self.add(path, EntryKind::Directory, 0, mode, modified);
    }

    /// Path within the archive of the virtual path `path`.
    fn inner<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.file).ok()
    }

    pub(crate) fn list(&self, dir: &Path) -> Result<Vec<Entry>> {
        let inner = self.inner(dir).ok_or_else(|| missing(dir))?;
        let names = self.children.get(inner).ok_or_else(|| missing(dir))?;
        Ok(names.iter().filter_map(|name| self.entries.get(&inner.join(name)).cloned()).collect())
    }

    pub(crate) fn entry(&self, path: &Path) -> Result<Entry> {
        let inner = self.inner(path).ok_or_else(|| missing(path))?;
        let mut entry = self.entries.get(inner).cloned().ok_or_else(|| missing(path))?;
        if inner.as_os_str().is_empty() {
            entry.name = self.file.file_name().map(OsStr::to_owned).unwrap_or_default();
        }
        Ok(entry)
    }

}

fn missing(path: &Path) -> Error {
    Error::ReadEntry { path: path.to_owned(), source: std::io::ErrorKind::NotFound.into() }
}

impl ArchiveSource {
    /// Bytes of all files at or under `path` (a virtual path), for the
    /// progress of extracting them.
    #[must_use]
    pub fn bytes_under(&self, path: &Path) -> u64 {
        let Some(inner) = self.inner(path) else { return 0 };
        self.entries
            .iter()
            .filter(|(entry_path, entry)| entry.kind == EntryKind::File && entry_path.starts_with(inner))
            .map(|(_, entry)| entry.size)
            .sum()
    }
}

/// Progress of [`extract`].
#[derive(Debug)]
pub enum ExtractEvent<'a> {
    /// A file starts, this is where it goes.
    File(&'a Path),
    /// So many more bytes are written.
    Bytes(u64),
}

/// Extracts the items `selected` (virtual paths: the archive file's path
/// followed by the path within it) of `archive` into `target_dir`, with
/// their paths below `base` (the directory of the archive they are listed
/// in), as copying them out would. `progress` is told about every file and
/// block; returning `false` stops (the result is then `Ok(false)`).
/// Links inside archives are left out.
///
/// # Errors
/// [`Error::Archive`] when the archive cannot be read or a file not written.
pub fn extract(
    archive: &Path,
    base: &Path,
    selected: &[PathBuf],
    target_dir: &Path,
    mut progress: impl FnMut(ExtractEvent<'_>) -> bool,
) -> Result<bool> {
    let error = |path: &Path, message: String| Error::Archive { path: path.to_owned(), message };
    let inner = |path: &Path| path.strip_prefix(archive).map(Path::to_path_buf).ok();
    let base = inner(base).ok_or_else(|| error(archive, String::new()))?;
    let wanted: Vec<PathBuf> = selected.iter().filter_map(|path| inner(path)).collect();

    let source = File::open(archive).map_err(|err| error(archive, err.to_string()))?;
    let filter_wanted = wanted.clone();
    let iterator = ArchiveIteratorBuilder::new(source)
        .filter(move |name, _| clean(name).is_some_and(|path| filter_wanted.iter().any(|wanted| path.starts_with(wanted))))
        .build()
        .map_err(|err| error(archive, err.to_string()))?;

    // The file being written, its target, mode and time.
    let mut current: Option<(File, PathBuf, u32, Option<SystemTime>)> = None;
    for content in iterator {
        match content {
            ArchiveContents::StartOfEntry(name, stat) => {
                let Some(relative) = clean(&name).and_then(|path| path.strip_prefix(&base).ok().map(Path::to_path_buf)) else {
                    continue;
                };
                let target = target_dir.join(relative);
                let mode = stat.st_mode;
                if name.ends_with('/') || mode & 0o170_000 == 0o040_000 {
                    std::fs::create_dir_all(&target).map_err(|err| error(&target, err.to_string()))?;
                } else if mode & 0o170_000 == 0o100_000 || mode & 0o170_000 == 0 {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| error(parent, err.to_string()))?;
                    }
                    let file = File::create(&target).map_err(|err| error(&target, err.to_string()))?;
                    let modified = u64::try_from(stat.st_mtime).ok().map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds));
                    if !progress(ExtractEvent::File(&target)) {
                        return Ok(false);
                    }
                    current = Some((file, target, mode, modified));
                }
            }
            ArchiveContents::DataChunk(bytes) => {
                if let Some((file, target, ..)) = &mut current {
                    std::io::Write::write_all(file, &bytes).map_err(|err| error(target, err.to_string()))?;
                }
                if !progress(ExtractEvent::Bytes(bytes.len() as u64)) {
                    return Ok(false);
                }
            }
            ArchiveContents::EndOfEntry => {
                if let Some((file, target, mode, modified)) = current.take() {
                    finish_file(&file, mode, modified).map_err(|err| error(&target, err.to_string()))?;
                }
            }
            ArchiveContents::Err(err) => return Err(error(archive, err.to_string())),
        }
    }
    Ok(true)
}

/// Mode bits (writable for the owner, files from archives often are not)
/// and the time from the archive.
fn finish_file(file: &File, mode: u32, modified: Option<SystemTime>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o777 != 0 {
        file.set_permissions(std::fs::Permissions::from_mode((mode & 0o7777) | 0o200))?;
    }
    if let Some(modified) = modified {
        file.set_modified(modified)?;
    }
    Ok(())
}

/// A path within an archive made safe: relative, without `.`, `..` and
/// a leading `/`. `None` when nothing is left.
fn clean(name: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::ParentDir => {
                path.pop();
            }
            _ => {}
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Opened archives, shared by the panels and kept while they are used.
#[derive(Debug, Default)]
pub(crate) struct ArchiveCache {
    open: Mutex<Vec<Arc<ArchiveSource>>>,
}

/// So many archives stay listed; more are listed again when entered.
const KEPT: usize = 8;

impl ArchiveCache {
    pub(crate) fn get(&self, file: &Path) -> Result<Arc<ArchiveSource>> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = open.iter().position(|archive| archive.file == file && archive.is_current()) {
            let archive = open.remove(index);
            open.push(Arc::clone(&archive));
            return Ok(archive);
        }
        drop(open);
        // Listed without the lock, other archives stay usable meanwhile.
        let archive = Arc::new(ArchiveSource::open(file)?);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.retain(|kept| kept.file != file);
        open.push(Arc::clone(&archive));
        if open.len() > KEPT {
            open.remove(0);
        }
        Ok(archive)
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// A zip made by bsdtar (libarchive's own tool), without directory entries.
    fn sample_zip(dir: &Path) -> Option<PathBuf> {
        let tree = dir.join("tree");
        std::fs::create_dir_all(tree.join("src/deep")).ok()?;
        std::fs::write(tree.join("readme.txt"), b"hello").ok()?;
        std::fs::write(tree.join("src/deep/a.rs"), b"fn main() {}").ok()?;
        let zip = dir.join("sample.zip");
        let status = Command::new("bsdtar")
            .args(["--format", "zip", "-cf"])
            .arg(&zip)
            .args(["-C"])
            .arg(&tree)
            .args(["readme.txt", "src/deep/a.rs"])
            .status()
            .ok()?;
        status.success().then_some(zip)
    }

    #[test]
    fn lists_a_zip_as_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(zip) = sample_zip(tmp.path()) else { return }; // No bsdtar here.
        let archive = ArchiveSource::open(&zip).unwrap();

        let names = |dir: &Path| {
            let mut names: Vec<_> = archive.list(dir).unwrap().into_iter().map(|e| (e.name, e.kind)).collect();
            names.sort_by(|a, b| a.0.cmp(&b.0));
            names
        };
        assert_eq!(names(&zip), [("readme.txt".into(), EntryKind::File), ("src".into(), EntryKind::Directory)]);
        assert_eq!(names(&zip.join("src/deep")), [("a.rs".into(), EntryKind::File)]);
        assert_eq!(archive.entry(&zip.join("readme.txt")).unwrap().size, 5);
        assert!(archive.list(&zip.join("nothing")).is_err());
        assert!(archive.is_current());
    }

    /// A minimal stored (uncompressed) Microsoft Cabinet with two files,
    /// one of them in a directory (cab paths use backslashes).
    fn sample_cab(dir: &Path) -> PathBuf {
        let files: [(&str, &[u8]); 2] = [("readme.txt", b"ahoj z cabu\n"), ("docs\\index.md", b"# docs\n")];
        // DOS date and time: 20.10.2026 15:30.
        let (year, month, day, hour, minute) = (2026_u16 - 1980, 10_u16, 20_u16, 15_u16, 30_u16);
        let (date, time) = ((year << 9) | (month << 5) | day, (hour << 11) | (minute << 5));
        let mut entries = Vec::new();
        let mut payload: Vec<u8> = Vec::new();
        for (name, content) in files {
            let offset = u32::try_from(payload.len()).unwrap();
            entries.extend(u32::try_from(content.len()).unwrap().to_le_bytes());
            entries.extend(offset.to_le_bytes());
            for value in [0_u16, date, time, 0x20] {
                entries.extend(value.to_le_bytes());
            }
            entries.extend(name.as_bytes());
            entries.push(0);
            payload.extend(content);
        }
        let data_start = 36 + 8 + entries.len();
        let total = data_start + 8 + payload.len();
        let mut cab = b"MSCF".to_vec();
        for value in [0, u32::try_from(total).unwrap(), 0, 36 + 8, 0] {
            cab.extend(value.to_le_bytes());
        }
        cab.extend([3, 1]); // version 1.3
        for value in [1_u16, 2, 0, 0, 0] {
            cab.extend(value.to_le_bytes()); // folders, files, flags, set, index
        }
        cab.extend(u32::try_from(data_start).unwrap().to_le_bytes());
        cab.extend([1_u16, 0].iter().flat_map(|value| value.to_le_bytes())); // one data block, stored
        cab.extend(entries);
        cab.extend(0_u32.to_le_bytes()); // no checksum
        let size = u16::try_from(payload.len()).unwrap().to_le_bytes();
        cab.extend(size);
        cab.extend(size);
        cab.extend(payload);
        let path = dir.join("sample.cab");
        std::fs::write(&path, cab).unwrap();
        path
    }

    #[test]
    fn lists_a_cab_with_its_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let cab = sample_cab(tmp.path());
        let archive = ArchiveSource::open(&cab).unwrap();
        let mut root: Vec<_> = archive.list(&cab).unwrap().into_iter().map(|e| (e.name, e.kind)).collect();
        root.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(root, [("docs".into(), EntryKind::Directory), ("readme.txt".into(), EntryKind::File)]);
        assert_eq!(archive.entry(&cab.join("docs/index.md")).unwrap().size, 7);
    }

    #[test]
    fn extracts_the_selected_items_below_their_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(zip) = sample_zip(tmp.path()) else { return };
        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();

        // From the directory "src" of the archive, its "deep" goes out.
        let mut files = 0;
        let mut bytes = 0;
        let done = extract(&zip, &zip.join("src"), &[zip.join("src/deep")], &out, |event| {
            match event {
                ExtractEvent::File(_) => files += 1,
                ExtractEvent::Bytes(count) => bytes += count,
            }
            true
        })
        .unwrap();
        assert!(done);
        assert_eq!(std::fs::read(out.join("deep/a.rs")).unwrap(), b"fn main() {}");
        assert!(!out.join("readme.txt").exists(), "only the selection");
        assert_eq!((files, bytes), (1, 12));
        assert_eq!(ArchiveSource::open(&zip).unwrap().bytes_under(&zip.join("src")), 12);

        // Stopped right away: nothing more happens.
        let stopped = extract(&zip, &zip, &[zip.join("readme.txt")], &out, |_| false).unwrap();
        assert!(!stopped);
    }

    #[test]
    fn browsable_by_extension_but_not_compressed_tars() {
        for name in ["a.zip", "B.7Z", "x.rar", "disk.iso", "plain.tar"] {
            assert!(is_browsable_archive(OsStr::new(name)), "{name}");
        }
        for name in ["a.tar.gz", "a.tgz", "a.tar.xz", "a.txt", "zip"] {
            assert!(!is_browsable_archive(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn paths_inside_are_made_safe() {
        assert_eq!(clean("./a/../b/c"), Some(PathBuf::from("b/c")));
        assert_eq!(clean("/etc/passwd"), Some(PathBuf::from("etc/passwd")));
        assert_eq!(clean("../.."), None);
    }
}
