use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use dc_vfs::{EntryKind, FileSource};

use crate::mask::MaskList;
use crate::quick_search::Pattern;
use crate::{FileItem, Sorting};

/// State of one file panel: current directory, its items, cursor and selection.
#[derive(Debug)]
pub struct Panel {
    path: PathBuf,
    items: Vec<FileItem>,
    cursor: usize,
    sorting: Sorting,
    show_hidden: bool,
    range: Option<RangeSelection>,
    /// While Shift is held, Shift+arrows only mark or only unmark, as the
    /// first item decided; `None` until then and after Shift is released.
    held_mark: Option<bool>,
}

/// Running Shift+PgUp/PgDn/Home/End or Shift+click selection.
///
/// Mirrors `FRangeSelectionStartIndex`, `FRangeSelectionEndIndex` and
/// `FRangeSelectionState` of `TOrderedFileView`: the range always spans from
/// the anchor to the cursor. Unlike the original, items leaving the range
/// keep their state: one hold of Shift never turns anything back, so where
/// the range ended before does not matter.
#[derive(Debug, Clone, Copy)]
struct RangeSelection {
    anchor: usize,
    /// Whether the range marks or unmarks, decided by the anchor item.
    mark: bool,
}

/// Result of pressing Enter on the item under the cursor.
#[derive(Debug, PartialEq, Eq)]
pub enum Activation {
    /// The panel moved to another directory.
    Navigated,
    /// The cursor is on a file, the caller decides how to open it.
    OpenFile(PathBuf),
    /// The cursor is on an archive to browse; the caller lists it (which
    /// may take a while) and then shows this path.
    EnterArchive(PathBuf),
}

/// Totals for the status line under the panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionInfo {
    pub selected_bytes: u64,
    pub total_bytes: u64,
    pub selected_files: usize,
    pub total_files: usize,
    pub selected_dirs: usize,
    pub total_dirs: usize,
}

