//! Multi-rename tool (`TfrmMultiRename` of the original, Ctrl+M): the files
//! with their old and new names above, the masks, find and replace and the
//! counter below; every change shows the new names at once.
//!
//! The names come from `dc_core::multi_rename`, the renaming itself from
//! `dc_ops::rename_many`. Presets, variables, the editor of names and the
//! log are not here yet.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use dc_core::multi_rename::{self, CaseStyle, Counter, FindReplace, RenameFile, RenameMask};
use dc_i18n::{fill, gettext, pgettext};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use jiff::tz::TimeZone;

use crate::dialogs;

mod imp {
    use std::cell::RefCell;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    /// One file of the list; the new name changes with the masks.
    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::RenameRow)]
    pub struct RenameRow {
        #[property(get, set)]
        old_name: RefCell<String>,
        #[property(get, set)]
        new_name: RefCell<String>,
        #[property(get, set)]
        path: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RenameRow {
        const NAME: &'static str = "DcRenameRow";
        type Type = super::RenameRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for RenameRow {}
}

glib::wrapper! {
    pub struct RenameRow(ObjectSubclass<imp::RenameRow>);
}

/// Width choices of the counter, as the original's (`cmbxWidth`).
const WIDTHS: [&str; 10] = ["01", "02", "03", "04", "05", "06", "07", "08", "09", "10"];

/// Which mask a placeholder goes into.
#[derive(Debug, Clone, Copy)]
enum Target {
    Name,
    Extension,
}

impl Target {
    fn action(self) -> &'static str {
        match self {
            Self::Name => "insert-name",
            Self::Extension => "insert-extension",
        }
    }
}

struct Tool {
    window: gtk::Window,
    files: RefCell<Vec<RenameFile>>,
    rows: gio::ListStore,
    name: gtk::Entry,
    extension: gtk::Entry,
    name_case: gtk::DropDown,
    extension_case: gtk::DropDown,
    find: gtk::Entry,
    replace: gtk::Entry,
    regex: gtk::CheckButton,
    case_sensitive: gtk::CheckButton,
    in_extension: gtk::CheckButton,
    substitution: gtk::CheckButton,
    only_first: gtk::CheckButton,
    start: gtk::SpinButton,
    step: gtk::SpinButton,
    width: gtk::DropDown,
    rename: gtk::Button,
    tz: TimeZone,
    /// Told after renaming, the panels show the new names.
    renamed: Box<dyn Fn()>,
}

/// Opens the tool for `files`, all of one directory or not.
pub fn show(parent: Option<&gtk::Window>, files: Vec<RenameFile>, renamed: impl Fn() + 'static) {
    if files.is_empty() {
        return;
    }
    let window = gtk::Window::builder().title(gettext("Multi-Rename Tool")).default_width(1000).default_height(640).build();
    window.set_transient_for(parent);

    let case_titles: Vec<String> = CaseStyle::ALL.iter().map(|style| style.title()).collect();
    let case_list = || gtk::DropDown::from_strings(&case_titles.iter().map(String::as_str).collect::<Vec<_>>());
    let spin = |value: f64| {
        let spin = gtk::SpinButton::with_range(-1e15, 1e15, 1.0);
        spin.set_value(value);
        spin.set_digits(0);
        spin
    };
    let check = |label: &str, tooltip: Option<&str>| {
        let check = gtk::CheckButton::with_mnemonic(label);
        check.set_tooltip_text(tooltip);
        check
    };
    let tool = Rc::new(Tool {
        window,
        files: RefCell::new(files),
        rows: gio::ListStore::new::<RenameRow>(),
        name: gtk::Entry::builder().hexpand(true).build(),
        extension: gtk::Entry::builder().hexpand(true).build(),
        name_case: case_list(),
        extension_case: case_list(),
        find: gtk::Entry::builder().hexpand(true).build(),
        replace: gtk::Entry::builder().hexpand(true).build(),
        regex: check(&gettext("Regular e_xpressions"), None),
        case_sensitive: check("A≠a", Some(&gettext("Case sensitive"))),
        in_extension: check("E", Some(&gettext("Replace also in file extensions"))),
        substitution: check(&gettext("_Use substitution"), None),
        only_first: check("1x", Some(&gettext("Replace only once per file"))),
        start: spin(1.0),
        step: spin(1.0),
        width: gtk::DropDown::from_strings(&WIDTHS),
        rename: gtk::Button::with_mnemonic(&pgettext("button", "_Rename")),
        tz: TimeZone::system(),
        renamed: Box::new(renamed),
    });
    tool.lay_out();
    tool.fill_rows();
    tool.reset();
    tool.connect();
    // The handlers hold the tool weakly; the window owns it until it is
    // gone (the handler goes with the window, and the tool with it).
    let owner = Rc::clone(&tool);
    tool.window.connect_destroy(move |_| {
        let _ = &owner;
    });
    tool.window.present();
    tool.name.grab_focus();
}

