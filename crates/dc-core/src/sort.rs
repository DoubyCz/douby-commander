use std::cmp::Ordering;

use crate::FileItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortColumn {
    Name,
    Extension,
    Size,
    Modified,
    Attributes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    #[must_use]
    pub fn reversed(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sorting {
    pub column: SortColumn,
    pub direction: SortDirection,
}

impl Default for Sorting {
    fn default() -> Self {
        Self { column: SortColumn::Name, direction: SortDirection::Ascending }
    }
}

impl Sorting {
    /// Order of two items as Double Commander shows them with default options:
    /// `..` first, then directories, then files, each group sorted by the
    /// chosen column and case insensitive name as a tie breaker.
    #[must_use]
    pub fn compare(self, a: &FileItem, b: &FileItem) -> Ordering {
        b.is_parent()
            .cmp(&a.is_parent())
            .then_with(|| b.is_directory_like().cmp(&a.is_directory_like()))
            .then_with(|| {
                let by_column = self.compare_column(a, b);
                match self.direction {
                    SortDirection::Ascending => by_column,
                    SortDirection::Descending => by_column.reverse(),
                }
            })
            .then_with(|| a.sort_name.cmp(&b.sort_name))
    }

    fn compare_column(self, a: &FileItem, b: &FileItem) -> Ordering {
        let (ea, eb) = (a.entry(), b.entry());
        match self.column {
            SortColumn::Name => a.sort_name_no_ext.cmp(&b.sort_name_no_ext),
            SortColumn::Extension => a.sort_extension.cmp(&b.sort_extension),
            SortColumn::Size => ea.size.cmp(&eb.size),
            SortColumn::Modified => ea.modified.cmp(&eb.modified),
            SortColumn::Attributes => ea.mode.cmp(&eb.mode),
        }
    }
}

#[cfg(test)]
mod tests {
    use dc_vfs::{Entry, EntryKind};

    use super::*;

    fn item(name: &str, kind: EntryKind, size: u64) -> FileItem {
        FileItem::new(Entry { name: name.into(), kind, size, modified: None, mode: 0 })
    }

    fn sorted(sorting: Sorting, mut items: Vec<FileItem>) -> Vec<String> {
        items.sort_by(|a, b| sorting.compare(a, b));
        items.iter().map(|i| i.display_name().to_owned()).collect()
    }

    fn sample() -> Vec<FileItem> {
        vec![
            item("b.txt", EntryKind::File, 10),
            item("Zeta", EntryKind::Directory, 0),
            item("a.rs", EntryKind::File, 300),
            item("link", EntryKind::Symlink { to_directory: true }, 0),
            FileItem::parent(),
            item("C.md", EntryKind::File, 20),
        ]
    }

    #[test]
    fn parent_then_directories_then_files_by_name_case_insensitive() {
        assert_eq!(sorted(Sorting::default(), sample()), ["..", "link", "Zeta", "a.rs", "b.txt", "C.md"]);
    }

    #[test]
    fn descending_keeps_parent_and_directories_on_top() {
        let sorting = Sorting { column: SortColumn::Name, direction: SortDirection::Descending };
        assert_eq!(sorted(sorting, sample()), ["..", "Zeta", "link", "C.md", "b.txt", "a.rs"]);
    }

    #[test]
    fn by_size_and_by_extension() {
        let by_size = Sorting { column: SortColumn::Size, direction: SortDirection::Descending };
        assert_eq!(sorted(by_size, sample()), ["..", "link", "Zeta", "a.rs", "C.md", "b.txt"]);

        let by_ext = Sorting { column: SortColumn::Extension, direction: SortDirection::Ascending };
        assert_eq!(sorted(by_ext, sample()), ["..", "link", "Zeta", "C.md", "a.rs", "b.txt"]);
    }
}
