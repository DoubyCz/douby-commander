use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::{Entry, EntryKind, Error, FileSource, Result};

/// The local file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFs;

impl FileSource for LocalFs {
    fn list(&self, dir: &Path) -> Result<Vec<Entry>> {
        let read_dir = fs::read_dir(dir).map_err(|source| Error::ReadDir { path: dir.to_owned(), source })?;

        let entries = read_dir
            .filter_map(std::result::Result::ok)
            .filter_map(|dir_entry| {
                let metadata = dir_entry.metadata().ok()?;
                Some(to_entry(dir_entry.file_name(), &dir_entry.path(), &metadata))
            })
            .collect();

        Ok(entries)
    }

    fn entry(&self, path: &Path) -> Result<Entry> {
        let metadata = fs::symlink_metadata(path).map_err(|source| Error::ReadEntry { path: path.to_owned(), source })?;
        let name = path.file_name().map_or_else(|| path.as_os_str().to_owned(), ToOwned::to_owned);
        Ok(to_entry(name, path, &metadata))
    }

    fn create_directory(&self, path: &Path) -> Result<()> {
        if fs::symlink_metadata(path).is_ok() {
            return Err(Error::DirectoryExists { path: path.to_owned() });
        }
        fs::create_dir_all(path).map_err(|source| Error::CreateDir { path: path.to_owned(), source })
    }

    fn remove(&self, path: &Path) -> Result<()> {
        let error = |source| Error::Remove { path: path.to_owned(), source };
        // symlink_metadata: a link to a directory is removed as a link.
        let metadata = fs::symlink_metadata(path).map_err(error)?;
        if metadata.is_dir() { fs::remove_dir(path) } else { fs::remove_file(path) }.map_err(error)
    }

    fn move_to_trash(&self, path: &Path) -> Result<()> {
        // The crate only resolves the parent directory, a link goes to the
        // trash as a link and its target stays.
        trash::delete(path).map_err(|err| Error::Trash { path: path.to_owned(), message: err.to_string() })
    }
}

/// `metadata` is of the item itself (links not followed).
fn to_entry(name: OsString, path: &Path, metadata: &fs::Metadata) -> Entry {
    let file_type = metadata.file_type();
    // Size of a directory entry says nothing to the user and a link
    // should show the size of what it points to.
    let (kind, size) = if file_type.is_symlink() {
        match fs::metadata(path) {
            Ok(target) if target.is_dir() => (EntryKind::Symlink { to_directory: true }, 0),
            Ok(target) => (EntryKind::Symlink { to_directory: false }, target.len()),
            Err(_) => (EntryKind::Symlink { to_directory: false }, 0),
        }
    } else if file_type.is_dir() {
        (EntryKind::Directory, 0)
    } else if file_type.is_file() {
        (EntryKind::File, metadata.len())
    } else {
        (EntryKind::Other, 0)
    };

    Entry { name, kind, size, modified: metadata.modified().ok(), mode: metadata.mode() }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn lists_files_directories_and_links() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("file.txt"), b"hello").unwrap();
        fs::create_dir(tmp.path().join("dir")).unwrap();
        symlink(tmp.path().join("dir"), tmp.path().join("link-to-dir")).unwrap();
        symlink(tmp.path().join("missing"), tmp.path().join("broken-link")).unwrap();

        let mut entries = LocalFs.list(tmp.path()).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        let summary: Vec<_> = entries.iter().map(|e| (e.name.to_str().unwrap(), e.kind)).collect();
        assert_eq!(
            summary,
            [
                ("broken-link", EntryKind::Symlink { to_directory: false }),
                ("dir", EntryKind::Directory),
                ("file.txt", EntryKind::File),
                ("link-to-dir", EntryKind::Symlink { to_directory: true }),
            ]
        );
        assert_eq!(entries[2].size, 5);
    }

    #[test]
    fn missing_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(LocalFs.list(&tmp.path().join("nope")), Err(Error::ReadDir { .. })));
    }

    #[test]
    fn single_entry_does_not_follow_links() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("dir")).unwrap();
        symlink(tmp.path().join("dir"), tmp.path().join("link")).unwrap();

        let entry = LocalFs.entry(&tmp.path().join("link")).unwrap();
        assert_eq!(entry.name, "link");
        assert_eq!(entry.kind, EntryKind::Symlink { to_directory: true });
        assert!(matches!(LocalFs.entry(&tmp.path().join("nope")), Err(Error::ReadEntry { .. })));
    }

    #[test]
    fn creates_nested_directories_but_not_existing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        LocalFs.create_directory(&tmp.path().join("a/b/c")).unwrap();
        assert!(tmp.path().join("a/b/c").is_dir());

        assert!(matches!(LocalFs.create_directory(&tmp.path().join("a")), Err(Error::DirectoryExists { .. })));
        fs::write(tmp.path().join("file"), b"").unwrap();
        assert!(matches!(LocalFs.create_directory(&tmp.path().join("file")), Err(Error::DirectoryExists { .. })));
    }

    #[test]
    fn removes_files_links_and_empty_directories_only() {
        let tmp = tempfile::tempdir().unwrap();
        let keep = tmp.path().join("keep");
        fs::create_dir(&keep).unwrap();
        fs::write(keep.join("data"), b"x").unwrap();
        let link = tmp.path().join("link-to-keep");
        symlink(&keep, &link).unwrap();
        let file = tmp.path().join("file.txt");
        fs::write(&file, b"x").unwrap();
        let empty = tmp.path().join("empty");
        fs::create_dir(&empty).unwrap();

        LocalFs.remove(&link).unwrap();
        LocalFs.remove(&file).unwrap();
        LocalFs.remove(&empty).unwrap();
        assert!(fs::symlink_metadata(&link).is_err() && !file.exists() && !empty.exists());
        assert!(keep.join("data").is_file(), "target of a removed link must survive");

        let err = LocalFs.remove(&keep).unwrap_err();
        assert!(matches!(err, Error::Remove { .. }), "a non-empty directory is not removed");
        assert!(!err.reason().contains(&*keep.to_string_lossy()), "reason is without the path");
    }
}