impl Tool {
    fn lay_out(self: &Rc<Self>) {
        let options = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        options.append(&self.mask_group());
        let right = gtk::Box::new(gtk::Orientation::Vertical, 6);
        right.append(&self.find_replace_group());
        right.append(&self.counter_group());
        right.set_hexpand(true);
        options.append(&right);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&self.file_list());
        content.append(&options);
        content.append(&self.buttons());
        self.window.set_child(Some(&content));
        self.window.set_default_widget(Some(&self.rename));
    }

    /// The files: old name, new name, path.
    fn file_list(&self) -> gtk::ScrolledWindow {
        let selection = gtk::NoSelection::new(Some(self.rows.clone()));
        let list = gtk::ColumnView::builder().model(&selection).reorderable(false).build();
        list.add_css_class("data-table");
        for (title, property, expand) in [
            (gettext("Old File Name"), "old-name", true),
            (gettext("New File Name"), "new-name", true),
            (gettext("File Path"), "path", false),
        ] {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().expect("column factories get ListItems");
                let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
                item.property_expression("item").chain_property::<RenameRow>(property).bind(&label, "label", Some(item));
                item.set_child(Some(&label));
            });
            let column = gtk::ColumnViewColumn::builder().title(title).factory(&factory).resizable(true).expand(expand).build();
            list.append_column(&column);
        }
        gtk::ScrolledWindow::builder().child(&list).vexpand(true).build()
    }

    /// Mask: name and extension, each with its placeholders and case.
    fn mask_group(self: &Rc<Self>) -> gtk::Frame {
        let mask = gtk::Grid::builder().row_spacing(6).column_spacing(6).build();
        for (row, (label, entry, target, case)) in [
            (gettext("File _Name"), &self.name, Target::Name, &self.name_case),
            (gettext("_Extension"), &self.extension, Target::Extension, &self.extension_case),
        ]
        .into_iter()
        .enumerate()
        {
            let row = i32::try_from(row * 2).unwrap_or(0);
            let title = gtk::Label::builder().label(label).use_underline(true).mnemonic_widget(entry).xalign(0.0).build();
            mask.attach(&title, 0, row, 3, 1);
            mask.attach(entry, 0, row + 1, 1, 1);
            mask.attach(&self.placeholder_button(target), 1, row + 1, 1, 1);
            mask.attach(case, 2, row + 1, 1, 1);
        }
        let mask = frame(&gettext("Mask"), &mask);
        mask.set_valign(gtk::Align::Start);
        mask.set_hexpand(true);
        mask
    }

    fn find_replace_group(&self) -> gtk::Frame {
        let find_replace = gtk::Grid::builder().row_spacing(6).column_spacing(6).build();
        for (row, (label, entry)) in [(gettext("_Find..."), &self.find), (gettext("Re_place..."), &self.replace)].into_iter().enumerate() {
            let row = i32::try_from(row).unwrap_or(0);
            let title = gtk::Label::builder().label(label).use_underline(true).mnemonic_widget(entry).xalign(0.0).build();
            find_replace.attach(&title, 0, row, 1, 1);
            find_replace.attach(entry, 1, row, 1, 1);
        }
        let checks = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for check in [&self.regex, &self.case_sensitive, &self.in_extension, &self.substitution, &self.only_first] {
            checks.append(check);
        }
        find_replace.attach(&checks, 0, 2, 2, 1);
        frame(&gettext("Find & Replace"), &find_replace)
    }

    fn counter_group(&self) -> gtk::Frame {
        let counter = gtk::Grid::builder().row_spacing(6).column_spacing(6).build();
        for (row, (label, widget)) in [
            (gettext("S_tart Number"), self.start.upcast_ref::<gtk::Widget>()),
            (gettext("_Interval"), self.step.upcast_ref()),
            (gettext("_Width"), self.width.upcast_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            let row = i32::try_from(row).unwrap_or(0);
            let title = gtk::Label::builder().label(label).use_underline(true).mnemonic_widget(widget).xalign(0.0).build();
            counter.attach(&title, 0, row, 1, 1);
            counter.attach(widget, 1, row, 1, 1);
        }
        frame(&gettext("Counter"), &counter)
    }

    /// Buttons, as the original: Reset All, Rename, Close.
    fn buttons(self: &Rc<Self>) -> gtk::Box {
        let reset = gtk::Button::with_mnemonic(&gettext("Reset _All"));
        reset.set_tooltip_text(Some("Ctrl+R"));
        let weak = Rc::downgrade(self);
        reset.connect_clicked(move |_| {
            if let Some(tool) = weak.upgrade() {
                tool.reset();
            }
        });
        self.rename.add_css_class("suggested-action");
        self.rename.set_tooltip_text(Some("F9"));
        let close = gtk::Button::with_mnemonic(&pgettext("button", "_Close"));
        close.set_tooltip_text(Some("Esc"));
        let window = self.window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        });
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.append(&reset);
        buttons.append(&gtk::Box::builder().hexpand(true).build());
        buttons.append(&self.rename);
        buttons.append(&close);
        buttons
    }

    /// `...` next to a mask: the placeholders by kind, as in the original.
    fn placeholder_button(self: &Rc<Self>, target: Target) -> gtk::MenuButton {
        let action = format!("rename.{}", target.action());
        let item = |menu: &gio::Menu, keyword: &str, inserted: &str, description: &str| {
            // Folder names may hold underscores, which are not mnemonics here.
            let label = format!("{keyword}  {}", description.replace('_', "__"));
            menu.append(Some(&label), Some(&gio::Action::print_detailed_name(&action, Some(&inserted.to_variant()))));
        };
        let menu = gio::Menu::new();

        let names = gio::Menu::new();
        item(&names, "[N]", "[N]", &gettext("Name"));
        item(&names, "[Nx]", "[N1]", &gettext("Character at position x"));
        item(&names, "[Nx:y]", "[N1:3]", &gettext("Characters from position x to y"));
        item(&names, "[A]", "[A]", &gettext("Complete filename with path and extension"));
        item(&names, "[Ax:y]", "[A1:3]", &gettext("Complete filename, char from pos x to y"));
        let parents = gio::Menu::new();
        let first_dir = self.files.borrow().first().map(|file| file.dir.clone()).unwrap_or_default();
        for (keyword, folder) in multi_rename::parent_placeholders(&first_dir) {
            item(&parents, &keyword, &keyword, &folder);
        }
        names.append_submenu(Some(&gettext("Parent folder(s)")), &parents);
        menu.append_submenu(Some(&gettext("Name")), &names);

        let extension = gio::Menu::new();
        item(&extension, "[E]", "[E]", &gettext("Extension"));
        item(&extension, "[Ex]", "[E1]", &gettext("Character at position x"));
        item(&extension, "[Ex:y]", "[E1:3]", &gettext("Characters from position x to y"));
        menu.append_submenu(Some(&gettext("Extension")), &extension);

        let counter = gio::Menu::new();
        item(&counter, "[C]", "[C]", &gettext("Counter"));
        item(&counter, "[G]", "[G]", &gettext("GUID"));
        menu.append_submenu(Some(&gettext("Counter")), &counter);

        let date = gio::Menu::new();
        for (keyword, description) in [
            ("[Y]", gettext("Year (2 digits)")),
            ("[YYYY]", gettext("Year (4 digits)")),
            ("[M]", gettext("Month")),
            ("[MM]", gettext("Month (2 digits)")),
            ("[MMM]", gettext("Month name (short, e.g., \"jan\")")),
            ("[MMMM]", gettext("Month name (long, e.g., \"january\")")),
            ("[D]", gettext("Day")),
            ("[DD]", gettext("Day (2 digits)")),
            ("[DDD]", gettext("Day of the week (short, e.g., \"mon\")")),
            ("[DDDD]", gettext("Day of the week (long, e.g., \"monday\")")),
            ("[YYYY]-[MM]-[DD]", gettext("Complete date")),
        ] {
            item(&date, keyword, keyword, &description);
        }
        menu.append_submenu(Some(&gettext("Date")), &date);

        let time = gio::Menu::new();
        for (keyword, description) in [
            ("[h]", gettext("Hour")),
            ("[hh]", gettext("Hour (2 digits)")),
            ("[n]", gettext("Minute")),
            ("[nn]", gettext("Minute (2 digits)")),
            ("[s]", gettext("Second")),
            ("[ss]", gettext("Second (2 digits)")),
            ("[hh]-[nn]-[ss]", gettext("Complete time")),
        ] {
            item(&time, keyword, keyword, &description);
        }
        menu.append_submenu(Some(&gettext("Time")), &time);

        let clear = gio::Menu::new();
        clear.append(Some(&gettext("Clear")), Some(&gio::Action::print_detailed_name(&action, Some(&"".to_variant()))));
        menu.append_section(None, &clear);

        gtk::MenuButton::builder().label("...").menu_model(&menu).tooltip_text(gettext("Insert a placeholder")).build()
    }

    fn connect(self: &Rc<Self>) {
        let actions = gio::SimpleActionGroup::new();
        for target in [Target::Name, Target::Extension] {
            let action = gio::SimpleAction::new(target.action(), Some(glib::VariantTy::STRING));
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, parameter| {
                if let (Some(tool), Some(text)) = (weak.upgrade(), parameter.and_then(glib::Variant::str)) {
                    tool.insert(target, text);
                }
            });
            actions.add_action(&action);
        }
        self.window.insert_action_group("rename", Some(&actions));

        let update = {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(tool) = weak.upgrade() {
                    tool.update();
                }
            }
        };
        for entry in [&self.name, &self.extension, &self.find, &self.replace] {
            let update = update.clone();
            entry.connect_changed(move |_| update());
        }
        for check in [&self.regex, &self.case_sensitive, &self.in_extension, &self.substitution, &self.only_first] {
            let update = update.clone();
            check.connect_toggled(move |_| update());
        }
        for dropdown in [&self.name_case, &self.extension_case, &self.width] {
            let update = update.clone();
            dropdown.connect_selected_notify(move |_| update());
        }
        for spin in [&self.start, &self.step] {
            let update = update.clone();
            spin.connect_value_changed(move |_| update());
        }
        // Groups to substitute exist only in regular expressions.
        self.regex.bind_property("active", &self.substitution, "sensitive").sync_create().build();

        let weak = Rc::downgrade(self);
        self.rename.connect_clicked(move |_| {
            if let Some(tool) = weak.upgrade() {
                tool.start_renaming();
            }
        });

        // Keys of the original's dialog: F9 rename, Esc close, Ctrl+R reset.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(tool) = weak.upgrade() else { return glib::Propagation::Proceed };
            let modifiers = state & gtk::accelerator_get_default_mod_mask();
            match (key, modifiers) {
                (gdk::Key::F9, gdk::ModifierType::NO_MODIFIER_MASK) => tool.start_renaming(),
                (gdk::Key::Escape, gdk::ModifierType::NO_MODIFIER_MASK) => tool.window.close(),
                (gdk::Key::r | gdk::Key::R, gdk::ModifierType::CONTROL_MASK) => tool.reset(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }

    /// Puts `text` at the end of the mask; positions in it (`[N1:3]`) come
    /// selected, to be typed over. Empty text clears the mask.
    fn insert(&self, target: Target, text: &str) {
        let entry = match target {
            Target::Name => &self.name,
            Target::Extension => &self.extension,
        };
        if text.is_empty() {
            entry.set_text("");
            entry.grab_focus();
            return;
        }
        let mut position = i32::try_from(entry.text().chars().count()).unwrap_or(i32::MAX);
        let start = position;
        entry.insert_text(text, &mut position);
        entry.grab_focus();
        if text.chars().any(|ch| ch.is_ascii_digit()) {
            // The letter after `[` stays, the positions up to `]` are selected.
            let length = i32::try_from(text.chars().count()).unwrap_or(0);
            entry.select_region(start + 2, start + length - 1);
        } else {
            entry.set_position(position);
        }
    }

    fn mask(&self) -> RenameMask {
        let case = |dropdown: &gtk::DropDown| {
            CaseStyle::ALL.get(usize::try_from(dropdown.selected()).unwrap_or(0)).copied().unwrap_or_default()
        };
        #[expect(clippy::cast_possible_truncation, reason = "whole numbers within the spin range")]
        let counter = Counter {
            start: self.start.value().round() as i64,
            step: self.step.value().round() as i64,
            width: usize::try_from(self.width.selected()).unwrap_or(0) + 1,
        };
        RenameMask {
            name: self.name.text().into(),
            extension: self.extension.text().into(),
            name_case: case(&self.name_case),
            extension_case: case(&self.extension_case),
            find_replace: FindReplace {
                find: self.find.text().into(),
                replace: self.replace.text().into(),
                regex: self.regex.is_active(),
                case_sensitive: self.case_sensitive.is_active(),
                in_extension: self.in_extension.is_active(),
                substitution: self.substitution.is_active() && self.regex.is_active(),
                only_first: self.only_first.is_active(),
            },
            counter,
        }
    }

    /// The new names of all files, or why there are none.
    fn new_names(&self) -> Result<Vec<String>, multi_rename::BadExpression> {
        let mask = self.mask();
        let compiled = mask.compile()?;
        Ok(self.files.borrow().iter().enumerate().map(|(index, file)| compiled.new_name(file, index, &self.tz)).collect())
    }

    fn fill_rows(&self) {
        let rows: Vec<RenameRow> = self
            .files
            .borrow()
            .iter()
            .map(|file| {
                glib::Object::builder()
                    .property("old-name", &file.name)
                    .property("new-name", &file.name)
                    .property("path", file.dir.to_string_lossy().as_ref())
                    .build()
            })
            .collect();
        self.rows.splice(0, self.rows.n_items(), &rows);
    }

    /// Shows the new names. A bad expression is shown instead of every
    /// name, its field red, and nothing can be renamed (as the original).
    fn update(&self) {
        let names = self.new_names();
        for index in 0..self.rows.n_items() {
            let Some(row) = self.rows.item(index).and_downcast::<RenameRow>() else { continue };
            let text = match &names {
                Ok(names) => names.get(usize::try_from(index).unwrap_or(usize::MAX)).cloned().unwrap_or_default(),
                Err(_) => gettext("Error in the regular expression"),
            };
            row.set_new_name(text);
        }
        match &names {
            Ok(_) => {
                self.find.remove_css_class("error");
                self.find.set_tooltip_text(None);
            }
            Err(err) => {
                self.find.add_css_class("error");
                self.find.set_tooltip_text(Some(&err.0));
            }
        }
        self.rename.set_sensitive(names.is_ok());
    }

    /// Reset All (Ctrl+R): the masks and options as on a fresh start.
    fn reset(&self) {
        let fresh = RenameMask::default();
        self.name.set_text(&fresh.name);
        self.extension.set_text(&fresh.extension);
        self.name_case.set_selected(0);
        self.extension_case.set_selected(0);
        self.find.set_text("");
        self.replace.set_text("");
        self.regex.set_active(fresh.find_replace.regex);
        self.case_sensitive.set_active(fresh.find_replace.case_sensitive);
        self.in_extension.set_active(fresh.find_replace.in_extension);
        self.substitution.set_active(fresh.find_replace.substitution);
        self.only_first.set_active(fresh.find_replace.only_first);
        #[expect(clippy::cast_precision_loss, reason = "small defaults")]
        {
            self.start.set_value(fresh.counter.start as f64);
            self.step.set_value(fresh.counter.step as f64);
        }
        self.width.set_selected(0);
        self.update();
    }

    /// Rename (F9): checks the names, asks about duplicates, renames.
    fn start_renaming(self: &Rc<Self>) {
        let Ok(names) = self.new_names() else { return };
        if let Some(bad) = names.iter().find(|name| name.is_empty() || name.contains('/') || *name == "." || *name == "..") {
            let message = fill(&gettext("Invalid file name: {name}"), &[("name", bad)]);
            dialogs::show_error(Some(&self.window), &message, "");
            return;
        }
        let duplicate = multi_rename::first_duplicate(&self.files.borrow(), &names);
        match duplicate {
            None => self.rename_files(&names, false),
            Some(path) => {
                // rsMulRenWarningDuplicate, rsMulRenAutoRename
                let message = format!(
                    "{}\n{}\n\n{}",
                    gettext("Warning, duplicate names!"),
                    path.display(),
                    gettext("Do auto-rename to \"name (1).ext\", \"name (2).ext\" etc.?")
                );
                let weak = Rc::downgrade(self);
                dialogs::yes_no(Some(&self.window), &message, move |yes| {
                    if let Some(tool) = weak.upgrade()
                        && yes
                    {
                        tool.rename_files(&names, true);
                    }
                });
            }
        }
    }

    fn rename_files(&self, names: &[String], auto_rename: bool) {
        let plan = multi_rename::plan(&self.files.borrow(), names, auto_rename);
        let failures = dc_ops::rename_many(&plan);
        // Renamed files are known by their new names from now on, as in the
        // original: the list shows them as old names.
        {
            let mut files = self.files.borrow_mut();
            for file in files.iter_mut() {
                let from = file.dir.join(&file.name);
                let Some(rename) = plan.iter().find(|rename| rename.from == from) else { continue };
                if failures.iter().any(|failure| failure.rename.from == from) {
                    continue;
                }
                if let Some(name) = rename.to.file_name() {
                    file.name = name.to_string_lossy().into_owned();
                }
            }
        }
        self.fill_rows();
        self.update();
        (self.renamed)();

        if !failures.is_empty() {
            let detail: Vec<String> = failures
                .iter()
                .take(10)
                .map(|failure| {
                    let name = |path: &Path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                    format!("{} → {}: {}", name(&failure.rename.from), name(&failure.rename.to), failure.error)
                })
                .collect();
            let mut detail = detail.join("\n");
            if failures.len() > 10 {
                detail.push_str("\n...");
            }
            dialogs::show_error(Some(&self.window), &gettext("Some files could not be renamed."), &detail);
        }
    }
}

/// A titled group of the dialog (`TGroupBox` of the original).
fn frame(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    child.set_margin_top(6);
    child.set_margin_bottom(6);
    child.set_margin_start(6);
    child.set_margin_end(6);
    gtk::Frame::builder().label(title).child(child).build()
}
