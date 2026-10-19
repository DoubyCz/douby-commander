//! One file panel: path header, file list and status line.
//!
//! The list only renders [`Panel`] state. Sorting, cursor movement on
//! navigation and selection all live in `dc-core`, column headers just
//! show the sort indicator and report clicks.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use dc_core::command::RangeStep;
use dc_core::quick_search::{Pattern, TYPING_TIMEOUT, TypeAhead};
use dc_core::{Panel, SortColumn, SortDirection, Sorting, format};
use dc_i18n::{N_, fill, gettext};
use dc_vfs::LocalFs;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use jiff::tz::TimeZone;

use crate::dialogs;
use crate::file_row::FileRow;

/// Sort key, column id, title (translated when the columns are built).
const COLUMNS: [(SortColumn, &str, &str); 5] = [
    (SortColumn::Name, "name", N_("Name")),
    (SortColumn::Extension, "ext", N_("Ext")),
    (SortColumn::Size, "size", N_("Size")),
    (SortColumn::Modified, "date", N_("Date")),
    (SortColumn::Attributes, "attr", N_("Attr")),
];

pub struct PanelView {
    panel: RefCell<Panel>,
    source: LocalFs,
    tz: TimeZone,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    column_view: gtk::ColumnView,
    path_label: gtk::Label,
    status_label: gtk::Label,
    root: gtk::Box,
    /// Set while the view itself moves the cursor, so the selection
    /// callback does not write the same position back.
    syncing: Cell<bool>,
    type_ahead: RefCell<TypeAhead>,
    path_listeners: RefCell<Vec<PathListener>>,
    /// Scroll position of directories left for a subdirectory, restored
    /// when coming back up, so the list looks as it did before.
    scroll_memory: RefCell<HashMap<PathBuf, f64>>,
    this: Weak<Self>,
}

type PathListener = Box<dyn Fn(&Path)>;

impl PanelView {
    pub fn new(path: PathBuf) -> Rc<Self> {
        let store = gio::ListStore::new::<FileRow>();
        let selection = gtk::SingleSelection::builder().model(&store).autoselect(true).build();
        let column_view = gtk::ColumnView::builder().model(&selection).reorderable(false).build();
        column_view.add_css_class("data-table");

        let path_label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Start).build();
        path_label.add_css_class("dc-path");
        let status_label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        status_label.add_css_class("dc-status");

        let scrolled = gtk::ScrolledWindow::builder().child(&column_view).vexpand(true).build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("dc-panel");
        root.append(&path_label);
        root.append(&scrolled);
        root.append(&status_label);

        let view = Rc::new_cyclic(|this| Self {
            panel: RefCell::new(Panel::new(path.clone())),
            source: LocalFs,
            tz: TimeZone::system(),
            store,
            selection,
            column_view,
            path_label,
            status_label,
            root,
            syncing: Cell::new(false),
            type_ahead: RefCell::new(TypeAhead::default()),
            path_listeners: RefCell::default(),
            scroll_memory: RefCell::default(),
            this: this.clone(),
        });
        view.setup_columns();
        view.connect_signals();

        let loaded = view.panel.borrow_mut().load(&view.source, path, None);
        if let Err(err) = loaded {
            // Nothing to show yet, the panel stays empty with the error in the status line.
            view.status_label.set_text(&err.to_string());
        }
        view.refresh();
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_active(&self, active: bool) {
        if active {
            self.root.add_css_class("dc-active");
        } else {
            self.root.remove_css_class("dc-active");
        }
    }

    /// Moves keyboard focus to the row under the cursor.
    pub fn focus(&self) {
        let cursor = self.panel.borrow().cursor();
        // FOCUS only picks the focused row inside the list, the widget itself
        // has to take keyboard focus too.
        self.column_view.scroll_to(position(cursor), None, gtk::ListScrollFlags::FOCUS, None);
        self.column_view.grab_focus();
    }

