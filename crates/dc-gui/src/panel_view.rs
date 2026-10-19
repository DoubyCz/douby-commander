//! One file panel: path header, file list and status line.
//!
//! The list only renders [`Panel`] state. Sorting, cursor movement on
//! navigation and selection all live in `dc-core`, column headers just
//! show the sort indicator and report clicks.

use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use dc_core::command::{Direction, RangeStep};
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
    /// Details: one row per item, all columns.
    column_view: gtk::ColumnView,
    /// List: names only, in columns side by side, scrolled horizontally.
    grid_view: gtk::GridView,
    /// Icons: icons or previews with names, in rows, scrolled vertically.
    icon_view: gtk::GridView,
    /// Sort headers above the list and icons views: the details columns
    /// without rows, so they look and work exactly as the details header.
    sort_headers: [gtk::ColumnView; 2],
    /// Previews of images in the icons view.
    thumbnails: RefCell<Thumbnails>,
    /// Shows one of the two views; both are views of the same rows.
    views: gtk::Stack,
    mode: Cell<ViewMode>,
    /// Last seen height of a details row, the list view uses the same.
    details_row_height: Cell<i32>,
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
    scroll_memory: RefCell<HashMap<PathBuf, (ViewMode, f64)>>,
    size_jobs: RefCell<SizeJobs>,
    this: Weak<Self>,
}

/// Sizes of directories to calculate (Space), one after another in a
/// worker thread, so that the panel stays usable meanwhile. Leaving the
/// directory cancels them: the results would not be shown anyway.
#[derive(Debug, Default)]
struct SizeJobs {
    /// Directory whose items are calculated.
    dir: PathBuf,
    queue: VecDeque<OsString>,
    /// Being calculated now.
    current: Option<OsString>,
    /// Set to stop the calculation running for `dir`.
    cancel: Arc<AtomicBool>,
    /// A task works the queue off.
    running: bool,
}

type PathListener = Box<dyn Fn(&Path)>;

/// How a panel shows its items (`cm_BriefView`, `cm_ColumnsView` of the
/// original, there "brief" and "full"; icons will follow).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// All columns, one row per item.
    Details,
    /// Names only, in columns side by side, scrolled horizontally.
    List,
    /// Icons or previews with the name below, scrolled vertically.
    Icons,
}

impl ViewMode {
    fn name(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::List => "list",
            Self::Icons => "icons",
        }
    }
}

/// Size of an icon or preview in the icons view (`gThumbSize` of the
/// original, 128×128).
const ICON_SIZE: i32 = 128;
/// Previews are made this much larger, sharp on scaled displays.
const THUMBNAIL_SCALE: i32 = 2;

/// Previews of images for the icons view, made one after another in a
/// worker thread for the items shown, kept for the current directory only.
#[derive(Debug, Default)]
struct Thumbnails {
    dir: PathBuf,
    ready: HashMap<OsString, gdk::Texture>,
    /// Asked for (queued, being made, or failed), not asked again.
    requested: std::collections::HashSet<OsString>,
    queue: VecDeque<OsString>,
    cancel: Arc<AtomicBool>,
    running: bool,
}

/// Pixels of a preview, made in a worker thread: textures are made from
/// them in the main thread.
struct PreviewPixels {
    width: i32,
    height: i32,
    stride: usize,
    alpha: bool,
    bytes: glib::Bytes,
}

/// A preview of the image `path`: the desktop's own (`~/.cache/thumbnails`,
/// shared with other file managers) if there is a valid one, else the
/// image scaled down to `size`.
fn load_preview(path: &Path, size: i32) -> Option<PreviewPixels> {
    let file = gio::File::for_path(path);
    let cached = file
        .query_info("thumbnail::path,thumbnail::is-valid", gio::FileQueryInfoFlags::NONE, None::<&gio::Cancellable>)
        .ok()
        .filter(|info| info.boolean("thumbnail::is-valid"))
        .and_then(|info| info.attribute_byte_string("thumbnail::path"));
    let pixbuf = match cached {
        Some(thumbnail) => gtk::gdk_pixbuf::Pixbuf::from_file(thumbnail.as_str()),
        None => gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, size, size, true),
    }
    .ok()?;
    Some(PreviewPixels {
        width: pixbuf.width(),
        height: pixbuf.height(),
        stride: usize::try_from(pixbuf.rowstride()).ok()?,
        alpha: pixbuf.has_alpha(),
        bytes: pixbuf.read_pixel_bytes(),
    })
}

