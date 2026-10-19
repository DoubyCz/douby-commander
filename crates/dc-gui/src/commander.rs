//! The two panels, which one is active, and the single place where keys
//! become commands and commands get executed.

use std::cell::{Cell, OnceCell};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use dc_core::command::{Command, TrashChoice};
use dc_core::keymap::{Key, Keymap, Shortcut};
use dc_i18n::{fill, gettext, ngettext, plural_count};
use dc_ops::{DeleteMode, DeleteOperation, Destination, TransferMode, TransferOperation};
use dc_vfs::FileSource;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::copy_dialog;
use crate::launch::{self, LaunchError};
use crate::operations::{OperationsUi, QueueMemory};
use crate::panel_view::{PanelView, ViewMode};
use crate::splitter::Splitter;
use crate::settings::Settings;
use crate::{dialogs, keys};

pub struct Commander {
    panels: [Rc<PanelView>; 2],
    active: Cell<usize>,
    keymap: Keymap,
    settings: Settings,
    operations: Rc<OperationsUi>,
    /// Set by the window once the panels are laid out.
    splitter: OnceCell<Rc<Splitter>>,
}

impl Commander {
    pub fn new(panels: [Rc<PanelView>; 2], keymap: Keymap, settings: Settings, operations: Rc<OperationsUi>) -> Self {
        Self { panels, active: Cell::new(0), keymap, settings, operations, splitter: OnceCell::new() }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_splitter(&self, splitter: Rc<Splitter>) {
        // Only one window and one divider; a second call changes nothing.
        let _ = self.splitter.set(splitter);
    }

    pub fn panels(&self) -> &[Rc<PanelView>; 2] {
        &self.panels
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn active(&self) -> &Rc<PanelView> {
        &self.panels[self.active.get()]
    }

    pub fn set_active(&self, index: usize) {
        self.active.set(index);
        for (i, panel) in self.panels.iter().enumerate() {
            panel.set_active(i == index);
        }
    }

    /// Runs `command` on the panel of `side` (0 left, 1 right), which becomes
    /// active first, for the buttons that belong to one panel.
    pub fn execute_on(self: &Rc<Self>, side: usize, command: Command) {
        self.set_active(side);
        self.execute(command);
        self.active().focus();
    }

    pub fn execute(self: &Rc<Self>, command: Command) {
        let panel = self.active();
        match command {
            Command::Open => panel.open_current(),
            Command::ChangeDirToParent => panel.go_up(),
            Command::EnterDir => {
                // Only into a directory: not up from `..`, and files stay closed.
                if panel.panel().current_item().is_some_and(|item| !item.is_parent() && item.is_directory_like()) {
                    panel.open_current();
                }
            }
            Command::FocusSwap => {
                let other = 1 - self.active.get();
                self.set_active(other);
                self.panels[other].focus();
            }
            Command::ToggleMark => panel.toggle_selection(),
            Command::ToggleMarkMoveDown => panel.toggle_selection_and_move(true),
            Command::ToggleMarkMoveUp => panel.toggle_selection_and_move(false),
            Command::MarkRange(step) => panel.select_range(step),
            Command::View => {
                if let Some(path) = panel.current_file() {
                    self.edit(&path, launch::edit_in_terminal(&path));
                }
            }
            Command::Edit => {
                if let Some(path) = panel.current_file() {
                    self.edit(&path, launch::edit_in_desktop_editor(&path));
                }
            }
            Command::Copy => self.transfer(TransferMode::Copy, false),
            Command::CopySamePanel => self.transfer(TransferMode::Copy, true),
            Command::Rename => self.transfer(TransferMode::Move, false),
            Command::MakeDir => self.make_dir(),
            Command::Delete { trash } => self.delete(trash),
            Command::Exit => {
                if let Some(window) = panel.window() {
                    window.close();
                }
            }
            Command::Refresh => panel.reload_or_leave(),
            Command::MarkMarkAll => panel.mark_all(true),
            Command::MarkUnmarkAll => panel.mark_all(false),
            Command::MarkInvert => panel.invert_marks(),
            Command::ChangeDirToRoot => panel.change_dir(PathBuf::from("/")),
            Command::ChangeDirToHome => panel.change_dir(glib::home_dir()),
            Command::BriefView => panel.set_view_mode(ViewMode::List),
            Command::ColumnsView => panel.set_view_mode(ViewMode::Details),
            Command::LeftEqualRight => self.panels[0].change_dir(self.panels[1].path()),
            Command::RightEqualLeft => self.panels[1].change_dir(self.panels[0].path()),
            Command::PanelsSplitterPerPos { percent } => {
                if let Some(splitter) = self.splitter.get() {
                    splitter.set_percent(percent);
                }
            }
        }
    }

    /// Key press anywhere in the window.
    ///
    /// Order matters: a running quick search takes typed characters and
    /// Backspace first (so a space continues the search instead of marking),
    /// then the keymap, and only then a character may start a new search.
    pub fn handle_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let panel = self.active();
        let typed = keys::typed_char(key, state);

        if panel.is_quick_searching() {
            if let Some(ch) = typed {
                panel.type_char(ch);
                return glib::Propagation::Stop;
            }
            if keys::shortcut(key, state) == Some(Shortcut::plain(Key::Backspace)) {
                panel.quick_search_backspace();
                return glib::Propagation::Stop;
            }
        }
        if !keys::is_modifier_key(key) {
            panel.end_quick_search();
        }

        if let Some(command) = keys::shortcut(key, state).and_then(|shortcut| self.keymap.command(&shortcut)) {
            self.execute(command);
            return glib::Propagation::Stop;
        }
        if let Some(ch) = typed
            && panel.type_char(ch)
        {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    /// F3 and F4: reports a failed start of an editor.
    fn edit(&self, path: &Path, result: Result<(), LaunchError>) {
        let Err(err) = result else { return };
        let detail = match err {
            LaunchError::NoEditor => gettext("No text editor found. Set one in the VISUAL or EDITOR variable."),
            LaunchError::NoTerminal => gettext("No terminal found. Install xdg-terminal-exec or set the TERMINAL variable."),
            LaunchError::NoDesktopEditor => gettext("No application is set for text files (text/plain)."),
            LaunchError::Spawn(err) => err.to_string(),
            LaunchError::Launch(err) => err.to_string(),
        };
        let message = fill(&gettext("Cannot open {path} in an editor"), &[("path", &path.display())]);
        dialogs::show_error(self.active().window().as_ref(), &message, &detail);
    }

    /// F7, `cm_MakeDir`.
    fn make_dir(self: &Rc<Self>) {
        let panel = Rc::clone(self.active());
        let (dir, suggestion) = {
            let state = panel.panel();
            (state.path().to_owned(), state.new_directory_suggestion())
        };
        let this = Rc::downgrade(self);
        dialogs::ask_new_directory(panel.window().as_ref(), &suggestion, move |name| {
            let Some(this) = this.upgrade() else { return };
            let name = name.trim();
            if !name.is_empty() {
                // A relative name may be a path (`a/b/c`), an absolute one replaces `dir`.
                match panel.source().create_directory(&dir.join(name)) {
                    Ok(()) => {
                        this.refresh_panels();
                        if let Some(first) = first_component(Path::new(name)) {
                            panel.reload_focusing(&first);
                        }
                    }
                    Err(err) => panel.show_error(&err),
                }
            }
            panel.focus();
        });
    }

    /// F8, Del, `cm_Delete`: marked items or the one under the cursor.
    fn delete(self: &Rc<Self>, trash: TrashChoice) {
        let panel = Rc::clone(self.active());
        let (dir, names): (PathBuf, Vec<OsString>) = {
            let state = panel.panel();
            (state.path().to_owned(), state.targets().iter().map(|item| item.entry().name.clone()).collect())
        };
        if names.is_empty() {
            return;
        }
        let mode = if trash.use_trash(self.settings.use_trash) { DeleteMode::Trash } else { DeleteMode::Permanently };
        let question = delete_question(&names, mode);

        let operations = Rc::clone(&self.operations);
        let (last_queue, new_queue) = (operations.last_queue(QueueMemory::Delete), operations.new_queue());
        dialogs::confirm_operation(panel.window().as_ref(), &question, last_queue, new_queue, move |queue| {
            let paths: Vec<PathBuf> = names.iter().map(|name| dir.join(name)).collect();
            operations.start(Box::new(DeleteOperation::new(panel.source(), paths, mode)), queue, QueueMemory::Delete);
        });
    }

    /// F5 `cm_Copy`, Shift+F5 `cm_CopySamePanel`, F6 `cm_Rename`: marked
    /// items or the one under the cursor, into the other panel's directory
    /// or (same panel) under another name next to them.
    fn transfer(self: &Rc<Self>, mode: TransferMode, same_panel: bool) {
        let panel = Rc::clone(self.active());
        let (dir, items): (PathBuf, Vec<(OsString, bool)>) = {
            let state = panel.panel();
            let items = state.targets().iter().map(|item| (item.entry().name.clone(), item.is_directory_like())).collect();
            (state.path().to_owned(), items)
        };
        if items.is_empty() {
            return;
        }
        let target = initial_target(&items, (!same_panel).then(|| self.panels[1 - self.active.get()].path()).as_deref());
        let names: Vec<OsString> = items.into_iter().map(|(name, _)| name).collect();

        let operations = Rc::clone(&self.operations);
        let request = copy_dialog::Request {
            mode,
            message: transfer_question(&names, mode),
            target,
            last_queue: operations.last_queue(QueueMemory::Transfer),
            new_queue: operations.new_queue(),
        };
        let source_panel = Rc::clone(&panel);
        copy_dialog::show(panel.window().as_ref(), &request, move |confirmed| {
            let Some(destination) = Destination::parse(&confirmed.target, &dir, names.len()) else { return };
            let paths: Vec<PathBuf> = names.iter().map(|name| dir.join(name)).collect();
            let operation = TransferOperation::new(mode, paths, destination, confirmed.options);
            operations.start(Box::new(operation), confirmed.queue, QueueMemory::Transfer);
            // As the original: marks go once the operation is on its way.
            source_panel.mark_all(false);
            source_panel.focus();
        });
    }

    /// Both panels may show what changed, e.g. the same directory, or the
    /// other panel may be inside a deleted directory.
    pub fn refresh_panels(&self) {
        for panel in &self.panels {
            panel.reload_or_leave();
        }
    }
}

/// Confirmation text of the original (`rsMsgDelSel`, `rsMsgDelFlDr` and
/// their trash variants): one name, or a count with up to five names.
fn delete_question(names: &[OsString], mode: DeleteMode) -> String {
    let trash = mode == DeleteMode::Trash;
    match names {
        [one] => {
            let template = if trash {
                gettext("Delete selected \"{name}\" into trash can?")
            } else {
                gettext("Delete selected \"{name}\"?")
            };
            fill(&template, &[("name", &one.to_string_lossy())])
        }
        many => {
            let count = plural_count(many.len());
            let template = if trash {
                ngettext(
                    "Delete {count} selected file/directory into trash can?",
                    "Delete {count} selected files/directories into trash can?",
                    count,
                )
            } else {
                ngettext("Delete {count} selected file/directory?", "Delete {count} selected files/directories?", count)
            };
            let mut text = fill(&template, &[("count", &many.len())]);
            text.push('\n');
            for name in many.iter().take(5) {
                text.push_str("\n\"");
                text.push_str(&name.to_string_lossy());
                text.push('"');
            }
            if many.len() > 5 {
                text.push_str("\n...");
            }
            text
        }
    }
}

/// Target prefilled in the copy and move dialog: the other panel's
/// directory with the name of a single file, or `*.*` for a directory and
/// several items. Without `other_dir` (copying in the same panel) only the
/// name, or `*.*`.
fn initial_target(items: &[(OsString, bool)], other_dir: Option<&Path>) -> String {
    let name = match items {
        [(name, false)] => name.to_string_lossy().into_owned(),
        [(name, true)] if other_dir.is_none() => name.to_string_lossy().into_owned(),
        _ => "*.*".to_owned(),
    };
    match other_dir {
        Some(dir) => {
            let dir = dir.to_string_lossy();
            let separator = if dir.ends_with('/') { "" } else { "/" };
            format!("{dir}{separator}{name}")
        }
        None => name,
    }
}

/// `rsMsgCpSel`, `rsMsgCpFlDr`, `rsMsgRenSel`, `rsMsgRenFlDr` of the original.
fn transfer_question(names: &[OsString], mode: TransferMode) -> String {
    let copying = mode == TransferMode::Copy;
    match names {
        [one] => {
            let template = if copying { gettext("Copy selected \"{name}\"?") } else { gettext("Rename/move selected \"{name}\"?") };
            fill(&template, &[("name", &one.to_string_lossy())])
        }
        many => {
            let count = plural_count(many.len());
            let template = if copying {
                ngettext("Copy {count} selected file/directory?", "Copy {count} selected files/directories?", count)
            } else {
                ngettext(
                    "Rename/move {count} selected file/directory?",
                    "Rename/move {count} selected files/directories?",
                    count,
                )
            };
            fill(&template, &[("count", &many.len())])
        }
    }
}

/// First directory of a relative path, the one that appears in the panel.
fn first_component(path: &Path) -> Option<OsString> {
    match path.components().next()? {
        Component::Normal(name) => Some(name.to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn delete_question_like_the_original() {
        assert_eq!(delete_question(&names(&["a.txt"]), DeleteMode::Trash), "Delete selected \"a.txt\" into trash can?");
        assert_eq!(delete_question(&names(&["a.txt"]), DeleteMode::Permanently), "Delete selected \"a.txt\"?");
        assert_eq!(
            delete_question(&names(&["a", "b"]), DeleteMode::Permanently),
            "Delete 2 selected files/directories?\n\n\"a\"\n\"b\""
        );
        let seven = delete_question(&names(&["1", "2", "3", "4", "5", "6", "7"]), DeleteMode::Trash);
        assert!(seven.starts_with("Delete 7 selected files/directories into trash can?\n"));
        assert!(seven.contains("\"5\"") && !seven.contains("\"6\"") && seven.ends_with("\n..."));
    }

    #[test]
    fn copy_target_like_the_original() {
        let file = [(OsString::from("a.txt"), false)];
        let dir = [(OsString::from("dir"), true)];
        let two = [(OsString::from("a"), false), (OsString::from("b"), true)];
        let other = Some(Path::new("/other"));
        assert_eq!(initial_target(&file, other), "/other/a.txt");
        assert_eq!(initial_target(&dir, other), "/other/*.*");
        assert_eq!(initial_target(&two, Some(Path::new("/"))), "/*.*");
        assert_eq!(initial_target(&file, None), "a.txt");
        assert_eq!(initial_target(&dir, None), "dir");
        assert_eq!(initial_target(&two, None), "*.*");
        assert_eq!(transfer_question(&names(&["a", "b"]), TransferMode::Move), "Rename/move 2 selected files/directories?");
    }

    #[test]
    fn focus_after_mkdir_is_the_first_new_directory() {
        assert_eq!(first_component(Path::new("a/b/c")), Some(OsString::from("a")));
        assert_eq!(first_component(Path::new("new")), Some(OsString::from("new")));
        assert_eq!(first_component(Path::new("/abs/path")), None);
    }
}