    pub fn connect_focus_enter(&self, callback: impl Fn() + 'static) {
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| callback());
        self.column_view.add_controller(focus);
    }

    /// Insert, Shift+Down (`forward`) and Shift+Up.
    pub fn toggle_selection_and_move(&self, forward: bool) {
        self.panel.borrow_mut().toggle_selection_and_move(forward);
        self.after_marking();
    }

    pub fn toggle_selection(&self) {
        self.panel.borrow_mut().toggle_selection_at_cursor();
        self.after_marking();
    }

    /// Shift+PgUp/PgDn/Home/End by default.
    pub fn select_range(&self, step: RangeStep) {
        let page = self.rows_per_page();
        let mut panel = self.panel.borrow_mut();
        let (cursor, last) = (panel.cursor(), panel.items().len().saturating_sub(1));
        match step {
            RangeStep::PageUp => panel.select_range_to(cursor.saturating_sub(page), false),
            RangeStep::PageDown => panel.select_range_to(cursor + page, false),
            RangeStep::First => panel.select_range_to(0, true),
            RangeStep::Last => panel.select_range_to(last, true),
        }
        drop(panel);
        self.after_marking();
    }

    fn clicked_with_modifiers(&self, index: usize, modifiers: gdk::ModifierType) {
        let mut panel = self.panel.borrow_mut();
        if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            panel.select_range_to(index, true);
        } else {
            panel.toggle_clicked(index);
        }
        drop(panel);
        self.after_marking();
        self.focus();
    }

    fn after_marking(&self) {
        self.sync_marks();
        self.update_status();
        self.show_cursor();
    }

    /// A printable character typed into the panel. Returns `false` when the
    /// character is not taken by quick search and keeps its own meaning:
    /// space, `+`, `-` and `*` do not start a search, as in the original.
    pub fn type_char(&self, ch: char) -> bool {
        let now = Instant::now();
        let mut type_ahead = self.type_ahead.borrow_mut();
        if !type_ahead.is_active(now) && matches!(ch, ' ' | '+' | '-' | '*') {
            return false;
        }
        let text = type_ahead.push(ch, now).to_owned();
        drop(type_ahead);
        self.run_quick_search(&text);
        true
    }

    /// Backspace: shortens a running quick search. Returns `false` when no
    /// search is running.
    pub fn quick_search_backspace(&self) -> bool {
        let text = self.type_ahead.borrow_mut().pop(Instant::now()).map(str::to_owned);
        match text {
            Some(text) => {
                self.run_quick_search(&text);
                true
            }
            None => false,
        }
    }

    /// Whether characters typed now continue a running quick search.
    pub fn is_quick_searching(&self) -> bool {
        self.type_ahead.borrow().is_active(Instant::now())
    }

    /// Any other key ends the quick search, the next letter starts a new one.
    pub fn end_quick_search(&self) {
        let was_active = self.type_ahead.borrow().is_active(Instant::now());
        self.type_ahead.borrow_mut().reset();
        if was_active {
            self.update_status();
        }
    }

    fn run_quick_search(&self, text: &str) {
        if !text.is_empty() && self.panel.borrow_mut().quick_search(&Pattern::new(text)) {
            self.show_cursor();
        }
        self.update_status();

        // Put the normal status line back once the typing pause expires.
        let this = self.this.clone();
        glib::timeout_add_local_once(TYPING_TIMEOUT + Duration::from_millis(50), move || {
            if let Some(view) = this.upgrade() {
                view.update_status();
            }
        });
    }

    pub fn go_up(&self) {
        let left = self.leaving();
        let result = self.panel.borrow_mut().go_up(&self.source);
        match result {
            Ok(true) => self.arrived(left),
            Ok(false) => {}
            Err(err) => self.show_error(&err),
        }
    }

    /// Shows `path`, e.g. from a drive button or the `/` and `~` buttons.
    pub fn change_dir(&self, path: PathBuf) {
        let left = self.leaving();
        let result = self.panel.borrow_mut().load(&self.source, path, None);
        match result {
            Ok(()) => self.arrived(left),
            Err(err) => self.show_error(&err),
        }
    }

    /// The directory being left and how far its list is scrolled.
    fn leaving(&self) -> (PathBuf, f64) {
        let scrolled = self.column_view.vadjustment().map_or(0.0, |adjustment| adjustment.value());
        (self.path(), scrolled)
    }

    /// After moving to another directory: going down remembers how the
    /// directory left was scrolled, going up shows it scrolled the same
    /// way again, with the cursor on the directory we came from at the
    /// same place on screen.
    fn arrived(&self, (left, scrolled): (PathBuf, f64)) {
        self.refresh();
        let now = self.path();
        if now.parent() == Some(left.as_path()) {
            self.scroll_memory.borrow_mut().insert(left, scrolled);
        } else if left.parent() == Some(now.as_path())
            && let Some(&remembered) = self.scroll_memory.borrow().get(&now)
        {
            // After the list is laid out with the new rows.
            let this = self.this.clone();
            glib::idle_add_local_once(move || {
                if let Some(this) = this.upgrade() {
                    this.restore_scroll(remembered);
                }
            });
        }
    }

    /// Scrolls to `value` unless the cursor would not be fully visible
    /// there (the directory changed meanwhile); then it stays as it is.
    fn restore_scroll(&self, value: f64) {
        let rows = self.store.n_items();
        let Some(adjustment) = self.column_view.vadjustment() else { return };
        if rows == 0 || adjustment.upper() <= 0.0 {
            return;
        }
        // The header scrolls with the rows, it counts as one more row.
        let row_height = adjustment.upper() / f64::from(rows + 1);
        let cursor_top = f64::from(position(self.panel.borrow().cursor()) + 1) * row_height;
        let value = value.clamp(0.0, (adjustment.upper() - adjustment.page_size()).max(0.0));
        if cursor_top >= value + row_height && cursor_top + row_height <= value + adjustment.page_size() {
            adjustment.set_value(value);
        }
    }

    pub fn path(&self) -> PathBuf {
        self.panel.borrow().path().to_owned()
    }

    /// `cm_MarkMarkAll` (true) / `cm_MarkUnmarkAll` (false).
    pub fn mark_all(&self, selected: bool) {
        self.panel.borrow_mut().mark_all(selected);
        self.after_marking();
    }

    pub fn invert_marks(&self) {
        self.panel.borrow_mut().invert_marks();
        self.after_marking();
    }

    /// Enter or double click: opens the directory or file under the cursor.
    pub fn open_current(&self) {
        let left = self.leaving();
        let result = self.panel.borrow_mut().activate(&self.source);
        match result {
            Ok(Some(dc_core::Activation::Navigated)) => self.arrived(left),
            Ok(Some(dc_core::Activation::OpenFile(path))) => self.open_file(&path),
            Ok(None) => {}
            Err(err) => self.show_error(&err),
        }
    }

    fn setup_columns(&self) {
        for (sort_column, id, title) in COLUMNS {
            let factory = gtk::SignalListItemFactory::new();
            let numeric = matches!(sort_column, SortColumn::Size);
            let this = self.this.clone();
            factory.connect_setup(move |_, object| {
                let list_item = list_item(object);
                let label = gtk::Label::builder().xalign(if numeric { 1.0 } else { 0.0 }).build();
                if matches!(sort_column, SortColumn::Name) {
                    // Without a minimum the name gives way to the fixed columns in a narrow window.
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    label.set_width_chars(18);
                }

                // Ctrl+click and Shift+click mark files. The cell handles them
                // before the row does, which would otherwise just move the cursor.
                let click = gtk::GestureClick::new();
                let (this, item) = (this.clone(), list_item.downgrade());
                click.connect_pressed(move |gesture, _, _, _| {
                    let modifiers = gesture.current_event_state();
                    if !modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK) {
                        return;
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    if let (Some(view), Some(item)) = (this.upgrade(), item.upgrade())
                        && let Ok(index) = usize::try_from(item.position())
                    {
                        view.clicked_with_modifiers(index, modifiers);
                    }
                });
                label.add_controller(click);

                // Follows `marked` of whatever row the cell currently shows,
                // a changed mark restyles the cell without rebinding it.
                list_item
                    .property_expression("item")
                    .chain_property::<FileRow>("marked")
                    .chain_closure::<Vec<String>>(glib::closure!(|_: Option<glib::Object>, marked: bool| {
                        if marked { vec!["dc-marked".to_owned()] } else { Vec::new() }
                    }))
                    .bind(&label, "css-classes", Some(list_item));

                list_item.set_child(Some(&label));
            });
            factory.connect_bind(move |_, object| {
                let list_item = list_item(object);
                let row = list_item.item().and_downcast::<FileRow>().expect("rows are FileRow");
                let label = list_item.child().and_downcast::<gtk::Label>().expect("child is set up as Label");
                let texts = row.texts();
                label.set_text(match sort_column {
                    SortColumn::Name => &texts.name,
                    SortColumn::Extension => &texts.extension,
                    SortColumn::Size => &texts.size,
                    SortColumn::Modified => &texts.modified,
                    SortColumn::Attributes => &texts.attributes,
                });
                // Directory names in bold. Pango attributes, not a CSS class:
                // the classes of the label follow the mark (see setup).
                let bold = sort_column == SortColumn::Name && texts.directory;
                label.set_attributes(bold.then(bold_attributes).as_ref());
            });

            // The sorter only makes the header clickable and shows the arrow,
            // the model itself is sorted by dc-core.
            let column = gtk::ColumnViewColumn::builder()
                .id(id)
                .title(gettext(title))
                .factory(&factory)
                .sorter(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal))
                .resizable(true)
                .expand(matches!(sort_column, SortColumn::Name))
                .build();
            self.column_view.append_column(&column);
        }

        let first = self.column_view.columns().item(0).and_downcast::<gtk::ColumnViewColumn>();
        self.column_view.sort_by_column(first.as_ref(), gtk::SortType::Ascending);
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.column_view.connect_activate(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.open_current();
            }
        });

        let weak = Rc::downgrade(self);
        self.selection.connect_selected_notify(move |selection| {
            if let Some(view) = weak.upgrade()
                && !view.syncing.get()
                && let Ok(index) = usize::try_from(selection.selected())
            {
                view.panel.borrow_mut().set_cursor(index);
            }
        });

        if let Some(sorter) = self.column_view.sorter().and_downcast::<gtk::ColumnViewSorter>() {
            let weak = Rc::downgrade(self);
            sorter.connect_changed(move |sorter, _| {
                if let Some(view) = weak.upgrade()
                    && let Some(sorting) = sorting_from_header(sorter)
                {
                    view.panel.borrow_mut().set_sorting(sorting);
                    view.refresh();
                }
            });
        }
    }

    /// Rebuilds all rows from the panel state.
    fn refresh(&self) {
        let panel = self.panel.borrow();
        let rows: Vec<FileRow> = panel.items().iter().map(|item| FileRow::new(item, &self.tz)).collect();

        self.syncing.set(true);
        self.store.splice(0, self.store.n_items(), &rows);
        self.syncing.set(false);

        self.path_label.set_text(&panel.path().display().to_string());
        let path = panel.path().to_owned();
        drop(panel);
        self.update_status();
        self.show_cursor();
        for callback in self.path_listeners.borrow().iter() {
            callback(&path);
        }
    }

    /// `callback` runs after every listing (new directory or reload), with
    /// the shown path; also right away with the current one.
    pub fn connect_path_changed(&self, callback: impl Fn(&Path) + 'static) {
        callback(self.panel.borrow().path());
        self.path_listeners.borrow_mut().push(Box::new(callback));
    }

    /// Copies marks from the panel to the rows. Only changed rows get the
    /// property set, bound cells restyle on its notification.
    fn sync_marks(&self) {
        let panel = self.panel.borrow();
        for (index, item) in panel.items().iter().enumerate() {
            if let Some(row) = self.store.item(position(index)).and_downcast::<FileRow>()
                && row.marked() != item.is_selected()
            {
                row.set_marked(item.is_selected());
            }
        }
    }

    /// How far PgUp/PgDn jump: the number of fully visible rows.
    fn rows_per_page(&self) -> usize {
        let rows = self.store.n_items();
        let Some(adjustment) = self.column_view.vadjustment() else { return 1 };
        if rows == 0 || adjustment.upper() <= 0.0 {
            return 1;
        }
        // The header scrolls with the rows, count it as one more row.
        let row_height = adjustment.upper() / f64::from(rows + 1);
        let visible = (adjustment.page_size() / row_height).floor() - 1.0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a small positive row count, at least 1, truncation intended"
        )]
        let visible = visible.max(1.0) as usize;
        visible
    }

    fn update_status(&self) {
        let type_ahead = self.type_ahead.borrow();
        if type_ahead.is_active(Instant::now()) {
            // rsSearchStatus of the original.
            self.status_label.set_text(&fill(&gettext("(SEARCH: {text})"), &[("text", &type_ahead.text())]));
        } else {
            self.status_label.set_text(&format::selection_info(&self.panel.borrow().selection_info()));
        }
    }

    /// Puts the list selection on the panel cursor and scrolls to it. Keyboard
    /// focus moves along only when the panel already has it.
    fn show_cursor(&self) {
        let cursor = position(self.panel.borrow().cursor());
        self.syncing.set(true);
        self.selection.set_selected(cursor);
        self.syncing.set(false);

        let flags = if self.column_view.has_focus() || self.column_view.focus_child().is_some() {
            gtk::ListScrollFlags::FOCUS
        } else {
            gtk::ListScrollFlags::NONE
        };
        self.column_view.scroll_to(cursor, None, flags, None);
    }

    /// The file under the cursor, for F3 and F4; not `..` or a directory.
    pub fn current_file(&self) -> Option<PathBuf> {
        let panel = self.panel.borrow();
        let item = panel.current_item().filter(|item| !item.is_parent() && !item.is_directory_like())?;
        Some(panel.path().join(&item.entry().name))
    }

    /// In the application for its type, as the desktop decides.
    pub fn open_file(&self, path: &Path) {
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        let window = self.window();
        let parent = window.clone();
        let message = fill(&gettext("Cannot open {path}"), &[("path", &path.display())]);
        launcher.launch(window.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let Err(err) = result {
                dialogs::show_error(parent.as_ref(), &message, &err.to_string());
            }
        });
    }

    pub fn show_error(&self, err: &dc_vfs::Error) {
        dialogs::show_error(self.window().as_ref(), &err.to_string(), &err.reason());
    }

    /// The window the panel is in, parent for its dialogs.
    pub fn window(&self) -> Option<gtk::Window> {
        self.root.root().and_downcast::<gtk::Window>()
    }

    /// Read access to the panel state, e.g. for the items a command works on.
    pub fn panel(&self) -> Ref<'_, Panel> {
        self.panel.borrow()
    }

    pub fn source(&self) -> LocalFs {
        self.source
    }

    /// After a change on disk: lists the directory again, or leaves it for
    /// the nearest existing parent when it is gone.
    pub fn reload_or_leave(&self) {
        let result = self.panel.borrow_mut().reload_or_leave(&self.source);
        match result {
            Ok(()) => self.refresh(),
            Err(err) => self.show_error(&err),
        }
    }

    /// Lists the directory again with the cursor on `name`.
    pub fn reload_focusing(&self, name: &OsStr) {
        let result = self.panel.borrow_mut().reload_focusing(&self.source, Some(name));
        match result {
            Ok(()) => self.refresh(),
            Err(err) => self.show_error(&err),
        }
    }
}

fn bold_attributes() -> gtk::pango::AttrList {
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
    attributes
}

fn list_item(object: &glib::Object) -> &gtk::ListItem {
    object.downcast_ref::<gtk::ListItem>().expect("column factories get ListItems")
}

fn position(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

fn sorting_from_header(sorter: &gtk::ColumnViewSorter) -> Option<Sorting> {
    let id = sorter.primary_sort_column()?.id()?;
    let (column, _, _) = COLUMNS.iter().find(|(_, column_id, _)| *column_id == id.as_str())?;
    let direction = match sorter.primary_sort_order() {
        gtk::SortType::Descending => SortDirection::Descending,
        _ => SortDirection::Ascending,
    };
    Some(Sorting { column: *column, direction })
}