impl PreviewPixels {
    fn texture(&self) -> gdk::Texture {
        let format = if self.alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
        gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.stride).upcast()
    }
}

/// Icon of a file type for the icons view, by name as the desktop does.
fn type_icon(name: &OsStr, directory: bool) -> (gio::Icon, bool) {
    if directory {
        return (gio::ThemedIcon::new("folder").upcast(), false);
    }
    let (content_type, _) = gio::content_type_guess(Some(Path::new(name)), None);
    let image = content_type.starts_with("image/");
    (gio::content_type_get_icon(&content_type), image)
}

/// Space around a name in the list view, matches `.dc-list-view > child`
/// in the base style (1px top and bottom, 6px left and right).
const LIST_CELL_PADDING: i32 = 2;
const LIST_CELL_SIDE_PADDING: i32 = 12;

/// Width of a column of the list view in `rem`, enough for a name.
const LIST_COLUMN_REM: f64 = 20.0;

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

        let details = gtk::ScrolledWindow::builder().child(&column_view).vexpand(true).build();

        // The list view: columns of names filled top to bottom, as many rows
        // as fit (set on resize), scrolled sideways.
        let grid_view = gtk::GridView::builder().model(&selection).orientation(gtk::Orientation::Horizontal).build();
        grid_view.add_css_class("dc-list-view");
        let list = gtk::ScrolledWindow::builder()
            .child(&grid_view)
            .vexpand(true)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let list_header = sort_header();
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list_box.append(&list_header);
        list_box.append(&list);

        // The icons view: icons or previews with the name below, in rows,
        // scrolled vertically.
        let icon_view = gtk::GridView::builder().model(&selection).max_columns(64).build();
        icon_view.add_css_class("dc-icon-view");
        let icons = gtk::ScrolledWindow::builder()
            .child(&icon_view)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let icons_header = sort_header();
        let icons_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        icons_box.append(&icons_header);
        icons_box.append(&icons);

        let views = gtk::Stack::builder().vexpand(true).build();
        views.add_named(&details, Some(ViewMode::Details.name()));
        views.add_named(&list_box, Some(ViewMode::List.name()));
        views.add_named(&icons_box, Some(ViewMode::Icons.name()));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("dc-panel");
        root.append(&path_label);
        root.append(&views);
        root.append(&status_label);

        let view = Rc::new_cyclic(|this| Self {
            panel: RefCell::new(Panel::new(path.clone())),
            source: LocalFs,
            tz: TimeZone::system(),
            store,
            selection,
            column_view,
            grid_view,
            icon_view,
            sort_headers: [list_header, icons_header],
            thumbnails: RefCell::default(),
            views,
            mode: Cell::new(ViewMode::Details),
            details_row_height: Cell::new(0),
            path_label,
            status_label,
            root,
            syncing: Cell::new(false),
            type_ahead: RefCell::new(TypeAhead::default()),
            path_listeners: RefCell::default(),
            scroll_memory: RefCell::default(),
            size_jobs: RefCell::default(),
            this: this.clone(),
        });
        view.setup_columns();
        view.setup_list_view(&list);
        view.setup_icon_view();
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
        self.scroll_to_row(position(cursor), gtk::ListScrollFlags::FOCUS);
        self.list_widget().grab_focus();
    }

    pub fn connect_focus_enter(&self, callback: impl Fn() + 'static) {
        let callback = Rc::new(callback);
        for widget in [self.column_view.upcast_ref::<gtk::Widget>(), self.grid_view.upcast_ref(), self.icon_view.upcast_ref()] {
            let focus = gtk::EventControllerFocus::new();
            let callback = Rc::clone(&callback);
            focus.connect_enter(move |_| callback());
            widget.add_controller(focus);
        }
    }

    /// Insert.
    pub fn toggle_selection_and_move(&self, forward: bool) {
        self.panel.borrow_mut().toggle_selection_and_move(forward);
        self.after_marking();
    }

    pub fn end_held_mark(&self) {
        self.panel.borrow_mut().end_held_mark();
    }

    /// Shift+arrows. The step follows the layout: in details up and down by
    /// an item (left and right nothing), in the list view left and right by
    /// a column, in the icons view up and down by a row.
    pub fn mark_and_move(&self, direction: Direction) {
        let across = |count: usize| isize::try_from(count).unwrap_or(1);
        let step = match (self.mode.get(), direction) {
            (ViewMode::Details, Direction::Left | Direction::Right) => return,
            (ViewMode::Details | ViewMode::List, Direction::Up) | (ViewMode::Icons, Direction::Left) => -1,
            (ViewMode::Details | ViewMode::List, Direction::Down) | (ViewMode::Icons, Direction::Right) => 1,
            (ViewMode::List, Direction::Left) => -across(self.list_rows()),
            (ViewMode::List, Direction::Right) => across(self.list_rows()),
            (ViewMode::Icons, Direction::Up) => -across(self.icon_columns()),
            (ViewMode::Icons, Direction::Down) => across(self.icon_columns()),
        };
        self.panel.borrow_mut().mark_and_move(step);
        self.after_marking();
    }

    /// Space: toggles the mark; a directory being marked gets its size
    /// calculated in the background (once per listing).
    pub fn toggle_selection(&self) {
        let directory = self
            .panel
            .borrow()
            .current_item()
            .filter(|item| !item.is_parent() && item.is_directory_like() && item.calculated_size().is_none())
            .map(|item| item.entry().name.clone());
        self.panel.borrow_mut().toggle_selection_at_cursor();
        self.after_marking();
        let marked = self.panel.borrow().current_item().is_some_and(dc_core::FileItem::is_selected);
        if let Some(name) = directory
            && marked
        {
            self.calculate_size(name);
        }
    }

    /// Queues the size of directory `name` of the current directory.
    fn calculate_size(&self, name: OsString) {
        let dir = self.path();
        let mut jobs = self.size_jobs.borrow_mut();
        if jobs.dir != dir {
            Self::cancel_size_jobs(&mut jobs, dir);
        }
        if jobs.current.as_ref() == Some(&name) || jobs.queue.contains(&name) {
            return;
        }
        jobs.queue.push_back(name);
        if jobs.running {
            return;
        }
        jobs.running = true;
        drop(jobs);

        let weak = self.this.clone();
        glib::spawn_future_local(async move {
            loop {
                let Some(view) = weak.upgrade() else { return };
                let job = {
                    let mut jobs = view.size_jobs.borrow_mut();
                    jobs.current = jobs.queue.pop_front();
                    if jobs.current.is_none() {
                        jobs.running = false;
                    }
                    jobs.current.clone().map(|name| (jobs.dir.clone(), name, Arc::clone(&jobs.cancel)))
                };
                // Not kept while waiting, the panel may go away meanwhile.
                drop(view);
                let Some((dir, name, cancel)) = job else { return };

                let (path, stop) = (dir.join(&name), Arc::clone(&cancel));
                let size = gio::spawn_blocking(move || dc_vfs::tree_size(&path, &stop)).await.ok().flatten();

                let Some(view) = weak.upgrade() else { return };
                view.size_jobs.borrow_mut().current = None;
                if let Some(size) = size
                    && !cancel.load(Ordering::Relaxed)
                    && view.path() == dir
                {
                    view.show_calculated_size(&name, size);
                }
            }
        });
    }

    /// Stops calculating sizes and forgets the queue, the panel shows `dir` now.
    fn cancel_size_jobs(jobs: &mut SizeJobs, dir: PathBuf) {
        jobs.cancel.store(true, Ordering::Relaxed);
        jobs.cancel = Arc::new(AtomicBool::new(false));
        jobs.queue.clear();
        jobs.dir = dir;
    }

    fn show_calculated_size(&self, name: &OsStr, size: u64) {
        let index = self.panel.borrow_mut().set_calculated_size(name, size);
        if let Some(index) = index
            && let Some(row) = self.store.item(position(index)).and_downcast::<FileRow>()
        {
            row.set_size_text(format::size(size));
        }
        self.update_status();
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

    /// The scrolling of the current view: vertical in details, horizontal
    /// in the list view.
    fn scroll_adjustment(&self) -> Option<gtk::Adjustment> {
        match self.mode.get() {
            ViewMode::Details => self.column_view.vadjustment(),
            ViewMode::List => self.grid_view.hadjustment(),
            ViewMode::Icons => self.icon_view.vadjustment(),
        }
    }

    /// The directory being left and how far its view is scrolled.
    fn leaving(&self) -> (PathBuf, ViewMode, f64) {
        let scrolled = self.scroll_adjustment().map_or(0.0, |adjustment| adjustment.value());
        (self.path(), self.mode.get(), scrolled)
    }

    /// After moving to another directory: going down remembers how the
    /// directory left was scrolled, going up shows it scrolled the same
    /// way again, with the cursor on the directory we came from at the
    /// same place on screen. Only within one view mode.
    fn arrived(&self, (left, mode, scrolled): (PathBuf, ViewMode, f64)) {
        let now = self.path();
        // Sizes and previews being made are of the directory left. Before
        // the new rows are shown: showing them asks for their previews.
        Self::cancel_size_jobs(&mut self.size_jobs.borrow_mut(), now.clone());
        Self::forget_thumbnails(&mut self.thumbnails.borrow_mut(), now.clone());
        self.refresh();
        if now.parent() == Some(left.as_path()) {
            self.scroll_memory.borrow_mut().insert(left, (mode, scrolled));
        } else if left.parent() == Some(now.as_path())
            && let Some(&(remembered_mode, remembered)) = self.scroll_memory.borrow().get(&now)
            && remembered_mode == self.mode.get()
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

    /// Scrolls back to `value`, as the directory was scrolled when left.
    /// No estimate of where the cursor is: an estimate one row off refused
    /// views with the cursor on the bottom row.
    fn restore_scroll(&self, value: f64) {
        let Some(adjustment) = self.scroll_adjustment() else { return };
        if self.store.n_items() == 0 || adjustment.upper() <= 0.0 {
            return;
        }
        adjustment.set_value(value.clamp(0.0, (adjustment.upper() - adjustment.page_size()).max(0.0)));
        // Should the directory have changed meanwhile, the cursor is
        // brought into view by the least scrolling; otherwise this keeps
        // the restored view as it is.
        self.scroll_to_row(position(self.panel.borrow().cursor()), gtk::ListScrollFlags::NONE);
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
                if numeric {
                    // Follows the row: a directory's size appears once calculated.
                    list_item
                        .property_expression("item")
                        .chain_property::<FileRow>("size-text")
                        .bind(&label, "label", Some(list_item));
                }
                setup_cell(&this, list_item, &label);
            });
            factory.connect_bind(move |_, object| {
                let list_item = list_item(object);
                let row = list_item.item().and_downcast::<FileRow>().expect("rows are FileRow");
                let label = list_item.child().and_downcast::<gtk::Label>().expect("child is set up as Label");
                let texts = row.texts();
                match sort_column {
                    SortColumn::Name => label.set_text(&texts.name),
                    SortColumn::Extension => label.set_text(&texts.extension),
                    // Bound to the row's `size-text` in setup.
                    SortColumn::Size => {}
                    SortColumn::Modified => label.set_text(&texts.modified),
                    SortColumn::Attributes => label.set_text(&texts.attributes),
                }
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

            // The same column in the headers of the list and icons views, without cells.
            for header in &self.sort_headers {
                let header_column = gtk::ColumnViewColumn::builder()
                    .id(id)
                    .title(gettext(title))
                    .factory(&gtk::SignalListItemFactory::new())
                    .sorter(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal))
                    .expand(matches!(sort_column, SortColumn::Name))
                    .build();
                header.append_column(&header_column);
            }
        }

        let first = self.column_view.columns().item(0).and_downcast::<gtk::ColumnViewColumn>();
        self.column_view.sort_by_column(first.as_ref(), gtk::SortType::Ascending);
    }

    /// The list view: full names, bold directories, marks as in details.
    /// Its rows follow the height of the panel.
    fn setup_list_view(&self, scrolled: &gtk::ScrolledWindow) {
        let factory = gtk::SignalListItemFactory::new();
        let this = self.this.clone();
        let column_width = self.list_column_width();
        factory.connect_setup(move |_, object| {
            // The grid makes every column as wide as the minimum width of
            // its cells, so that is set; longer names are ellipsized.
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(1)
                .width_request(column_width)
                .build();
            setup_cell(&this, list_item(object), &label);
        });
        factory.connect_bind(|_, object| {
            let list_item = list_item(object);
            let row = list_item.item().and_downcast::<FileRow>().expect("rows are FileRow");
            let label = list_item.child().and_downcast::<gtk::Label>().expect("child is set up as Label");
            let texts = row.texts();
            label.set_text(&texts.full_name);
            label.set_attributes(texts.directory.then(bold_attributes).as_ref());
        });
        self.grid_view.set_factory(Some(&factory));

        let weak = self.this.clone();
        self.grid_view.connect_activate(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.open_current();
            }
        });

        // As many rows as fit: the grid's "columns" are rows when it is
        // horizontal, both limits the same makes it exact.
        let weak = self.this.clone();
        scrolled.vadjustment().connect_page_size_notify(move |adjustment| {
            if let Some(view) = weak.upgrade() {
                let row_height = view.list_row_height();
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a small positive row count, truncation intended"
                )]
                let rows = (adjustment.page_size() / f64::from(row_height)).floor().max(1.0) as u32;
                if rows != view.grid_view.max_columns() {
                    // Not now: this runs while the grid is laid out, a change
                    // made here would show only after the next relayout
                    // (e.g. scrolling), with the old rows stretched meanwhile.
                    let weak = view.this.clone();
                    glib::idle_add_local_once(move || {
                        if let Some(view) = weak.upgrade() {
                            view.grid_view.set_max_columns(rows.max(view.grid_view.min_columns()));
                            view.grid_view.set_min_columns(rows);
                            view.grid_view.set_max_columns(rows);
                        }
                    });
                }
            }
        });
    }

    /// The icons view: a type icon or a preview, the name below in up to
    /// two lines; previews are asked for as items come into view.
    fn setup_icon_view(&self) {
        let factory = gtk::SignalListItemFactory::new();
        let this = self.this.clone();
        factory.connect_setup(move |_, object| {
            let list_item = list_item(object);
            let icon = gtk::Image::builder().pixel_size(ICON_SIZE).build();
            let preview = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .width_request(ICON_SIZE)
                .height_request(ICON_SIZE)
                .build();
            let label = gtk::Label::builder()
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .justify(gtk::Justification::Center)
                .max_width_chars(1)
                .width_request(ICON_SIZE)
                .build();
            let cell = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
            cell.append(&icon);
            cell.append(&preview);
            cell.append(&label);

            // The preview, once made, replaces the type icon.
            let thumbnail = list_item.property_expression("item").chain_property::<FileRow>("thumbnail");
            thumbnail.bind(&preview, "paintable", Some(list_item));
            thumbnail
                .chain_closure::<bool>(glib::closure!(|_: Option<glib::Object>, t: Option<gdk::Texture>| t.is_some()))
                .bind(&preview, "visible", Some(list_item));
            list_item
                .property_expression("item")
                .chain_property::<FileRow>("thumbnail")
                .chain_closure::<bool>(glib::closure!(|_: Option<glib::Object>, t: Option<gdk::Texture>| t.is_none()))
                .bind(&icon, "visible", Some(list_item));
            setup_cell_with(&this, list_item, &label, cell.upcast_ref());
        });
        let this = self.this.clone();
        factory.connect_bind(move |_, object| {
            let list_item = list_item(object);
            let row = list_item.item().and_downcast::<FileRow>().expect("rows are FileRow");
            let cell = list_item.child().expect("child is set up");
            let texts = row.texts();
            let (icon, image) = type_icon(&texts.file_name, texts.directory);
            if let Some(image_widget) = cell.first_child().and_downcast::<gtk::Image>() {
                image_widget.set_from_gicon(&icon);
            }
            if let Some(label) = cell.last_child().and_downcast::<gtk::Label>() {
                label.set_text(&texts.full_name);
                label.set_attributes(texts.directory.then(bold_attributes).as_ref());
            }
            if image
                && row.thumbnail().is_none()
                && let Some(view) = this.upgrade()
            {
                view.request_thumbnail(texts.file_name.clone());
            }
        });
        self.icon_view.set_factory(Some(&factory));

        let weak = self.this.clone();
        self.icon_view.connect_activate(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.open_current();
            }
        });
    }

    /// Icons per row of the icons view, as laid out.
    fn icon_columns(&self) -> usize {
        let cell = self.icon_view.first_child().map_or(0, |cell| cell.width());
        if cell <= 0 {
            return 1;
        }
        usize::try_from(self.icon_view.width() / cell).unwrap_or(1).max(1)
    }

    /// Queues a preview of the image `name` of the current directory.
    fn request_thumbnail(&self, name: OsString) {
        let dir = self.path();
        let mut thumbnails = self.thumbnails.borrow_mut();
        if thumbnails.dir != dir {
            Self::forget_thumbnails(&mut thumbnails, dir);
        }
        if !thumbnails.requested.insert(name.clone()) {
            return;
        }
        thumbnails.queue.push_back(name);
        if thumbnails.running {
            return;
        }
        thumbnails.running = true;
        drop(thumbnails);

        let weak = self.this.clone();
        glib::spawn_future_local(async move {
            loop {
                let Some(view) = weak.upgrade() else { return };
                let job = {
                    let mut thumbnails = view.thumbnails.borrow_mut();
                    let next = thumbnails.queue.pop_front();
                    if next.is_none() {
                        thumbnails.running = false;
                    }
                    next.map(|name| (thumbnails.dir.clone(), name, Arc::clone(&thumbnails.cancel)))
                };
                drop(view);
                let Some((dir, name, cancel)) = job else { return };

                let path = dir.join(&name);
                let stop = Arc::clone(&cancel);
                let pixels = gio::spawn_blocking(move || {
                    if stop.load(Ordering::Relaxed) { None } else { load_preview(&path, ICON_SIZE * THUMBNAIL_SCALE) }
                })
                .await
                .ok()
                .flatten();

                let Some(view) = weak.upgrade() else { return };
                if let Some(pixels) = pixels
                    && !cancel.load(Ordering::Relaxed)
                    && view.path() == dir
                {
                    let texture = pixels.texture();
                    let index = view.panel.borrow().position_of(&name);
                    if let Some(row) = index.and_then(|index| view.store.item(position(index))).and_downcast::<FileRow>() {
                        row.set_thumbnail(Some(&texture));
                    }
                    view.thumbnails.borrow_mut().ready.insert(name, texture);
                }
            }
        });
    }

    /// Previews are of the directory shown: a new one starts empty, what
    /// is being made for the old one stops.
    fn forget_thumbnails(thumbnails: &mut Thumbnails, dir: PathBuf) {
        thumbnails.cancel.store(true, Ordering::Relaxed);
        thumbnails.cancel = Arc::new(AtomicBool::new(false));
        thumbnails.queue.clear();
        thumbnails.requested.clear();
        thumbnails.ready.clear();
        thumbnails.dir = dir;
    }

    /// Width of a name in the list view: `LIST_COLUMN_REM` times the font
    /// size (as CSS `rem`), less the horizontal padding of the cell.
    fn list_column_width(&self) -> i32 {
        let font = self.grid_view.pango_context().font_description();
        let size = font.as_ref().map_or(0.0, |font| f64::from(font.size()) / f64::from(gtk::pango::SCALE));
        // Points to pixels at the 96 dpi GTK works in, unless given in pixels.
        let pixels = if font.as_ref().is_some_and(gtk::pango::FontDescription::is_size_absolute) { size } else { size * 96.0 / 72.0 };
        let pixels = if pixels > 0.0 { pixels } else { 16.0 };
        #[expect(clippy::cast_possible_truncation, reason = "a width of a few hundred pixels")]
        let width = (pixels * LIST_COLUMN_REM).round() as i32;
        width - LIST_CELL_SIDE_PADDING
    }

    /// Height of one name in the list view: that of a details row, so both
    /// views space their rows alike (the grid stretches its rows to it).
    /// Before any details row was laid out, a line of text and the padding.
    fn list_row_height(&self) -> i32 {
        let details_row = self
            .column_view
            .last_child()
            .and_then(|list| list.first_child())
            .map(|row| row.height())
            .filter(|height| *height > 0);
        if let Some(height) = details_row {
            self.details_row_height.set(height);
        }
        match self.details_row_height.get() {
            0 => {
                let (_, text_height) = self.grid_view.create_pango_layout(Some("Ág")).pixel_size();
                (text_height + LIST_CELL_PADDING).max(1)
            }
            height => height,
        }
    }

    /// Rows per column of the list view.
    fn list_rows(&self) -> usize {
        usize::try_from(self.grid_view.max_columns()).unwrap_or(1).max(1)
    }

    /// `cm_BriefView` / `cm_ColumnsView`: the same items, cursor, marks and
    /// sorting in another view.
    pub fn set_view_mode(&self, mode: ViewMode) {
        if mode == self.mode.get() {
            return;
        }
        let had_focus = self.has_focus();
        self.mode.set(mode);
        self.views.set_visible_child_name(mode.name());
        if had_focus {
            self.focus();
        } else {
            self.show_cursor();
        }
    }

    /// The widget of the current view.
    fn list_widget(&self) -> gtk::Widget {
        match self.mode.get() {
            ViewMode::Details => self.column_view.clone().upcast(),
            ViewMode::List => self.grid_view.clone().upcast(),
            ViewMode::Icons => self.icon_view.clone().upcast(),
        }
    }

    /// Whether the keyboard focus is in this panel's list.
    pub fn has_focus(&self) -> bool {
        let widget = self.list_widget();
        widget.has_focus() || widget.focus_child().is_some()
    }

    fn scroll_to_row(&self, index: u32, flags: gtk::ListScrollFlags) {
        match self.mode.get() {
            ViewMode::Details => self.column_view.scroll_to(index, None, flags, None),
            ViewMode::List => self.grid_view.scroll_to(index, flags, None),
            ViewMode::Icons => self.icon_view.scroll_to(index, flags, None),
        }
    }

    /// `cm_SortByName` and the like, as a click on the header: the same
    /// column again reverses, a new one starts descending for size and date
    /// (largest, newest first) as `DoSortByFunctions` of the original.
    pub fn sort_by(&self, column: SortColumn) {
        let current = self.panel.borrow().sorting();
        let direction = if current.column == column {
            current.direction.reversed()
        } else if matches!(column, SortColumn::Size | SortColumn::Modified) {
            SortDirection::Descending
        } else {
            SortDirection::Ascending
        };
        self.set_sorting(Sorting { column, direction });
    }

    /// `cm_ReverseOrder`.
    pub fn reverse_order(&self) {
        let current = self.panel.borrow().sorting();
        self.set_sorting(Sorting { direction: current.direction.reversed(), ..current });
    }

    fn set_sorting(&self, sorting: Sorting) {
        self.panel.borrow_mut().set_sorting(sorting);
        // Rows, status and the arrows of all headers.
        self.refresh();
    }

    /// Shows `sorting` in a header (its arrow), unless it shows it already.
    /// Changing it tells the panel through the sorter, which then sees no
    /// change and stops.
    fn show_sorting_in(header: &gtk::ColumnView, sorting: Sorting) {
        let shown = header.sorter().and_downcast::<gtk::ColumnViewSorter>().and_then(|sorter| sorting_from_header(&sorter));
        if shown == Some(sorting) {
            return;
        }
        let id = COLUMNS.iter().find(|(column, ..)| *column == sorting.column).map(|(_, id, _)| *id);
        let columns = header.columns();
        let column = (0..columns.n_items())
            .filter_map(|index| columns.item(index).and_downcast::<gtk::ColumnViewColumn>())
            .find(|candidate| candidate.id().as_deref() == id);
        let order = match sorting.direction {
            SortDirection::Ascending => gtk::SortType::Ascending,
            SortDirection::Descending => gtk::SortType::Descending,
        };
        header.sort_by_column(column.as_ref(), order);
    }

    /// All headers show the panel's sorting.
    fn update_list_headers(&self) {
        let sorting = self.panel.borrow().sorting();
        Self::show_sorting_in(&self.column_view, sorting);
        for header in &self.sort_headers {
            Self::show_sorting_in(header, sorting);
        }
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

        // A click on any header sorts the panel; the others then show it
        // too (`update_list_headers`), without sorting again.
        for header in std::iter::once(&self.column_view).chain(&self.sort_headers) {
            let Some(sorter) = header.sorter().and_downcast::<gtk::ColumnViewSorter>() else { continue };
            let weak = Rc::downgrade(self);
            sorter.connect_changed(move |sorter, _| {
                if let Some(view) = weak.upgrade()
                    && let Some(sorting) = sorting_from_header(sorter)
                    && sorting != view.panel.borrow().sorting()
                {
                    view.panel.borrow_mut().set_sorting(sorting);
                    view.refresh();
                }
            });
        }
    }

    /// Rebuilds all rows from the panel state.
    fn refresh(&self) {
        // New rows replace the focused one; the focus would fall to some
        // other widget (a column header) and the arrows would move there.
        let had_focus = self.has_focus();
        let panel = self.panel.borrow();
        // Previews made already stay with their rows (sorting, marks).
        let thumbnails = self.thumbnails.borrow();
        let same_dir = thumbnails.dir == panel.path();
        let rows: Vec<FileRow> = panel
            .items()
            .iter()
            .map(|item| {
                let row = FileRow::new(item, &self.tz);
                if same_dir && let Some(texture) = thumbnails.ready.get(&item.entry().name) {
                    row.set_thumbnail(Some(texture));
                }
                row
            })
            .collect();
        drop(thumbnails);

        self.syncing.set(true);
        self.store.splice(0, self.store.n_items(), &rows);
        self.syncing.set(false);

        self.path_label.set_text(&panel.path().display().to_string());
        let path = panel.path().to_owned();
        drop(panel);
        self.update_status();
        self.update_list_headers();
        if had_focus {
            self.focus();
        } else {
            self.show_cursor();
        }
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
        // The list view moves by a column less one, as the original.
        if self.mode.get() == ViewMode::List {
            return self.list_rows().saturating_sub(1).max(1);
        }
        // The icons view by the rows fully visible.
        if self.mode.get() == ViewMode::Icons {
            let cell = self.icon_view.first_child().map_or(0, |cell| cell.height());
            let visible = if cell > 0 { self.icon_view.height() / cell } else { 1 };
            return self.icon_columns() * usize::try_from(visible).unwrap_or(1).max(1);
        }
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

        let flags = if self.has_focus() { gtk::ListScrollFlags::FOCUS } else { gtk::ListScrollFlags::NONE };
        self.scroll_to_row(cursor, flags);
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

/// What every cell showing a row does, in both views: Ctrl+click and
/// Shift+click mark files (the cell handles them before the row does,
/// which would otherwise just move the cursor), and the label follows
/// `marked` of whatever row it currently shows, so a changed mark restyles
/// it without rebinding.
fn setup_cell(view: &Weak<PanelView>, list_item: &gtk::ListItem, label: &gtk::Label) {
    setup_cell_with(view, list_item, label, label.upcast_ref());
}

/// As [`setup_cell`] for a cell whose `child` holds the `label` and more.
fn setup_cell_with(view: &Weak<PanelView>, list_item: &gtk::ListItem, label: &gtk::Label, child: &gtk::Widget) {
    let click = gtk::GestureClick::new();
    let (view, item) = (view.clone(), list_item.downgrade());
    click.connect_pressed(move |gesture, _, _, _| {
        let modifiers = gesture.current_event_state();
        if !modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK) {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let (Some(view), Some(item)) = (view.upgrade(), item.upgrade())
            && let Ok(index) = usize::try_from(item.position())
        {
            view.clicked_with_modifiers(index, modifiers);
        }
    });
    child.add_controller(click);

    list_item
        .property_expression("item")
        .chain_property::<FileRow>("marked")
        .chain_closure::<Vec<String>>(glib::closure!(|_: Option<glib::Object>, marked: bool| {
            if marked { vec!["dc-marked".to_owned()] } else { Vec::new() }
        }))
        .bind(label, "css-classes", Some(list_item));

    list_item.set_child(Some(child));
}

/// Sort headers for the list and icons views: a details table without
/// rows; its columns are added with those of the details view.
fn sort_header() -> gtk::ColumnView {
    let header = gtk::ColumnView::builder()
        .model(&gtk::NoSelection::new(Some(gio::ListStore::new::<FileRow>())))
        .reorderable(false)
        .focusable(false)
        .build();
    header.add_css_class("data-table");
    header
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
