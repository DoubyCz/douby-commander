//! The two panels, which one is active, and the single place where keys
//! become commands and commands get executed.

use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use dc_core::command::{Command, TrashChoice};
use dc_core::multi_rename::RenameFile;
use dc_core::keymap::{Key, Keymap, Shortcut};
use dc_core::mask::{MaskList, MaskOptions};
use dc_core::FileItem;
use dc_core::path_input;
use dc_core::startup::{self, Startup};
use dc_i18n::{fill, gettext, ngettext, plural_count};
use dc_ops::{DeleteMode, DeleteOperation, Destination, ExtractOperation, TransferMode, TransferOperation};
use dc_vfs::{FileSource, LocalFs};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::copy_dialog;
use crate::find_files::{self, FindWindow};
use crate::launch::{self, LaunchError};
use crate::operations::{OperationsUi, QueueMemory};
use crate::panel_view::{PanelView, ViewMode, not_in_archives};
use crate::quick_view::QuickView;
use crate::splitter::Splitter;
use crate::settings::Settings;
use crate::{about, dialogs, keys, menu, multi_rename};

pub struct Commander {
    panels: [Rc<PanelView>; 2],
    active: Cell<usize>,
    keymap: Keymap,
    settings: Settings,
    operations: Rc<OperationsUi>,
    /// Set by the window once the panels are laid out.
    splitter: OnceCell<Rc<Splitter>>,
    quick_view: Rc<QuickView>,
    /// The panel whose place the quick view takes while it is open.
    quick_view_side: Cell<Option<usize>>,
    /// Per panel a stack of its whole column and the quick view: opening
    /// the quick view only switches the page, the column stays as it was.
    covers: OnceCell<[gtk::Stack; 2]>,
    /// The quick view follows the cursor once the current event is over.
    quick_view_pending: Cell<bool>,
    mark_memory: RefCell<MarkMemory>,
    /// The window to find files, made at the first Alt+F7.
    find: OnceCell<Rc<FindWindow>>,
}

/// What the mask dialog of Num+ / Num- offers next time.
struct MarkMemory {
    mask: String,
    options: MaskOptions,
    /// Masks used before, newest first.
    history: Vec<String>,
}

impl MarkMemory {
    /// Masks kept in the history.
    const HISTORY: usize = 20;

    fn remember(&mut self, mask: &str, options: MaskOptions) {
        mask.clone_into(&mut self.mask);
        self.options = options;
        self.history.retain(|earlier| earlier != mask);
        self.history.insert(0, mask.to_owned());
        self.history.truncate(Self::HISTORY);
    }
}

impl Default for MarkMemory {
    /// `*` as in the original.
    fn default() -> Self {
        Self { mask: "*".to_owned(), options: MaskOptions::default(), history: Vec::new() }
    }
}

impl Commander {
    pub fn new(panels: [Rc<PanelView>; 2], keymap: Keymap, settings: Settings, operations: Rc<OperationsUi>) -> Self {
        Self {
            panels,
            active: Cell::new(0),
            keymap,
            settings,
            operations,
            splitter: OnceCell::new(),
            quick_view: QuickView::new(),
            quick_view_side: Cell::new(None),
            covers: OnceCell::new(),
            quick_view_pending: Cell::new(false),
            mark_memory: RefCell::default(),
            find: OnceCell::new(),
        }
    }

    /// Whether hidden files are listed now (Ctrl+.), in both panels.
    pub fn shows_hidden(&self) -> bool {
        self.panels[0].panel().shows_hidden()
    }

