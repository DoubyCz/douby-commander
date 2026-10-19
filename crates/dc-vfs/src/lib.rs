//! File sources: everything a panel can list and browse.
//!
//! The original Double Commander models this as `TFileSource` with many
//! subclasses (local, archive, WFX plugin, GIO, ...). Here it is a trait, and
//! for now only the local file system implements it.

mod local;
pub mod mounts;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fmt, io};

use dc_i18n::{fill, gettext};

pub use local::LocalFs;

/// What kind of object a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    /// Symbolic link. `to_directory` says whether the target is a directory,
    /// which decides if the link behaves like a folder in the panel.
    Symlink { to_directory: bool },
    /// Device, FIFO, socket and other special files.
    Other,
}

/// One item of a directory listing.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Unix mode bits including the file type (`st_mode`).
    pub mode: u32,
}

impl Entry {
    /// Directories and links to directories open on Enter and sort first.
    #[must_use]
    pub fn is_directory_like(&self) -> bool {
        matches!(self.kind, EntryKind::Directory | EntryKind::Symlink { to_directory: true })
    }
}

/// Failure of a file source. `Display` says what failed (translated, with
/// the path), [`Error::reason`] and `source()` say why.
#[derive(Debug)]
pub enum Error {
    ReadDir { path: PathBuf, source: io::Error },
    ReadEntry { path: PathBuf, source: io::Error },
    DirectoryExists { path: PathBuf },
    CreateDir { path: PathBuf, source: io::Error },
    Remove { path: PathBuf, source: io::Error },
    Trash { path: PathBuf, message: String },
}

impl Error {
    /// Why it failed (e.g. "Permission denied"), for messages that show it
    /// below what failed. Texts of the system and of the trash library are
    /// in English for now.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::ReadDir { source, .. }
            | Self::ReadEntry { source, .. }
            | Self::CreateDir { source, .. }
            | Self::Remove { source, .. } => source.to_string(),
            Self::DirectoryExists { .. } => String::new(),
            Self::Trash { message, .. } => message.clone(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (template, path) = match self {
            Self::ReadDir { path, .. } => (gettext("Cannot read directory {path}"), path),
            Self::ReadEntry { path, .. } => (gettext("Cannot read {path}"), path),
            // Texts of the original: rsMsgErrDirExists, rsMsgErrForceDir, rsMsgNotDelete.
            Self::DirectoryExists { path } => (gettext("Directory {path} exists!"), path),
            Self::CreateDir { path, .. } => (gettext("Can not create directory {path}!"), path),
            Self::Remove { path, .. } => (gettext("Can not delete file {path}"), path),
            Self::Trash { path, .. } => (gettext("Can not delete \"{path}\" to trash!"), path),
        };
        f.write_str(&fill(&template, &[("path", &path.display())]))
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadDir { source, .. }
            | Self::ReadEntry { source, .. }
            | Self::CreateDir { source, .. }
            | Self::Remove { source, .. } => Some(source),
            Self::DirectoryExists { .. } | Self::Trash { .. } => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Bytes of all files in the tree under `dir` (Space on a directory in
/// the original). Links are not followed and count as nothing, unreadable
/// parts are left out. `None` once `cancel` is set, checked at every item.
#[must_use]
pub fn tree_size(dir: &Path, cancel: &std::sync::atomic::AtomicBool) -> Option<u64> {
    use std::sync::atomic::Ordering;

    let mut total: u64 = 0;
    let mut pending = vec![dir.to_owned()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        for entry in entries.filter_map(std::result::Result::ok) {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            // DirEntry::metadata does not follow links.
            let Ok(metadata) = entry.metadata() else { continue };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    (!cancel.load(Ordering::Relaxed)).then_some(total)
}

/// A browsable tree of files.
///
/// `Send + Sync` because file operations use it from worker threads.
pub trait FileSource: Send + Sync {
    /// Lists the entries of `dir`, without `.` and `..`.
    ///
    /// # Errors
    /// Fails when the directory itself cannot be opened or read.
    /// Entries that disappear or cannot be inspected during listing are skipped.
    fn list(&self, dir: &Path) -> Result<Vec<Entry>>;

    /// Information about one item, links are not followed (a link to a
    /// directory is [`EntryKind::Symlink`]).
    ///
    /// # Errors
    /// When the item does not exist or cannot be inspected.
    fn entry(&self, path: &Path) -> Result<Entry>;

    /// Creates `path` including missing parent directories.
    ///
    /// # Errors
    /// [`Error::DirectoryExists`] when something of that name exists already,
    /// [`Error::CreateDir`] when it cannot be created.
    fn create_directory(&self, path: &Path) -> Result<()>;

    /// Removes one item: a file, a link (never what it points to) or an
    /// empty directory. Trees are deleted item by item by the delete
    /// operation, so that every failure can be answered on its own.
    ///
    /// # Errors
    /// [`Error::Remove`] with the reason from the system.
    fn remove(&self, path: &Path) -> Result<()>;

    /// Moves a file or a whole directory tree to the desktop trash.
    ///
    /// # Errors
    /// [`Error::Trash`] when there is no usable trash for the item.
    fn move_to_trash(&self, path: &Path) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn tree_size_sums_files_without_following_links() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("tree");
        fs::create_dir_all(tree.join("a/b")).unwrap();
        fs::write(tree.join("one"), [0_u8; 100]).unwrap();
        fs::write(tree.join("a/b/two"), [0_u8; 23]).unwrap();
        let outside = tmp.path().join("big");
        fs::write(&outside, [0_u8; 5000]).unwrap();
        symlink(&outside, tree.join("link")).unwrap();

        assert_eq!(tree_size(&tree, &AtomicBool::new(false)), Some(123));
        assert_eq!(tree_size(&tree, &AtomicBool::new(true)), None, "cancelled");
    }
}