impl Panel {
    /// Creates an empty panel, call [`Panel::load`] to fill it.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            items: Vec::new(),
            cursor: 0,
            sorting: Sorting::default(),
            show_hidden: false,
            range: None,
            held_mark: None,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn items(&self) -> &[FileItem] {
        &self.items
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub fn sorting(&self) -> Sorting {
        self.sorting
    }

    #[must_use]
    pub fn current_item(&self) -> Option<&FileItem> {
        self.items.get(self.cursor)
    }

    /// Moves the cursor. Any ordinary move also sets a new starting point
    /// for the next range selection.
    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.items.len().saturating_sub(1));
        self.range = None;
    }

    /// Lists `path` and shows it. With `focus` the cursor lands on that name,
    /// otherwise on the first item. On error the panel stays as it was.
    ///
    /// # Errors
    /// When the directory cannot be listed.
    pub fn load(&mut self, source: &dyn FileSource, path: PathBuf, focus: Option<&OsStr>) -> dc_vfs::Result<()> {
        let entries = source.list(&path)?;

        let parent = path.parent().is_some().then(FileItem::parent);
        let items = parent
            .into_iter()
            .chain(entries.into_iter().map(FileItem::new))
            .filter(|item| self.show_hidden || !item.is_hidden())
            .collect();

        self.path = path;
        self.items = items;
        self.sort_items();
        self.set_cursor(focus.and_then(|name| self.position_of(name)).unwrap_or(0));
        Ok(())
    }

    /// Lists the current directory again, keeping the cursor and selection by name.
    /// When the item under the cursor is gone, the cursor stays on the same
    /// row, which is then the item that followed it.
    ///
    /// # Errors
    /// When the directory cannot be listed.
    pub fn reload(&mut self, source: &dyn FileSource) -> dc_vfs::Result<()> {
        let focus = self.current_item().map(|item| item.entry().name.clone());
        self.reload_focusing(source, focus.as_deref())
    }

    /// Like [`Panel::reload`], but puts the cursor on `focus` (e.g. a just
    /// created directory).
    ///
    /// # Errors
    /// When the directory cannot be listed.
    pub fn reload_focusing(&mut self, source: &dyn FileSource, focus: Option<&OsStr>) -> dc_vfs::Result<()> {
        let old_cursor = self.cursor;
        let selected: HashSet<OsString> =
            self.items.iter().filter(|item| item.selected).map(|item| item.entry().name.clone()).collect();

        self.load(source, self.path.clone(), focus)?;
        for item in &mut self.items {
            item.selected = selected.contains(&item.entry().name);
        }
        if focus.is_some_and(|name| self.position_of(name).is_none()) {
            self.set_cursor(old_cursor);
        }
        Ok(())
    }

    /// Reloads after a change on disk. When the directory itself is gone
    /// (deleted from the other panel), goes to the nearest existing parent.
    ///
    /// # Errors
    /// When not even the root can be listed.
    pub fn reload_or_leave(&mut self, source: &dyn FileSource) -> dc_vfs::Result<()> {
        let Err(err) = self.reload(source) else { return Ok(()) };
        let ancestors: Vec<PathBuf> = self.path.ancestors().skip(1).map(Path::to_path_buf).collect();
        for ancestor in ancestors {
            if self.load(source, ancestor, None).is_ok() {
                return Ok(());
            }
        }
        Err(err)
    }

    /// What file operations work on: the marked items, or the item under
    /// the cursor when nothing is marked. Never `..`.
    #[must_use]
    pub fn targets(&self) -> Vec<&FileItem> {
        let marked: Vec<&FileItem> = self.items.iter().filter(|item| item.selected).collect();
        if !marked.is_empty() {
            return marked;
        }
        self.current_item().filter(|item| !item.is_parent()).into_iter().collect()
    }

    /// F7 offers the name under the cursor, without extension for files,
    /// as `cm_MakeDir` of the original does.
    #[must_use]
    pub fn new_directory_suggestion(&self) -> String {
        match self.current_item() {
            Some(item) if item.is_parent() => String::new(),
            Some(item) if item.is_directory_like() => item.display_name().to_owned(),
            Some(item) => item.name_no_ext().to_owned(),
            None => String::new(),
        }
    }

    /// Enter: opens a directory, goes up on `..`, reports a file to open.
    ///
    /// # Errors
    /// When the target directory cannot be listed.
    pub fn activate(&mut self, source: &dyn FileSource) -> dc_vfs::Result<Option<Activation>> {
        let Some(item) = self.current_item() else { return Ok(None) };

        if item.is_parent() {
            return Ok(self.go_up(source)?.then_some(Activation::Navigated));
        }
        let target = self.path.join(&item.entry().name);
        if item.is_directory_like() {
            self.load(source, target, None)?;
            Ok(Some(Activation::Navigated))
        } else if item.entry().kind == EntryKind::Archive {
            // Listing it may take a while, the caller does it in the background.
            Ok(Some(Activation::EnterArchive(target)))
        } else {
            Ok(Some(Activation::OpenFile(target)))
        }
    }

    /// Backspace / Ctrl+PgUp: goes to the parent directory and puts the cursor
    /// on the directory we came from. Returns `false` in the root.
    ///
    /// # Errors
    /// When the parent directory cannot be listed.
    pub fn go_up(&mut self, source: &dyn FileSource) -> dc_vfs::Result<bool> {
        let (Some(parent), Some(came_from)) = (self.path.parent(), self.path.file_name()) else {
            return Ok(false);
        };
        let (parent, came_from) = (parent.to_owned(), came_from.to_owned());
        self.load(source, parent, Some(&came_from))?;
        Ok(true)
    }

    /// Shift+arrows: marks (or unmarks) the items from the cursor up to the
    /// one before the item `step` away, then moves there; in the list and
    /// icons views a step is a whole column or row. The first item of a
    /// Shift held down decides: turned to marked, everything until Shift
    /// is released ([`Self::end_held_mark`]) only gets marked, never
    /// toggled back, and the other way round. At the first or last item
    /// only that item changes.
    pub fn mark_and_move(&mut self, step: isize) {
        self.range = None;
        let Some(last) = self.items.len().checked_sub(1) else { return };
        let cursor = self.cursor;
        let target = cursor.saturating_add_signed(step).min(last);
        let Some(item) = self.items.get(cursor) else { return };
        let mark = *self.held_mark.get_or_insert(!item.selected);
        if target == cursor {
            self.set_selected(cursor, mark);
            return;
        }
        let range = if target > cursor { cursor..target } else { target + 1..cursor + 1 };
        for index in range {
            self.set_selected(index, mark);
        }
        self.set_cursor(target);
    }

    /// Shift was released: the next Shift+arrow, Shift+click or
    /// Shift+PgUp/PgDn/Home/End decides anew, a range starts anew.
    pub fn end_held_mark(&mut self) {
        self.held_mark = None;
        self.range = None;
    }

    /// Insert and Shift+Down (`forward`), Shift+Up: toggles the item under
    /// the cursor, then moves. Going back over the same items unmarks them.
    pub fn toggle_selection_and_move(&mut self, forward: bool) {
        self.toggle_selection_at_cursor();
        let target = if forward { self.cursor + 1 } else { self.cursor.saturating_sub(1) };
        self.set_cursor(target);
    }

    /// Space: toggles the selection under the cursor. `..` cannot be selected.
    pub fn toggle_selection_at_cursor(&mut self) {
        self.range = None;
        if let Some(item) = self.items.get_mut(self.cursor)
            && !item.is_parent()
        {
            item.selected = !item.selected;
        }
    }

    /// Ctrl+click: toggles the clicked item and moves the cursor there. When
    /// nothing is marked yet, the item under the old cursor gets marked too,
    /// so two Ctrl+clicks mark two items.
    pub fn toggle_clicked(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        if index != self.cursor && !self.items.iter().any(FileItem::is_selected) {
            self.set_selected(self.cursor, true);
        }
        self.set_cursor(index);
        self.toggle_selection_at_cursor();
    }

    /// Shift+PgUp/PgDn/Home/End and Shift+click: marks everything from the
    /// starting point to `target` and moves the cursor to `target`.
    ///
    /// The starting point is where the cursor was before the first range
    /// step, further steps grow or shrink the same range. With
    /// `include_target` false (PgUp/PgDn) the item the cursor lands on stays
    /// out of the range, unless it is the first or last item.
    pub fn select_range_to(&mut self, target: usize, include_target: bool) {
        let Some(last) = self.items.len().checked_sub(1) else { return };
        let target = target.min(last);

        let anchor = self.cursor;
        let range = if let Some(range) = self.range {
            range
        } else {
            // One hold of Shift only marks or only unmarks, as decided by
            // its first item (with Shift+arrows too).
            let mark = *self.held_mark.get_or_insert(!self.items[anchor].selected);
            RangeSelection { anchor, mark }
        };

        let end = if include_target || target == 0 || target == last {
            target
        } else if target > range.anchor {
            target - 1
        } else if target < range.anchor {
            target + 1
        } else {
            target
        };

        // Items left out when the range shrinks stay as they are: within
        // one hold of Shift nothing turns back (the original unmarks them).
        let span = range.anchor.min(end)..=range.anchor.max(end);
        for index in span {
            self.set_selected(index, range.mark);
        }

        self.cursor = target;
        self.range = Some(range);
    }

    /// `cm_MarkMarkAll` / `cm_MarkUnmarkAll`: every item except `..`.
    pub fn mark_all(&mut self, selected: bool) {
        self.range = None;
        for item in self.items.iter_mut().filter(|item| !item.is_parent()) {
            item.selected = selected;
        }
    }

    /// `cm_MarkPlus` / `cm_MarkMinus`: items whose name matches `masks`,
    /// directories too, never `..`.
    pub fn mark_group(&mut self, masks: &MaskList, selected: bool) {
        self.range = None;
        for item in self.items.iter_mut().filter(|item| !item.is_parent()) {
            if masks.matches(&item.entry().name.to_string_lossy()) {
                item.selected = selected;
            }
        }
    }

    /// `cm_MarkInvert`: every item except `..`.
    pub fn invert_marks(&mut self) {
        self.range = None;
        for item in self.items.iter_mut().filter(|item| !item.is_parent()) {
            item.selected = !item.selected;
        }
    }

    fn set_selected(&mut self, index: usize, selected: bool) {
        if let Some(item) = self.items.get_mut(index)
            && !item.is_parent()
        {
            item.selected = selected;
        }
    }

    /// Changes sorting, the cursor stays on the same item.
    pub fn set_sorting(&mut self, sorting: Sorting) {
        let focus = self.current_item().map(|item| item.entry().name.clone());
        self.sorting = sorting;
        self.sort_items();
        if let Some(index) = focus.and_then(|name| self.position_of(&name)) {
            self.set_cursor(index);
        }
    }

    /// Quick search: moves the cursor to the first matching item, starting at
    /// the cursor itself and wrapping around. Returns `false` when nothing
    /// matches, the cursor then stays where it was.
    pub fn quick_search(&mut self, pattern: &Pattern) -> bool {
        let len = self.items.len();
        let found = (0..len)
            .map(|offset| (self.cursor + offset) % len)
            .find(|&index| !self.items[index].is_parent() && pattern.matches(self.items[index].display_name()));
        if let Some(index) = found {
            self.set_cursor(index);
        }
        found.is_some()
    }

    #[must_use]
    pub fn selection_info(&self) -> SelectionInfo {
        self.items.iter().filter(|item| !item.is_parent()).fold(SelectionInfo::default(), |mut info, item| {
            if item.is_directory_like() {
                // A calculated size counts as the bytes of the directory.
                let size = item.size();
                info.total_dirs += 1;
                info.total_bytes += size;
                if item.selected {
                    info.selected_dirs += 1;
                    info.selected_bytes += size;
                }
            } else {
                let size = item.entry().size;
                info.total_files += 1;
                info.total_bytes += size;
                if item.selected {
                    info.selected_files += 1;
                    info.selected_bytes += size;
                }
            }
            info
        })
    }

    /// Stores the calculated size of the directory `name`; its index, or
    /// `None` when it is not listed (any more).
    pub fn set_calculated_size(&mut self, name: &OsStr, size: u64) -> Option<usize> {
        let index = self.position_of(name)?;
        let item = &mut self.items[index];
        if !item.is_directory_like() {
            return None;
        }
        item.calculated_size = Some(size);
        Some(index)
    }

    fn sort_items(&mut self) {
        let sorting = self.sorting;
        self.items.sort_by(|a, b| sorting.compare(a, b));
    }

    /// Index of the item `name`, `..` excluded.
    #[must_use]
    pub fn position_of(&self, name: &OsStr) -> Option<usize> {
        self.items.iter().position(|item| !item.is_parent() && item.entry().name == name)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dc_vfs::LocalFs;

    use super::*;

    fn names(panel: &Panel) -> Vec<&str> {
        panel.items().iter().map(FileItem::display_name).collect()
    }

    fn marked_names(panel: &Panel) -> Vec<&str> {
        panel.items().iter().filter(|item| item.is_selected()).map(FileItem::display_name).collect()
    }

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("docs")).unwrap();
        fs::write(tmp.path().join("docs/readme.md"), b"# hi").unwrap();
        fs::write(tmp.path().join("big.bin"), vec![0; 2048]).unwrap();
        fs::write(tmp.path().join("small.txt"), b"abc").unwrap();
        fs::write(tmp.path().join(".hidden"), b"").unwrap();
        tmp
    }

    #[test]
    fn loads_sorted_listing_with_parent_and_without_hidden() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        assert_eq!(names(&panel), ["..", "docs", "big.bin", "small.txt"]);
    }

    #[test]
    fn enter_and_go_back_up_restores_cursor() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();

        panel.set_cursor(1);
        assert_eq!(panel.activate(&LocalFs).unwrap(), Some(Activation::Navigated));
        assert_eq!(panel.path(), tmp.path().join("docs"));
        assert_eq!(names(&panel), ["..", "readme.md"]);

        // Enter on ".." returns to the parent with the cursor on "docs".
        assert_eq!(panel.activate(&LocalFs).unwrap(), Some(Activation::Navigated));
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.current_item().unwrap().display_name(), "docs");

        panel.set_cursor(2);
        assert_eq!(panel.activate(&LocalFs).unwrap(), Some(Activation::OpenFile(tmp.path().join("big.bin"))));
    }

    #[test]
    fn failed_load_keeps_previous_state() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        assert!(panel.load(&LocalFs, tmp.path().join("missing"), None).is_err());
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.items().len(), 4);
    }

    #[test]
    fn mark_and_move_by_a_row_or_column() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            fs::write(tmp.path().join(name), b"").unwrap();
        }
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();

        // `..` a b c d e f g; from "a" three on: a b c marked, cursor on d.
        panel.set_cursor(1);
        panel.mark_and_move(3);
        assert_eq!(marked_names(&panel), ["a", "b", "c"]);
        assert_eq!(panel.cursor(), 4);
        // Back with Shift still held: only marking, d c b, cursor on a.
        panel.mark_and_move(-3);
        assert_eq!(marked_names(&panel), ["a", "b", "c", "d"]);
        assert_eq!(panel.cursor(), 1);
        // Shift released and pressed again on a marked item: only unmarking,
        // also over items not marked (e, f stay unmarked).
        panel.end_held_mark();
        panel.set_cursor(4);
        panel.mark_and_move(3);
        assert_eq!(marked_names(&panel), ["a", "b", "c"]);
        assert_eq!(panel.cursor(), 7);
        panel.mark_and_move(-1);
        assert_eq!(marked_names(&panel), ["a", "b", "c"], "unmarking g, not toggling it");
        panel.end_held_mark();
        panel.set_cursor(1);
        for name in ["a", "b", "c"] {
            assert!(marked_names(&panel).contains(&name));
        }
        panel.mark_and_move(1);
        panel.mark_and_move(1);
        panel.mark_and_move(1);
        assert_eq!(marked_names(&panel), Vec::<&str>::new());
        panel.end_held_mark();
        // Past the end: up to the last item, which stays unmarked.
        panel.set_cursor(5);
        panel.mark_and_move(10);
        assert_eq!(panel.cursor(), 7);
        assert_eq!(marked_names(&panel), ["e", "f"]);
        // At the last item: only it changes, to marked as Shift is held.
        panel.mark_and_move(3);
        assert_eq!(marked_names(&panel), ["e", "f", "g"]);
    }

    #[test]
    fn insert_selects_and_moves_parent_is_never_selected() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();

        panel.toggle_selection_and_move(true); // ".."
        panel.toggle_selection_and_move(true); // docs
        panel.toggle_selection_and_move(true); // big.bin
        assert_eq!(panel.cursor(), 3);

        let selected: Vec<_> =
            panel.items().iter().filter(|i| i.is_selected()).map(FileItem::display_name).collect();
        assert_eq!(selected, ["docs", "big.bin"]);

        let info = panel.selection_info();
        assert_eq!(
            info,
            SelectionInfo {
                selected_bytes: 2048,
                total_bytes: 2051,
                selected_files: 1,
                total_files: 2,
                selected_dirs: 1,
                total_dirs: 1,
            }
        );

        // Selection survives a reload.
        panel.reload(&LocalFs).unwrap();
        assert_eq!(panel.selection_info().selected_files, 1);
        assert_eq!(panel.cursor(), 3);
    }

    #[test]
    fn resorting_keeps_cursor_on_the_same_item() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        panel.set_cursor(3); // small.txt

        panel.set_sorting(Sorting { column: crate::SortColumn::Size, direction: crate::SortDirection::Descending });
        assert_eq!(names(&panel), ["..", "docs", "big.bin", "small.txt"]);
        panel.set_sorting(Sorting { column: crate::SortColumn::Size, direction: crate::SortDirection::Ascending });
        assert_eq!(names(&panel), ["..", "docs", "small.txt", "big.bin"]);
        assert_eq!(panel.current_item().unwrap().display_name(), "small.txt");
    }

    #[test]
    fn quick_search_starts_at_cursor_and_wraps() {
        let tmp = fixture();
        fs::write(tmp.path().join("bin.sh"), b"").unwrap();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        assert_eq!(names(&panel), ["..", "docs", "big.bin", "bin.sh", "small.txt"]);

        assert!(panel.quick_search(&Pattern::new("b")));
        assert_eq!(panel.cursor(), 2);
        // Narrowing the text keeps the cursor if the current item still matches.
        assert!(panel.quick_search(&Pattern::new("bi")));
        assert_eq!(panel.cursor(), 2);
        assert!(panel.quick_search(&Pattern::new("bin")));
        assert_eq!(panel.cursor(), 3);
        // From here "d" is found by wrapping around, ".." is skipped.
        assert!(panel.quick_search(&Pattern::new("d")));
        assert_eq!(panel.cursor(), 1);
        assert!(!panel.quick_search(&Pattern::new("zzz")));
        assert_eq!(panel.cursor(), 1);
    }

    /// Panel with `..` and files `f0` to `f{count - 1}`.
    fn numbered(count: usize) -> (tempfile::TempDir, Panel) {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..count {
            fs::write(tmp.path().join(format!("f{i:02}")), b"").unwrap();
        }
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        (tmp, panel)
    }

    fn marked(panel: &Panel) -> Vec<usize> {
        panel.items().iter().enumerate().filter(|(_, item)| item.is_selected()).map(|(i, _)| i).collect()
    }

    #[test]
    fn shift_arrows_toggle_and_going_back_unmarks() {
        let (_tmp, mut panel) = numbered(5);
        panel.set_cursor(1);
        panel.toggle_selection_and_move(true);
        panel.toggle_selection_and_move(true);
        assert_eq!(marked(&panel), [1, 2]);
        assert_eq!(panel.cursor(), 3);

        panel.toggle_selection_and_move(false); // toggles 3, moves to 2
        panel.toggle_selection_and_move(false); // unmarks 2, moves to 1
        assert_eq!(marked(&panel), [1, 3]);
        assert_eq!(panel.cursor(), 1);
    }

    #[test]
    fn page_range_stops_before_target_and_never_turns_back() {
        let (_tmp, mut panel) = numbered(10); // indexes 0 ("..") to 10
        panel.set_cursor(2);

        panel.select_range_to(5, false); // Shift+PgDn
        assert_eq!(marked(&panel), [2, 3, 4]);
        assert_eq!(panel.cursor(), 5);

        panel.select_range_to(8, false); // Shift+PgDn again extends
        assert_eq!(marked(&panel), [2, 3, 4, 5, 6, 7]);

        // Shift+PgUp in the same hold: nothing gets unmarked.
        panel.select_range_to(4, false);
        assert_eq!(marked(&panel), [2, 3, 4, 5, 6, 7]);

        panel.select_range_to(0, false); // past the anchor: other side, ".." is never marked
        assert_eq!(marked(&panel), [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(panel.cursor(), 0);
    }

    #[test]
    fn home_end_include_target_and_last_item_is_reached() {
        let (_tmp, mut panel) = numbered(5);
        panel.set_cursor(3);
        panel.select_range_to(5, true); // Shift+End
        assert_eq!(marked(&panel), [3, 4, 5]);

        // PgDn hitting the end of the list marks the last item as well.
        panel.set_cursor(1);
        panel.select_range_to(99, false);
        assert_eq!(marked(&panel), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn range_from_marked_anchor_unmarks() {
        let (_tmp, mut panel) = numbered(5);
        panel.set_cursor(1);
        panel.select_range_to(5, true);
        assert_eq!(marked(&panel), [1, 2, 3, 4, 5]);

        // Shift held anew, starting on a marked item: the range unmarks.
        panel.end_held_mark();
        panel.set_cursor(2);
        panel.select_range_to(4, true);
        assert_eq!(marked(&panel), [1, 5]);
    }

    #[test]
    fn ctrl_click_marks_old_cursor_when_nothing_is_marked() {
        let (_tmp, mut panel) = numbered(5);
        panel.set_cursor(1);
        panel.toggle_clicked(4);
        assert_eq!(marked(&panel), [1, 4]);
        assert_eq!(panel.cursor(), 4);

        panel.toggle_clicked(2);
        panel.toggle_clicked(4);
        assert_eq!(marked(&panel), [1, 2]);
    }

    #[test]
    fn shift_click_range_starts_at_cursor_and_combines_with_earlier_marks() {
        let (_tmp, mut panel) = numbered(8);
        panel.set_cursor(1);
        panel.toggle_clicked(2); // marks 1 and 2
        panel.set_cursor(5);
        panel.select_range_to(7, true);
        assert_eq!(marked(&panel), [1, 2, 5, 6, 7]);
    }

    #[test]
    fn mark_all_unmark_all_and_invert_skip_parent() {
        let (_tmp, mut panel) = numbered(3);
        panel.set_cursor(1);
        panel.toggle_selection_at_cursor();

        panel.invert_marks();
        assert_eq!(marked(&panel), [2, 3]);
        panel.mark_all(true);
        assert_eq!(marked(&panel), [1, 2, 3]);
        panel.mark_all(false);
        assert!(marked(&panel).is_empty());
    }

    #[test]
    fn mark_group_adds_and_removes_by_mask() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        let options = crate::mask::MaskOptions::default();

        panel.mark_group(&MaskList::new("*.txt;*.bin", options), true);
        assert_eq!(marked_names(&panel), ["big.bin", "small.txt"]);
        // Directories too, `..` never.
        panel.mark_group(&MaskList::new("*", options), true);
        assert_eq!(marked_names(&panel), ["docs", "big.bin", "small.txt"]);
        panel.mark_group(&MaskList::new("*.BIN", options), false);
        assert_eq!(marked_names(&panel), ["docs", "small.txt"]);
    }

    #[test]
    fn targets_are_marked_items_or_the_current_one_never_parent() {
        let (_tmp, mut panel) = numbered(4);
        panel.set_cursor(0);
        assert!(panel.targets().is_empty(), "\"..\" is never a target");

        panel.set_cursor(2);
        let names = |panel: &Panel| panel.targets().iter().map(|i| i.display_name().to_owned()).collect::<Vec<_>>();
        assert_eq!(names(&panel), ["f01"]);

        panel.select_range_to(3, true);
        panel.set_cursor(4);
        assert_eq!(names(&panel), ["f01", "f02"]);
    }

    #[test]
    fn new_directory_suggestion_follows_the_original() {
        let tmp = fixture();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, tmp.path().to_owned(), None).unwrap();
        let suggestion = |panel: &mut Panel, index| {
            panel.set_cursor(index);
            panel.new_directory_suggestion()
        };
        assert_eq!(suggestion(&mut panel, 0), ""); // ..
        assert_eq!(suggestion(&mut panel, 1), "docs");
        assert_eq!(suggestion(&mut panel, 2), "big"); // big.bin
    }

    #[test]
    fn reload_after_delete_keeps_row_and_after_mkdir_focuses_new_name() {
        let (tmp, mut panel) = numbered(4);
        panel.set_cursor(2); // f01
        fs::remove_file(tmp.path().join("f01")).unwrap();
        panel.reload(&LocalFs).unwrap();
        assert_eq!(panel.current_item().unwrap().display_name(), "f02");

        fs::create_dir(tmp.path().join("new")).unwrap();
        panel.reload_focusing(&LocalFs, Some(OsStr::new("new"))).unwrap();
        assert_eq!(panel.current_item().unwrap().display_name(), "new");
    }

    #[test]
    fn reload_of_deleted_directory_goes_to_nearest_parent() {
        let tmp = fixture();
        let deep = tmp.path().join("docs/a/b");
        fs::create_dir_all(&deep).unwrap();
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, deep, None).unwrap();

        fs::remove_dir_all(tmp.path().join("docs/a")).unwrap();
        panel.reload_or_leave(&LocalFs).unwrap();
        assert_eq!(panel.path(), tmp.path().join("docs"));
    }

    #[test]
    fn root_has_no_parent_item() {
        let mut panel = Panel::new(PathBuf::new());
        panel.load(&LocalFs, PathBuf::from("/"), None).unwrap();
        assert!(!panel.items().iter().any(FileItem::is_parent));
        assert!(!panel.go_up(&LocalFs).unwrap());
    }
}
