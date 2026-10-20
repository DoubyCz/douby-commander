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
