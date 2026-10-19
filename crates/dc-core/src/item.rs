use std::ffi::OsString;

use dc_vfs::{Entry, EntryKind};

/// A directory entry prepared for display in a panel.
///
/// Display strings and lowercase sort keys are computed once on load,
/// sorting a large directory then does no allocations.
#[derive(Debug, Clone)]
pub struct FileItem {
    entry: Entry,
    is_parent: bool,
    display_name: String,
    name_no_ext: String,
    extension: String,
    pub(crate) sort_name_no_ext: String,
    pub(crate) sort_extension: String,
    pub(crate) sort_name: String,
    pub(crate) selected: bool,
    /// Size of a directory's contents once calculated (Space), forgotten
    /// with the listing.
    pub(crate) calculated_size: Option<u64>,
}

impl FileItem {
    #[must_use]
    pub fn new(entry: Entry) -> Self {
        Self::build(entry, false)
    }

    /// The `..` item at the top of every non-root directory.
    pub(crate) fn parent() -> Self {
        let entry = Entry { name: OsString::from(".."), kind: EntryKind::Directory, size: 0, modified: None, mode: 0 };
        Self::build(entry, true)
    }

    fn build(entry: Entry, is_parent: bool) -> Self {
        let display_name = entry.name.to_string_lossy().into_owned();
        let (name_no_ext, extension) = if entry.is_directory_like() {
            (display_name.clone(), String::new())
        } else {
            split_extension(&display_name)
        };

        Self {
            sort_name_no_ext: name_no_ext.to_lowercase(),
            sort_extension: extension.to_lowercase(),
            sort_name: display_name.to_lowercase(),
            entry,
            is_parent,
            display_name,
            name_no_ext,
            extension,
            selected: false,
            calculated_size: None,
        }
    }

    #[must_use]
    pub fn entry(&self) -> &Entry {
        &self.entry
    }

    /// Size of a directory's contents, when calculated.
    #[must_use]
    pub fn calculated_size(&self) -> Option<u64> {
        self.calculated_size
    }

    /// Bytes the item stands for: a file's size, a directory's calculated
    /// size, otherwise nothing.
    #[must_use]
    pub fn size(&self) -> u64 {
        if self.is_directory_like() { self.calculated_size.unwrap_or(0) } else { self.entry.size }
    }

    #[must_use]
    pub fn is_parent(&self) -> bool {
        self.is_parent
    }

    #[must_use]
    pub fn is_directory_like(&self) -> bool {
        self.entry.is_directory_like()
    }

    #[must_use]
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Full name for display (lossy UTF-8).
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Name without extension, directories keep their whole name.
    #[must_use]
    pub fn name_no_ext(&self) -> &str {
        &self.name_no_ext
    }

    /// Extension without the dot, empty for directories and dot files.
    #[must_use]
    pub fn extension(&self) -> &str {
        &self.extension
    }

    pub(crate) fn is_hidden(&self) -> bool {
        !self.is_parent && self.display_name.starts_with('.')
    }
}

/// Splits `name.ext` at the last dot. A leading dot (`.bashrc`) or a trailing
/// dot (`file.`) does not start an extension.
fn split_extension(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => (name[..dot].to_owned(), name[dot + 1..].to_owned()),
        _ => (name.to_owned(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_extension_like_double_commander() {
        let split = |name| split_extension(name);
        assert_eq!(split("archive.tar.gz"), ("archive.tar".into(), "gz".into()));
        assert_eq!(split(".bashrc"), (".bashrc".into(), String::new()));
        assert_eq!(split(".config.toml"), (".config".into(), "toml".into()));
        assert_eq!(split("Makefile"), ("Makefile".into(), String::new()));
        assert_eq!(split("file."), ("file.".into(), String::new()));
    }

    #[test]
    fn directories_have_no_extension() {
        let entry = Entry { name: "my.dir".into(), kind: EntryKind::Directory, size: 0, modified: None, mode: 0 };
        let item = FileItem::new(entry);
        assert_eq!(item.name_no_ext(), "my.dir");
        assert_eq!(item.extension(), "");
    }
}
