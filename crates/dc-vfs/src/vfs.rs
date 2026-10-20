//! What the panels browse: the local file system with archives in it.
//!
//! A path is local unless an archive file is on its way: then the rest of
//! the path is inside that archive (`/home/me/lib.zip/src`). Archive files
//! in local listings are marked [`EntryKind::Archive`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::archive::{ArchiveCache, ArchiveSource, is_browsable_archive};
use crate::{Entry, EntryKind, Error, FileSource, LocalFs, Result};

#[derive(Debug, Default)]
pub struct Vfs {
    local: LocalFs,
    archives: ArchiveCache,
}

/// Where a path leads.
enum Place {
    Local,
    /// Inside the archive file at this path.
    Archive(PathBuf),
}

impl Vfs {
    /// Lists the archive `file` (slow for big ones), so that browsing it
    /// afterwards is instant. For a worker thread before entering it.
    ///
    /// # Errors
    /// [`Error::Archive`] when it cannot be read.
    pub fn open_archive(&self, file: &Path) -> Result<()> {
        self.archives.get(file).map(|_| ())
    }

    /// The archive file `path` is inside of (or is), if any.
    #[must_use]
    pub fn archive_of(&self, path: &Path) -> Option<PathBuf> {
        match place(path) {
            Place::Archive(file) => Some(file),
            Place::Local => None,
        }
    }

    fn archive(&self, file: &Path) -> Result<Arc<ArchiveSource>> {
        self.archives.get(file)
    }
}

/// A local path is one that exists as a directory or a file that is no
/// archive; otherwise the nearest existing file on the way may be an
/// archive the rest of the path is in.
fn place(path: &Path) -> Place {
    if let Ok(metadata) = fs::metadata(path) {
        let archive = metadata.is_file() && path.file_name().is_some_and(is_browsable_archive);
        return if archive { Place::Archive(path.to_owned()) } else { Place::Local };
    }
    for ancestor in path.ancestors().skip(1) {
        if let Ok(metadata) = fs::metadata(ancestor) {
            let archive = metadata.is_file() && ancestor.file_name().is_some_and(is_browsable_archive);
            return if archive { Place::Archive(ancestor.to_owned()) } else { Place::Local };
        }
    }
    Place::Local
}

/// Archive files of a local listing become [`EntryKind::Archive`].
fn mark_archive(mut entry: Entry) -> Entry {
    if entry.kind == EntryKind::File && is_browsable_archive(&entry.name) {
        entry.kind = EntryKind::Archive;
    }
    entry
}

impl FileSource for Vfs {
    fn list(&self, dir: &Path) -> Result<Vec<Entry>> {
        match place(dir) {
            Place::Local => Ok(self.local.list(dir)?.into_iter().map(mark_archive).collect()),
            // Archives inside are shown as archives too, but not entered:
            // they have to be copied out first (Jirka's choice).
            Place::Archive(file) => Ok(self.archive(&file)?.list(dir)?.into_iter().map(mark_archive).collect()),
        }
    }

    fn entry(&self, path: &Path) -> Result<Entry> {
        match place(path) {
            Place::Archive(file) if file != path => self.archive(&file)?.entry(path),
            _ => self.local.entry(path).map(mark_archive),
        }
    }

    fn create_directory(&self, path: &Path) -> Result<()> {
        match place(path) {
            Place::Local => self.local.create_directory(path),
            Place::Archive(_) => Err(Error::ReadOnly { path: path.to_owned() }),
        }
    }

    fn remove(&self, path: &Path) -> Result<()> {
        match place(path) {
            Place::Archive(file) if file != path => Err(Error::ReadOnly { path: path.to_owned() }),
            _ => self.local.remove(path),
        }
    }

    fn move_to_trash(&self, path: &Path) -> Result<()> {
        match place(path) {
            Place::Archive(file) if file != path => Err(Error::ReadOnly { path: path.to_owned() }),
            _ => self.local.move_to_trash(path),
        }
    }
}
