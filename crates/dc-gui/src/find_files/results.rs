//! The "Results" page: what the search found, while it searches too, and
//! what to do with it.

use std::path::{Path, PathBuf};

use dc_core::find::Found;
use dc_core::format;
use dc_i18n::{fill, gettext, pgettext};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use jiff::tz::TimeZone;

mod imp {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    /// One item found.
    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::FoundRow)]
    pub struct FoundRow {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        folder: RefCell<String>,
        #[property(get, set)]
        size: RefCell<String>,
        #[property(get, set)]
        modified: RefCell<String>,
        #[property(get, set)]
        group: RefCell<String>,
        pub path: RefCell<PathBuf>,
        pub is_dir: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FoundRow {
        const NAME: &'static str = "DcFoundRow";
        type Type = super::FoundRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FoundRow {}
}

glib::wrapper! {
    pub struct FoundRow(ObjectSubclass<imp::FoundRow>);
}

impl FoundRow {
    fn new(found: &Found, tz: &TimeZone) -> Self {
        let name = found.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let row: Self = glib::Object::builder()
            .property("name", if found.is_dir { format!("[{name}]") } else { name })
            .property("folder", found.path.parent().map(|dir| dir.display().to_string()).unwrap_or_default())
            .property("size", if found.is_dir { pgettext("size column", "<DIR>") } else { format::size(found.size) })
            .property("modified", format::modified(found.modified, tz))
            .property("group", found.group.map(|group| group.to_string()).unwrap_or_default())
            .build();
        found.path.clone_into(&mut row.imp().path.borrow_mut());
        row.imp().is_dir.set(found.is_dir);
        row
    }

    pub fn path(&self) -> PathBuf {
        self.imp().path.borrow().clone()
    }

    pub fn is_dir(&self) -> bool {
        self.imp().is_dir.get()
    }
}

pub(super) struct Results {
    pub page: gtk::Box,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    pub view: gtk::ColumnView,
    group_column: gtk::ColumnViewColumn,
    status: gtk::Label,
    current: gtk::Label,
    pub feed: gtk::Button,
    pub go_to: gtk::Button,
    pub open: gtk::Button,
    pub remove: gtk::Button,
    tz: TimeZone,
}

impl Results {
    pub fn new() -> Self {
        let store = gio::ListStore::new::<FoundRow>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let view = gtk::ColumnView::builder().model(&selection).reorderable(false).vexpand(true).build();
        view.add_css_class("data-table");
        let mut group_column = None;
        for (title, property, expand, right) in [
            (gettext("Group"), "group", false, true),
            (gettext("Name"), "name", true, false),
            (gettext("Directory"), "folder", true, false),
            (gettext("Size"), "size", false, true),
            (gettext("Date"), "modified", false, false),
        ] {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().expect("column factories get ListItems");
                let label = gtk::Label::builder()
                    .xalign(if right { 1.0 } else { 0.0 })
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                item.property_expression("item").chain_property::<FoundRow>(property).bind(&label, "label", Some(item));
                item.set_child(Some(&label));
            });
            let column = gtk::ColumnViewColumn::builder().title(title).factory(&factory).resizable(true).expand(expand).build();
            view.append_column(&column);
            if property == "group" {
                column.set_visible(false);
                group_column = Some(column);
            }
        }
        let this = Self {
            page: gtk::Box::new(gtk::Orientation::Vertical, 6),
            store,
            selection,
            view,
            group_column: group_column.expect("the group column is in the list"),
            status: gtk::Label::builder().xalign(0.0).build(),
            current: gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["dim-label"]).build(),
            feed: gtk::Button::with_mnemonic(&gettext("Feed to _panel")),
            go_to: gtk::Button::with_mnemonic(&gettext("_Go to file")),
            open: gtk::Button::with_mnemonic(&gettext("_Open")),
            remove: gtk::Button::with_mnemonic(&gettext("_Remove from list")),
            tz: TimeZone::system(),
        };
        this.feed.set_tooltip_text(Some(&gettext("The items found in the active panel, to work with them as with files of a directory")));
        this.go_to.set_tooltip_text(Some(&gettext("Enter (Space keeps this window open)")));
        this.open.set_tooltip_text(Some("F4"));
        this.remove.set_tooltip_text(Some("Del"));
        let scrolled = gtk::ScrolledWindow::builder().child(&this.view).vexpand(true).build();
        let buttons = gtk::Box::builder().spacing(8).build();
        buttons.append(&this.feed);
        buttons.append(&gtk::Box::builder().hexpand(true).build());
        for button in [&this.go_to, &this.open, &this.remove] {
            buttons.append(button);
        }
        this.page.append(&this.status);
        this.page.append(&this.current);
        this.page.append(&scrolled);
        this.page.append(&buttons);
        this.update_buttons();
        this
    }

    pub fn clear(&self, duplicates: bool) {
        self.store.remove_all();
        self.group_column.set_visible(duplicates);
        self.update_buttons();
    }

    pub fn append(&self, found: &[Found]) {
        let rows: Vec<FoundRow> = found.iter().map(|found| FoundRow::new(found, &self.tz)).collect();
        let was_empty = self.store.n_items() == 0;
        self.store.extend_from_slice(&rows);
        if was_empty && !rows.is_empty() {
            self.selection.select_item(0, true);
        }
        self.update_buttons();
    }

    /// "Scanned: N, found: M" and what is going on.
    pub fn set_status(&self, scanned: u64, found: usize, current: &str) {
        self.status.set_text(&fill(&gettext("Scanned: {scanned}    Found: {found}"), &[("scanned", &scanned), ("found", &found)]));
        self.current.set_text(current);
    }

    pub fn update_buttons(&self) {
        let any = self.store.n_items() > 0;
        for button in [&self.feed, &self.go_to, &self.open, &self.remove] {
            button.set_sensitive(any);
        }
    }

    fn rows(&self) -> impl Iterator<Item = FoundRow> + '_ {
        (0..self.store.n_items()).filter_map(|index| self.store.item(index).and_downcast::<FoundRow>())
    }

    /// The row the actions are for: the first one selected.
    pub fn current(&self) -> Option<FoundRow> {
        let selected = self.selection.selection();
        if selected.is_empty() {
            return None;
        }
        self.store.item(selected.minimum()).and_downcast()
    }

    /// Paths of all rows.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.rows().map(|row| row.path()).collect()
    }

    /// Selected rows go away, the cursor stays where they were.
    pub fn remove_selected(&self) {
        let selected = self.selection.selection();
        let Some(first) = (!selected.is_empty()).then(|| selected.minimum()) else { return };
        for index in (0..self.store.n_items()).rev() {
            if selected.contains(index) {
                self.store.remove(index);
            }
        }
        let count = self.store.n_items();
        if count > 0 {
            self.selection.select_item(first.min(count - 1), true);
        }
        self.update_buttons();
    }

    /// Focus on the list, at the row selected.
    pub fn focus(&self) {
        if let Some(position) = (!self.selection.selection().is_empty()).then(|| self.selection.selection().minimum()) {
            self.view.scroll_to(position, None, gtk::ListScrollFlags::FOCUS, None);
        } else {
            self.view.grab_focus();
        }
    }
}

/// The start directory a list of found items is shown under: the deepest
/// directory all of them are in.
pub(super) fn common_dir(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut common = paths.first()?.parent()?.to_owned();
    for path in &paths[1..] {
        while !path.starts_with(&common) {
            common = common.parent().map(Path::to_owned)?;
        }
    }
    Some(common)
}
