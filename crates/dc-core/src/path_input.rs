//! A path typed by hand into the path field above the file list
//! (`cm_EditPath`, a double click on the path): what the text means and
//! where it leads on disk.
//!
//! As the original (`onKeyRETURN` of `TFileViewHeader`): `~` and
//! environment variables are replaced, a relative path is relative to the
//! panel's directory, an existing file means its directory with the cursor
//! on it. A path that does not exist yet can be created (our addition).

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The path `text` stands for, typed in a panel showing `base`: trimmed,
/// `~` replaced by `home`, `$NAME` and `${NAME}` by the values of `var`
/// (unknown ones stay as typed), relative to `base`, with `.` and `..`
/// resolved by the text alone (as the panel goes up: to the parent shown).
/// `None` for a text without a path.
#[must_use]
pub fn expand(text: &str, base: &Path, home: &Path, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text = replace_vars(text, &var);
    let path = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => base.join(&text),
    };
    Some(normalize(&path))
}

/// `$NAME` and `${NAME}` replaced by their values.
fn replace_vars(text: &str, var: &impl Fn(&str) -> Option<OsString>) -> String {
    let is_name_char = |ch: char| ch == '_' || ch.is_ascii_alphanumeric();
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(dollar) = rest.find('$') {
        result.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        let (name, tail) = if let Some(braced) = after.strip_prefix('{') {
            braced.find('}').map_or(("", after), |end| (&braced[..end], &braced[end + 1..]))
        } else {
            let end = after.find(|ch| !is_name_char(ch)).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        let valid = name.starts_with(|ch: char| ch == '_' || ch.is_ascii_alphabetic()) && name.chars().all(is_name_char);
        if let Some(value) = valid.then(|| var(name)).flatten() {
            result.push_str(&value.to_string_lossy());
            rest = tail;
        } else {
            result.push('$');
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

/// `.` dropped and `..` taking the previous component away, never above `/`.
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// Where a typed path leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// An existing directory (a link to one too).
    Directory,
    /// An existing file: shown in its directory, with the cursor on it.
    File { dir: PathBuf, name: OsString },
    /// Not there yet: `existing` is the deepest directory on the way that
    /// is, `missing` the directories to create under it, outermost first.
    /// `allowed`: whether this user may create directories in `existing`
    /// (write and enter it, on a disk not mounted read-only).
    Missing { existing: PathBuf, missing: Vec<OsString>, allowed: bool },
    /// On the way is something that is not a directory (a file, a broken
    /// link), nothing can be created under it.
    Blocked(PathBuf),
}

/// Where the absolute `path` leads on the local disk.
#[must_use]
pub fn destination(path: &Path) -> Destination {
    for ancestor in path.ancestors() {
        match fs::metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => {
                if ancestor == path {
                    return Destination::Directory;
                }
                let missing = path
                    .strip_prefix(ancestor)
                    .map(|rest| rest.components().map(|part| part.as_os_str().to_owned()).collect())
                    .unwrap_or_default();
                let allowed = rustix::fs::access(ancestor, rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK).is_ok();
                return Destination::Missing { existing: ancestor.to_owned(), missing, allowed };
            }
            Ok(_) => {
                return match (ancestor == path, path.parent(), path.file_name()) {
                    (true, Some(dir), Some(name)) => Destination::File { dir: dir.to_owned(), name: name.to_owned() },
                    _ => Destination::Blocked(ancestor.to_owned()),
                };
            }
            // A link pointing nowhere is there, but no directory.
            Err(_) if fs::symlink_metadata(ancestor).is_ok() => return Destination::Blocked(ancestor.to_owned()),
            Err(_) => {}
        }
    }
    // Not even `/`: a relative path, which `expand` never gives.
    Destination::Blocked(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_in(text: &str) -> Option<PathBuf> {
        let var = |name: &str| match name {
            "DATA" => Some(OsString::from("/srv/data")),
            "EMPTY" => Some(OsString::new()),
            _ => None,
        };
        expand(text, Path::new("/home/me/work"), Path::new("/home/me"), var)
    }

    #[test]
    fn expands_home_variables_and_relative_paths() {
        assert_eq!(expand_in("   "), None);
        assert_eq!(expand_in(" /etc/ "), Some(PathBuf::from("/etc")));
        assert_eq!(expand_in("~"), Some(PathBuf::from("/home/me")));
        assert_eq!(expand_in("~/a/b"), Some(PathBuf::from("/home/me/a/b")));
        // Not the home of someone else, a name starting with a tilde.
        assert_eq!(expand_in("~other"), Some(PathBuf::from("/home/me/work/~other")));
        assert_eq!(expand_in("src/lib"), Some(PathBuf::from("/home/me/work/src/lib")));
        assert_eq!(expand_in("$DATA/x"), Some(PathBuf::from("/srv/data/x")));
        assert_eq!(expand_in("${DATA}x"), Some(PathBuf::from("/srv/datax")));
        assert_eq!(expand_in("/a$EMPTY/b"), Some(PathBuf::from("/a/b")));
        // Unknown, unfinished or not a name: as typed.
        assert_eq!(expand_in("/a/$NOPE/b"), Some(PathBuf::from("/a/$NOPE/b")));
        assert_eq!(expand_in("/a/${DATA"), Some(PathBuf::from("/a/${DATA")));
        assert_eq!(expand_in("/cena $5"), Some(PathBuf::from("/cena $5")));
        assert_eq!(expand_in("/a/$"), Some(PathBuf::from("/a/$")));
    }

    #[test]
    fn resolves_dots_by_the_text() {
        assert_eq!(expand_in(".."), Some(PathBuf::from("/home/me")));
        assert_eq!(expand_in("./a/../b/."), Some(PathBuf::from("/home/me/work/b")));
        assert_eq!(expand_in("/../../x"), Some(PathBuf::from("/x")));
        assert_eq!(expand_in("/a//b///"), Some(PathBuf::from("/a/b")));
    }

    #[test]
    fn finds_what_is_there_and_what_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/file.txt"), b"").unwrap();
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("broken")).unwrap();

        assert_eq!(destination(&root.join("dir")), Destination::Directory);
        assert_eq!(
            destination(&root.join("dir/file.txt")),
            Destination::File { dir: root.join("dir"), name: OsString::from("file.txt") }
        );
        assert_eq!(
            destination(&root.join("dir/a/b/c")),
            Destination::Missing {
                existing: root.join("dir"),
                missing: vec![OsString::from("a"), OsString::from("b"), OsString::from("c")],
                allowed: true,
            }
        );
        assert_eq!(destination(&root.join("dir/file.txt/sub")), Destination::Blocked(root.join("dir/file.txt")));
        assert_eq!(destination(&root.join("broken/sub")), Destination::Blocked(root.join("broken")));
    }

    #[test]
    fn tells_when_directories_cannot_be_created() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = destination(&locked.join("new"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Root may write anywhere, the check then says so too. The owner
        // of a new directory is whoever runs the test.
        let root = std::os::unix::fs::MetadataExt::uid(&fs::metadata(&locked).unwrap()) == 0;
        assert_eq!(result, Destination::Missing { existing: locked, missing: vec![OsString::from("new")], allowed: root });
    }
}