    /// `cm_ShowSysFiles`: hidden files shown or not, in both panels as in
    /// the original; the menu item follows.
    fn toggle_hidden(&self) {
        let show = !self.shows_hidden();
        for panel in &self.panels {
            panel.set_show_hidden(show);
        }
        if let Some(window) = self.active().window() {
            menu::show_hidden_checked(&window, show);
        }
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

    /// A panel the quick view stands for becomes active (Tab, a click on
    /// its drive bar): the quick view closes, the panel is back as it was.
    pub fn set_active(&self, index: usize) {
        if self.quick_view_side.get() == Some(index) {
            self.close_quick_view();
        }
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
        if self.refused_in_archive(command) {
            return;
        }
        match command {
            Command::Open => panel.open_current(),
            Command::ChangeDirToParent => panel.go_up(),
            Command::FocusSwap => {
                let other = 1 - self.active.get();
                self.set_active(other);
                self.panels[other].focus();
            }
            Command::ToggleMark => panel.toggle_selection(),
            Command::ToggleMarkMoveDown => panel.toggle_selection_and_move(true),
            Command::MarkMove(direction) => panel.mark_and_move(direction),
            Command::MarkRange(step) => panel.select_range(step),
            Command::QuickView { close } => {
                if self.quick_view_side.get().is_some() {
                    self.close_quick_view();
                } else if !close {
                    self.open_quick_view();
                }
            }
            Command::ShellExecute => {
                if let Some(path) = panel.current_path() {
                    panel.open_file(&path);
                }
            }
            Command::OpenWith => {
                if let Some(path) = panel.current_path() {
                    panel.open_file_with(&path);
                }
            }
            Command::Copy => self.transfer(TransferMode::Copy, false),
            Command::CopySamePanel => self.transfer(TransferMode::Copy, true),
            Command::Rename => self.transfer(TransferMode::Move, false),
            Command::RenameOnly => panel.start_rename(),
            Command::MultiRename => self.multi_rename(),
            Command::Search => self.search(),
            Command::MakeDir => self.make_dir(),
            Command::RunTerm => {
                if let Err(err) = launch::open_terminal(&panel.terminal_dir()) {
                    let detail = match err {
                        LaunchError::Spawn(err) => err.to_string(),
                        LaunchError::NoTerminal => {
                            gettext("No terminal found. Install xdg-terminal-exec or set the TERMINAL variable.")
                        }
                    };
                    dialogs::show_error(panel.window().as_ref(), &gettext("Cannot run the terminal"), &detail);
                }
            }
            Command::Delete { trash } => self.delete(trash),
            Command::Exit => {
                if let Some(window) = panel.window() {
                    window.close();
                }
            }
            Command::About => about::show(panel.window().as_ref()),
            Command::Refresh => panel.reload_or_leave(),
            Command::MarkMarkAll => panel.mark_all(true),
            Command::MarkUnmarkAll => panel.mark_all(false),
            Command::MarkInvert => panel.invert_marks(),
            Command::MarkPlus => self.mark_group(true),
            Command::MarkMinus => self.mark_group(false),
            Command::ChangeDirToRoot => panel.change_dir(PathBuf::from("/")),
            Command::ChangeDirToHome => panel.change_dir(glib::home_dir()),
            Command::EditPath => panel.start_path_edit(),
            Command::ShowSysFiles => self.toggle_hidden(),
            Command::BriefView => panel.set_view_mode(ViewMode::List),
            Command::ColumnsView => panel.set_view_mode(ViewMode::Details),
            Command::ThumbnailsView => panel.set_view_mode(ViewMode::Icons),
            Command::TerminalView => panel.show_terminal(),
            Command::CycleView => panel.cycle_view(),
            Command::SortBy(column) => panel.sort_by(column),
            Command::ReverseOrder => panel.reverse_order(),
            Command::LeftEqualRight => self.panels[0].change_dir(self.panels[1].path()),
            Command::RightEqualLeft => self.panels[1].change_dir(self.panels[0].path()),
            Command::TransferLeft => self.transfer_path(0),
            Command::TransferRight => self.transfer_path(1),
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
    /// Shift released: a new hold of it decides anew whether Shift+arrows
    /// mark or unmark.
    pub fn handle_key_released(&self, key: gdk::Key) {
        if matches!(key, gdk::Key::Shift_L | gdk::Key::Shift_R) {
            for panel in &self.panels {
                panel.end_held_mark();
            }
        }
    }

    pub fn handle_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        // A new press of Shift too, in case its release went to another window.
        self.handle_key_released(key);
        let panel = self.active();
        // Renaming in place or editing the path: the keys belong to the entry.
        if panel.is_renaming() || panel.is_editing_path() {
            return glib::Propagation::Proceed;
        }
        // In the terminal every key is the shell's (Tab, ...), except those
        // that show the files again (F9 too) and Shift+F9.
        if panel.is_terminal_shown() && panel.has_focus() {
            let command = keys::shortcut(key, state).and_then(|shortcut| self.keymap.command(&shortcut));
            return match command {
                Some(
                    command @ (Command::BriefView
                    | Command::ColumnsView
                    | Command::ThumbnailsView
                    | Command::CycleView
                    | Command::RunTerm),
                ) => {
                    self.execute(command);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            };
        }
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
            // Esc only closes an open quick view, otherwise it is left to
            // the widgets (a popover closes with it).
            if command == (Command::QuickView { close: true }) && self.quick_view_side.get().is_none() {
                return glib::Propagation::Proceed;
            }
            self.execute(command);
            return glib::Propagation::Stop;
        }
        if let Some(ch) = typed {
            if panel.type_char(ch) {
                return glib::Propagation::Stop;
            }
            // Outside a quick search `+`, `-` and `*` of any key (not only
            // the keypad) select or unselect a group and invert the marks;
            // inside one they are typed.
            let group = match ch {
                '+' => Some(Command::MarkPlus),
                '-' => Some(Command::MarkMinus),
                '*' => Some(Command::MarkInvert),
                _ => None,
            };
            if let Some(command) = group {
                self.execute(command);
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    }

    /// Commands that work on files on disk only: inside an archive (the
    /// active panel's, or the other one's as a target) they are refused
    /// with a message, archives can only be browsed so far.
    fn refused_in_archive(&self, command: Command) -> bool {
        let other = &self.panels[1 - self.active.get()];
        let refused = match command {
            Command::ShellExecute
            | Command::OpenWith
            | Command::CopySamePanel
            | Command::MakeDir
            | Command::RunTerm
            | Command::Delete { .. }
            | Command::Rename
            | Command::RenameOnly
            | Command::MultiRename => self.active().in_archive(),
            // Out of an archive it extracts; into one not yet.
            Command::Copy => other.in_archive(),
            _ => false,
        } || (command == Command::Rename && other.in_archive());
        if refused {
            dialogs::show_error(self.active().window().as_ref(), &not_in_archives(), "");
        }
        refused || self.refused_in_found(command)
    }

    /// Commands that need one directory: refused in a list of items found
    /// by a search (the active panel's, or the other one's as a target).
    fn refused_in_found(&self, command: Command) -> bool {
        let other = &self.panels[1 - self.active.get()];
        let refused = match command {
            Command::MakeDir | Command::RenameOnly | Command::CopySamePanel => self.active().is_found(),
            Command::Copy | Command::Rename => other.is_found(),
            _ => false,
        };
        if refused {
            let message = gettext("This does not work in a list of items found by a search.");
            dialogs::show_error(self.active().window().as_ref(), &message, "");
        }
        refused
    }

    /// Items found inside archives cannot be copied, moved or deleted from
    /// a list of items found (only browsed in the archive itself).
    fn refused_found_in_archive(&self, paths: &[PathBuf]) -> bool {
        let refused = self.active().is_found()
            && paths.iter().any(|path| path.parent().is_some_and(|dir| self.active().is_archive_path(dir)));
        if refused {
            dialogs::show_error(self.active().window().as_ref(), &not_in_archives(), "");
        }
        refused
    }

    /// The window gives the stacks of both columns ([`Commander::covers`]).
    pub fn set_covers(self: &Rc<Self>, covers: [gtk::Stack; 2]) {
        // Only one window; a second call changes nothing.
        let _ = self.covers.set(covers);
        let weak = Rc::downgrade(self);
        self.quick_view.connect_open(move |path| {
            if let Some(this) = weak.upgrade() {
                this.close_quick_view();
                this.active().open_file(&path);
            }
        });
        // The keyboard stays with the active panel, also after a click on
        // the player or the button.
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_enter(move |_| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(this) = weak.upgrade() {
                    this.active().focus();
                }
            });
        });
        self.quick_view.widget().add_controller(focus);
        for (side, panel) in self.panels.iter().enumerate() {
            let weak = Rc::downgrade(self);
            panel.connect_cursor_changed(move || {
                if let Some(this) = weak.upgrade()
                    && this.quick_view_side.get().is_some()
                    && this.active.get() == side
                {
                    this.follow_cursor();
                }
            });
        }
    }

    /// F3: the quick view in the other panel's place.
    fn open_quick_view(self: &Rc<Self>) {
        let Some(covers) = self.covers.get() else { return };
        let side = 1 - self.active.get();
        let cover = &covers[side];
        cover.add_named(self.quick_view.widget(), Some("quick-view"));
        cover.set_visible_child(self.quick_view.widget());
        self.quick_view_side.set(Some(side));
        self.quick_view.show(self.active().quick_view_subject());
    }

    /// Esc, F3 again, Tab: the other panel is back, untouched.
    fn close_quick_view(&self) {
        let (Some(side), Some(covers)) = (self.quick_view_side.take(), self.covers.get()) else { return };
        let cover = &covers[side];
        cover.set_visible_child_name("files");
        cover.remove(self.quick_view.widget());
        self.quick_view.hide();
    }

    /// The cursor of the active panel moved: the quick view shows the new
    /// item once the current event is over (one listing notifies a lot).
    fn follow_cursor(self: &Rc<Self>) {
        if self.quick_view_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(this) = weak.upgrade() else { return };
            this.quick_view_pending.set(false);
            if this.quick_view_side.get().is_some() {
                this.quick_view.show(this.active().quick_view_subject());
            }
        });
    }

    /// Alt+F7, `cm_Search`: the window to find files, starting in the
    /// active panel's directory (in an archive: the archive's), its marked
    /// items offered as "selected directories and files".
    fn search(self: &Rc<Self>) {
        let panel = self.active();
        let (dir, selected) = if let Some(archive) = panel.archive_file() {
            (archive.parent().map_or_else(|| PathBuf::from("/"), Path::to_owned), Vec::new())
        } else {
            let state = panel.panel();
            let marked = state.items().iter().filter(|item| item.is_selected()).map(|item| state.item_path(item)).collect();
            (state.path().to_owned(), marked)
        };
        let window = self.find.get_or_init(|| {
            let (go_to, open, feed) = (Rc::downgrade(self), Rc::downgrade(self), Rc::downgrade(self));
            let actions = find_files::Actions {
                go_to: Box::new(move |path| {
                    if let Some(this) = go_to.upgrade() {
                        this.go_to_found(path);
                    }
                }),
                open: Box::new(move |path| {
                    if let Some(this) = open.upgrade() {
                        this.active().open_file(path);
                    }
                }),
                feed: Box::new(move |paths, base| {
                    if let Some(this) = feed.upgrade() {
                        let panel = this.active();
                        panel.show_found(paths, base);
                        panel.focus();
                        if let Some(window) = panel.window() {
                            window.present();
                        }
                    }
                }),
            };
            FindWindow::new(panel.window().as_ref(), actions)
        });
        window.show(dir, selected);
    }

    /// "Go to file" of the search: its directory in the active panel, the
    /// cursor on it (inside an archive too), the main window up.
    fn go_to_found(&self, path: &Path) {
        let panel = self.active();
        let shown = match (path.parent(), path.file_name()) {
            (Some(dir), Some(name)) => panel.change_dir_focusing(dir.to_owned(), Some(name)),
            _ => panel.change_dir_focusing(path.to_owned(), None),
        };
        if shown {
            panel.focus();
        }
        if let Some(window) = panel.window() {
            window.present();
        }
    }

    /// Ctrl+M, `cm_MultiRename`: marked items or the one under the cursor.
    fn multi_rename(self: &Rc<Self>) {
        let panel = self.active();
        let files: Vec<RenameFile> = {
            let state = panel.panel();
            let mut files = Vec::new();
            for item in state.targets() {
                let entry = item.entry();
                // Masks work on text; a name that is not UTF-8 would not survive them.
                let Some(name) = entry.name.to_str() else {
                    let message = fill(&gettext("Invalid file name: {name}"), &[("name", &entry.name.to_string_lossy())]);
                    dialogs::show_error(panel.window().as_ref(), &message, "");
                    return;
                };
                files.push(RenameFile {
                    dir: item.dir().unwrap_or(state.path()).to_owned(),
                    name: name.to_owned(),
                    is_dir: item.is_directory_like(),
                    modified: entry.modified.and_then(|time| jiff::Timestamp::try_from(time).ok()),
                });
            }
            files
        };
        let this = Rc::downgrade(self);
        multi_rename::show(panel.window().as_ref(), files, move || {
            if let Some(this) = this.upgrade() {
                this.refresh_panels();
            }
        });
    }

    /// Alt+Left / Alt+Right, `cm_TransferLeft` / `cm_TransferRight`: the
    /// panel of `target` shows the directory under the cursor of the other,
    /// active one; `..` its parent with the cursor on the directory left, on
    /// a file the active panel's directory with the cursor on the file. Towards the active panel itself nothing.
    fn transfer_path(&self, target: usize) {
        if target == self.active.get() {
            return;
        }
        let (path, focus) = {
            let state = self.active().panel();
            let dir = state.path().to_owned();
            match state.current_item() {
                // Out of the items found: the directory they are shown under.
                Some(item) if item.is_parent() && state.is_found() => (dir, None),
                // As if the target panel went up itself: the cursor on the
                // directory we come from.
                Some(item) if item.is_parent() => match dir.parent() {
                    Some(parent) => (parent.to_owned(), dir.file_name().map(OsStr::to_owned)),
                    None => (dir, None),
                },
                Some(item) if item.is_directory_like() => (state.item_path(item), None),
                Some(item) => (item.dir().map_or(dir, Path::to_owned), Some(item.entry().name.clone())),
                None => (dir, None),
            }
        };
        self.panels[target].change_dir_focusing(path, focus.as_deref());
    }

    /// Num+ / Num- (`+` / `-`), `cm_MarkPlus` / `cm_MarkMinus`: asks for a
    /// mask and marks or unmarks what matches it. The mask, the options and
    /// the history are shared by both panels.
    fn mark_group(self: &Rc<Self>, select: bool) {
        let panel = Rc::clone(self.active());
        if panel.panel().items().iter().all(FileItem::is_parent) {
            return;
        }
        let (mask, history, options) = {
            let memory = self.mark_memory.borrow();
            (memory.mask.clone(), memory.history.clone(), memory.options)
        };
        let this = Rc::downgrade(self);
        dialogs::ask_mask(panel.window().as_ref(), select, &mask, &history, options, move |mask, options| {
            let Some(this) = this.upgrade() else { return };
            this.mark_memory.borrow_mut().remember(&mask, options);
            panel.mark_group(&MaskList::new(&mask, options), select);
            panel.focus();
        });
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
                match LocalFs.create_directory(&dir.join(name)) {
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

    /// Enter in the path entry of the panel at `side`: goes where `text`
    /// leads. An existing file is shown in its directory with the cursor
    /// on it, a path that does not exist can be created after a question
    /// that shows which directories it takes; `by_button` (the button
    /// "Create (N) and go" clicked, which says it already) creates them
    /// without asking. On an error or a refused question the entry stays
    /// open to correct the text.
    pub fn enter_path(self: &Rc<Self>, side: usize, text: &str, by_button: bool) {
        let panel = Rc::clone(&self.panels[side]);
        let Some(target) = panel.typed_path(text) else {
            panel.end_path_edit(true);
            return;
        };
        let went = if panel.is_archive_path(&target) {
            show_archive_path(&panel, target)
        } else {
            match path_input::destination(&target) {
                path_input::Destination::Directory => panel.change_dir_focusing(target, None),
                path_input::Destination::File { dir, name } => panel.change_dir_focusing(dir, Some(&name)),
                path_input::Destination::Blocked(path) => {
                    let message = fill(&gettext("{path} is not a directory."), &[("path", &path.display())]);
                    dialogs::show_error(panel.window().as_ref(), &message, "");
                    false
                }
                path_input::Destination::Missing { allowed: true, .. } if by_button => self.create_and_go(&panel, target),
                path_input::Destination::Missing { existing, missing, allowed } => {
                    let this = Rc::downgrade(self);
                    let (window, dir) = (panel.window(), target.clone());
                    let request = dialogs::NewDirectories { target: &target, existing: &existing, missing: &missing, allowed };
                    dialogs::confirm_create_directories(window.as_ref(), &request, move || {
                        if let Some(this) = this.upgrade()
                            && this.create_and_go(&panel, dir)
                        {
                            panel.end_path_edit(true);
                        }
                    });
                    false
                }
            }
        };
        if went {
            self.panels[side].end_path_edit(true);
        }
    }

    /// The command line (at start, or given to the running application):
    /// the panel `-P` names becomes active, `-L` and `-R` and a path alone
    /// (for the active panel) are shown.
    pub fn apply_startup(&self, startup: &Startup<PathBuf>) {
        if let Some(side) = startup.active {
            self.set_active(side.index());
        }
        for (side, path) in [(0, &startup.left), (1, &startup.right)] {
            if let Some(path) = path {
                self.show_at_start(side, path);
            }
        }
        if let Some(path) = &startup.active_path {
            self.show_at_start(self.active.get(), path);
        }
        self.active().focus();
    }

    /// `path` of the command line in the panel at `side`: a file in its
    /// directory with the cursor on it, a path that does not exist as far
    /// as it does.
    fn show_at_start(&self, side: usize, path: &Path) {
        let panel = &self.panels[side];
        panel.end_path_edit(false);
        if panel.is_archive_path(path) {
            show_archive_path(panel, path.to_owned());
        } else {
            let (dir, focus) = startup::place(path);
            panel.change_dir_focusing(dir, focus.as_deref());
        }
    }

    /// Creates the directory `dir` with the missing ones on the way, then
    /// `panel` shows it. `false` on an error (shown).
    fn create_and_go(&self, panel: &PanelView, dir: PathBuf) -> bool {
        match LocalFs.create_directory(&dir) {
            Ok(()) => {
                // The other panel may show where they were made.
                self.refresh_panels();
                panel.change_dir_focusing(dir, None)
            }
            Err(err) => {
                panel.show_error(&err);
                false
            }
        }
    }

    /// F8, Del, `cm_Delete`: marked items or the one under the cursor.
    fn delete(self: &Rc<Self>, trash: TrashChoice) {
        let panel = Rc::clone(self.active());
        let (names, paths): (Vec<OsString>, Vec<PathBuf>) = {
            let state = panel.panel();
            state.targets().iter().map(|item| (item.entry().name.clone(), state.item_path(item))).unzip()
        };
        if names.is_empty() || self.refused_found_in_archive(&paths) {
            return;
        }
        let mode = if trash.use_trash(self.settings.use_trash) { DeleteMode::Trash } else { DeleteMode::Permanently };
        let question = delete_question(&names, mode);

        let operations = Rc::clone(&self.operations);
        let (last_queue, new_queue) = (operations.last_queue(QueueMemory::Delete), operations.new_queue());
        dialogs::confirm_operation(panel.window().as_ref(), &question, last_queue, new_queue, move |queue| {
            operations.start(Box::new(DeleteOperation::new(LocalFs, paths, mode)), queue, QueueMemory::Delete);
        });
    }

    /// F5 `cm_Copy`, Shift+F5 `cm_CopySamePanel`, F6 `cm_Rename`: marked
    /// items or the one under the cursor, into the other panel's directory
    /// or (same panel) under another name next to them.
    fn transfer(self: &Rc<Self>, mode: TransferMode, same_panel: bool) {
        let panel = Rc::clone(self.active());
        let (dir, items, paths): (PathBuf, Vec<(OsString, bool)>, Vec<PathBuf>) = {
            let state = panel.panel();
            let targets = state.targets();
            let items = targets.iter().map(|item| (item.entry().name.clone(), item.is_directory_like())).collect();
            let paths = targets.iter().map(|item| state.item_path(item)).collect();
            (state.path().to_owned(), items, paths)
        };
        if items.is_empty() || self.refused_found_in_archive(&paths) {
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
        // Out of an archive: extracting, a relative target is relative to
        // the archive's directory (as in the original).
        let archive = panel.archive_file();
        let extract_bytes: u64 = if archive.is_some() { paths.iter().map(|path| panel.archive_bytes(path)).sum() } else { 0 };
        let source_panel = Rc::clone(&panel);
        copy_dialog::show(panel.window().as_ref(), &request, move |confirmed| {
            let base = archive.as_ref().and_then(|file| file.parent()).unwrap_or(&dir);
            let Some(destination) = Destination::parse(&confirmed.target, base, names.len()) else { return };
            let operation: Box<dyn dc_ops::Operation> = match &archive {
                Some(file) => Box::new(ExtractOperation::new(
                    file.clone(),
                    dir.clone(),
                    paths.clone(),
                    destination,
                    confirmed.options,
                    extract_bytes,
                )),
                None => Box::new(TransferOperation::new(mode, paths.clone(), destination, confirmed.options)),
            };
            operations.start(operation, confirmed.queue, QueueMemory::Transfer);
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
/// A path in an archive, the archive itself knows what is there; the
/// archive file is a file to stand on, Enter goes in. `false` when it
/// cannot be listed (the error is shown).
fn show_archive_path(panel: &PanelView, target: PathBuf) -> bool {
    match (target.parent(), target.file_name()) {
        (Some(dir), Some(name)) if !panel.is_archive_path(dir) => panel.change_dir_focusing(dir.to_owned(), Some(name)),
        _ => panel.change_dir_focusing(target, None),
    }
}

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
