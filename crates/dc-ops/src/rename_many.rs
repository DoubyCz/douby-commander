//! Renaming many files at once, for the multi-rename tool.
//!
//! The new names may be the old names of other files of the list (`a` to
//! `b` and `b` to `a`), so those go through a temporary name first, as in
//! the original. A file that is not on the list is never overwritten.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use dc_core::multi_rename::Rename;
use rustix::fs::{CWD, RenameFlags, renameat_with};

/// One rename that failed, with why.
#[derive(Debug)]
pub struct RenameFailure {
    pub rename: Rename,
    pub error: io::Error,
}

/// Does `renames`; returns those that failed, the others are done.
#[must_use]
pub fn rename_many(renames: &[Rename]) -> Vec<RenameFailure> {
    let old: std::collections::HashSet<&Path> = renames.iter().map(|rename| rename.from.as_path()).collect();
    let mut failures = Vec::new();
    let mut parked = Vec::new();
    for (index, rename) in renames.iter().enumerate() {
        let result = if old.contains(rename.to.as_path()) {
            let temporary = temporary_name(&rename.to, index);
            rename_new(&rename.from, &temporary).map(|()| parked.push((temporary, rename)))
        } else {
            rename_new(&rename.from, &rename.to)
        };
        if let Err(error) = result {
            failures.push(RenameFailure { rename: rename.clone(), error });
        }
    }
    for (temporary, rename) in parked {
        if let Err(error) = rename_new(&temporary, &rename.to) {
            // Its old name back, if still free.
            let _ = rename_new(&temporary, &rename.from);
            failures.push(RenameFailure { rename: rename.clone(), error });
        }
    }
    failures
}

/// A free name next to `path` for the time of the renaming.
fn temporary_name(path: &Path, index: usize) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    // Taken only by leftovers of another run; a few tries are plenty, and
    // the last one, should it exist, makes that rename fail safely.
    (0..100)
        .map(|attempt| dir.join(format!(".douby-rename-{}-{index}-{attempt}", std::process::id())))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .unwrap_or_else(|| dir.join(".douby-rename"))
}

/// Renames `from` to `to` unless `to` exists. A file system that cannot
/// say so atomically is asked first.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(err) if err == rustix::io::Errno::INVAL || err == rustix::io::Errno::NOSYS => {
            if fs::symlink_metadata(to).is_ok() && !same_file(from, to) {
                return Err(io::Error::from(ErrorKind::AlreadyExists));
            }
            fs::rename(from, to)
        }
        Err(err) => Err(err.into()),
    }
}

/// Whether both paths are one file (a change of letter case only, on a
/// file system that ignores it).
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(dir).expect("readable").map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn swaps_and_chains_without_losing_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (name, content) in [("a", "A"), ("b", "B"), ("c", "C")] {
            fs::write(dir.path().join(name), content).expect("writable");
        }
        let rename = |from: &str, to: &str| Rename { from: dir.path().join(from), to: dir.path().join(to) };
        // a and b swap, c takes a free name.
        let failures = rename_many(&[rename("a", "b"), rename("b", "a"), rename("c", "d")]);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(names(dir.path()), ["a", "b", "d"]);
        assert_eq!(fs::read_to_string(dir.path().join("a")).expect("readable"), "B");
        assert_eq!(fs::read_to_string(dir.path().join("b")).expect("readable"), "A");
    }

    #[test]
    fn never_overwrites_a_file_off_the_list() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("a"), "A").expect("writable");
        fs::write(dir.path().join("other"), "O").expect("writable");
        let failures = rename_many(&[Rename { from: dir.path().join("a"), to: dir.path().join("other") }]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(dir.path().join("other")).expect("readable"), "O");
    }
}
