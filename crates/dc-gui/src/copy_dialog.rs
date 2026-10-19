//! Copy and move dialog (`TfrmCopyDlg` of the original with the options of
//! `TFileSystemCopyMoveOperationOptionsUI`).
//!
//! Start runs the operation at once, "Add To Queue #N" (F2) puts it into
//! the last used queue, "▾" offers a new queue and queues 1 to 5. F5 and F6
//! in the dialog step the selection through parts of the target (whole,
//! name, name with extension, extension, path).

use std::cell::Cell;
use std::rc::Rc;

use dc_i18n::{NC_, fill, gettext, pgettext};
use dc_ops::{DirectoryExists, FileExists, FollowLinks, PropertyError, QueueId, TransferMode, TransferOptions};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::dialogs;

/// What the dialog was opened with.
pub struct Request {
    pub mode: TransferMode,
    /// `rsMsgCpSel` and friends, made by the caller.
    pub message: String,
    pub target: String,
    pub last_queue: u32,
    pub new_queue: QueueId,
}

/// The confirmed dialog.
pub struct Confirmed {
    pub target: String,
    pub options: TransferOptions,
    pub queue: QueueId,
}

const FILE_EXISTS: [((&str, &str), FileExists); 4] = [
    (NC_("when file exists", "Ask"), FileExists::Ask),
    (NC_("when file exists", "Overwrite"), FileExists::Overwrite),
    (NC_("when file exists", "Overwrite Older"), FileExists::OverwriteOlder),
    (NC_("when file exists", "Skip"), FileExists::Skip),
];

const DIRECTORY_EXISTS: [((&str, &str), DirectoryExists); 3] = [
    (NC_("when directory exists", "Ask"), DirectoryExists::Ask),
    (NC_("when directory exists", "Merge"), DirectoryExists::Merge),
    (NC_("when directory exists", "Skip"), DirectoryExists::Skip),
];

const PROPERTY_ERROR: [((&str, &str), PropertyError); 3] = [
    (NC_("when cannot set property", "Ask"), PropertyError::Ask),
    (NC_("when cannot set property", "Don't set anymore"), PropertyError::DontSetAnymore),
    (NC_("when cannot set property", "Ignore errors"), PropertyError::Ignore),
];

pub fn show(parent: Option<&gtk::Window>, request: &Request, on_ok: impl FnOnce(Confirmed) + 'static) {
    let copying = request.mode == TransferMode::Copy;
    // rsDlgCp, rsDlgMv
    let title = if copying { gettext("Copy file(s)") } else { gettext("Move file(s)") };
    let dialog = gtk::Window::builder().title(title).modal(true).resizable(false).default_width(560).build();
    dialog.set_transient_for(parent);

    let label = gtk::Label::builder().label(&request.message).xalign(0.0).wrap(true).max_width_chars(70).build();
    let entry = gtk::Entry::builder().text(&request.target).activates_default(true).hexpand(true).build();
    label.set_mnemonic_widget(Some(&entry));

    let options = Options::build(request.mode);

    let show_options = gtk::ToggleButton::with_mnemonic(&pgettext("button", "O_ptions"));
    let add_to_queue = gtk::Button::with_mnemonic(&fill(
        &gettext("A_dd To Queue #{number} ({key})"),
        &[("number", &request.last_queue), ("key", &"F2")],
    ));
    let queue_menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let popover = gtk::Popover::builder().child(&queue_menu).build();
    // Without a label the button shows the theme's own arrow ("▾" of the original).
    let more_queues = gtk::MenuButton::builder().popover(&popover).build();
    let cancel = gtk::Button::with_mnemonic(&dialogs::cancel_label());
    // rsDlgOpStart
    let start = gtk::Button::with_mnemonic(&pgettext("button", "_Start"));
    start.add_css_class("suggested-action");

    let spacer = gtk::Box::builder().hexpand(true).build();
    let buttons = row(&[
        show_options.upcast_ref(),
        add_to_queue.upcast_ref(),
        more_queues.upcast_ref(),
        spacer.upcast_ref(),
        cancel.upcast_ref(),
        start.upcast_ref(),
    ]);
    buttons.set_margin_top(12);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    for widget in [label.upcast_ref::<gtk::Widget>(), entry.upcast_ref(), buttons.upcast_ref(), options.widget.upcast_ref()] {
        content.append(widget);
    }
    dialog.set_child(Some(&content));
    dialog.set_default_widget(Some(&start));

    show_options.bind_property("active", &options.widget, "visible").sync_create().build();
    // Nothing to do without a target.
    for button in [&start, &add_to_queue] {
        entry.bind_property("text", button, "sensitive").transform_to(|_, text: String| Some(!text.trim().is_empty())).sync_create().build();
    }

    // Several buttons, one decision.
    let on_ok = Cell::new(Some(on_ok));
    let options = Rc::new(options);
    let choose = Rc::new(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        entry,
        #[strong]
        options,
        move |queue: QueueId| {
            let target = entry.text().to_string();
            if target.trim().is_empty() {
                return;
            }
            let options = options.read();
            dialog.close();
            if let Some(on_ok) = on_ok.take() {
                on_ok(Confirmed { target, options, queue });
            }
        }
    ));
    let connect = |button: &gtk::Button, queue: QueueId| {
        let choose = Rc::clone(&choose);
        button.connect_clicked(move |_| choose(queue));
    };
    connect(&start, QueueId::Free);
    connect(&add_to_queue, QueueId::Numbered(request.last_queue));
    let new_queue = gtk::Button::builder().label(gettext("New queue")).css_classes(["flat"]).build();
    connect(&new_queue, request.new_queue);
    queue_menu.append(&new_queue);
    for number in 1..=5 {
        let queue = QueueId::Numbered(number);
        let button = gtk::Button::builder().label(queue.to_string()).css_classes(["flat"]).build();
        connect(&button, queue);
        queue_menu.append(&button);
    }
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| dialog.close()
    ));
    let last_queue = request.last_queue;
    connect_keys(&dialog, &entry, move || choose(QueueId::Numbered(last_queue)));

    dialog.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}

fn row(widgets: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::builder().spacing(8).build();
    for widget in widgets {
        row.append(*widget);
    }
    row
}

/// Keys of the dialog: Esc, F2 (`cm_AddToQueue`), F5 and F6
/// (`cm_ToggleSelectionInName`).
fn connect_keys(dialog: &gtk::Window, entry: &gtk::Entry, add_to_queue: impl Fn() + 'static) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let step = Cell::new(SelectionStep::Whole);
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        entry,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, state| {
            if !(state & gtk::accelerator_get_default_mod_mask()).is_empty() {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::Escape => dialog.close(),
                gdk::Key::F2 => add_to_queue(),
                gdk::Key::F5 | gdk::Key::F6 => step_selection(&entry, &step),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        }
    ));
    dialog.add_controller(keys);
}

/// Parts of the target text the selection steps through, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionStep {
    Whole,
    NameNoExtension,
    NameWithExtension,
    Extension,
    Path,
}

impl SelectionStep {
    fn next(self) -> Self {
        match self {
            Self::Whole => Self::NameNoExtension,
            Self::NameNoExtension => Self::NameWithExtension,
            Self::NameWithExtension => Self::Extension,
            Self::Extension => Self::Path,
            Self::Path => Self::Whole,
        }
    }

    /// Start and length in characters within `text`.
    fn range(self, text: &str) -> (usize, usize) {
        let full = text.chars().count();
        let path = text.rfind('/').map_or(0, |slash| text[..=slash].chars().count());
        let name = &text[text.rfind('/').map_or(0, |slash| slash + 1)..];
        let extension = name.rfind('.').map_or(0, |dot| name[dot..].chars().count());
        match self {
            Self::Whole => (0, full),
            Self::NameNoExtension => (path, full - path - extension),
            Self::NameWithExtension => (path, full - path),
            Self::Extension => (full - extension + 1, extension.saturating_sub(1)),
            Self::Path => (0, path.saturating_sub(1)),
        }
    }
}

/// `cm_ToggleSelectionInName`: the next part that is not empty and differs
/// from what is selected now.
fn step_selection(entry: &gtk::Entry, step: &Cell<SelectionStep>) {
    let text = entry.text();
    if text.is_empty() {
        return;
    }
    let current = entry.selection_bounds().map(|(start, end)| (start, end - start));
    for _ in 0..5 {
        step.set(step.get().next());
        let (start, length) = step.get().range(&text);
        let range = (i32::try_from(start).unwrap_or(0), i32::try_from(length).unwrap_or(0));
        if length > 0 && Some(range) != current {
            entry.select_region(range.0, range.0 + range.1);
            return;
        }
    }
}

/// The options panel, hidden until "Options" is pressed.
struct Options {
    widget: gtk::Grid,
    file_exists: gtk::DropDown,
    directory_exists: gtk::DropDown,
    property_error: gtk::DropDown,
    check_free_space: gtk::CheckButton,
    follow_links: TriState,
    copy_attributes: gtk::CheckButton,
    drop_read_only: gtk::CheckButton,
    copy_time: gtk::CheckButton,
    copy_ownership: gtk::CheckButton,
    reserve_space: gtk::CheckButton,
    copy_on_write: gtk::CheckButton,
}

impl Options {
    fn build(mode: TransferMode) -> Self {
        let defaults = TransferOptions::default();
        let copying = mode == TransferMode::Copy;
        let grid = gtk::Grid::builder().row_spacing(4).column_spacing(12).margin_top(12).visible(false).build();

        let drop_down = |items: &[&(&str, &str)], selected: usize| {
            let labels: Vec<String> = items.iter().map(|(context, text)| pgettext(*context, *text)).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let drop_down = gtk::DropDown::from_strings(&labels);
            drop_down.set_selected(u32::try_from(selected).unwrap_or(0));
            drop_down
        };
        let position = |found: Option<usize>| found.unwrap_or(0);
        let file_exists = drop_down(
            &FILE_EXISTS.iter().map(|(text, _)| text).collect::<Vec<_>>(),
            position(FILE_EXISTS.iter().position(|(_, value)| *value == defaults.file_exists)),
        );
        let directory_exists = drop_down(
            &DIRECTORY_EXISTS.iter().map(|(text, _)| text).collect::<Vec<_>>(),
            position(DIRECTORY_EXISTS.iter().position(|(_, value)| *value == defaults.directory_exists)),
        );
        let property_error = drop_down(
            &PROPERTY_ERROR.iter().map(|(text, _)| text).collect::<Vec<_>>(),
            position(PROPERTY_ERROR.iter().position(|(_, value)| *value == defaults.property_error)),
        );
        let rows: [(String, &gtk::DropDown); 3] = [
            (gettext("When _file exists"), &file_exists),
            (gettext("When dir_ectory exists"), &directory_exists),
            (gettext("When ca_nnot set property"), &property_error),
        ];
        let mut row = 0;
        for (caption, widget) in rows {
            let label = gtk::Label::builder().label(caption).use_underline(true).xalign(0.0).mnemonic_widget(widget).build();
            grid.attach(&label, 0, row, 1, 1);
            grid.attach(widget, 1, row, 1, 1);
            row += 1;
        }

        let check = |caption: String, active: bool| {
            gtk::CheckButton::builder().label(caption).use_underline(true).active(active).build()
        };
        let check_free_space = check(gettext("C_heck free space"), defaults.check_free_space);
        let follow_links = TriState::new(&gettext("Fo_llow links"), defaults.follow_links);
        let copy_attributes = check(gettext("Cop_y attributes"), defaults.copy_attributes);
        let drop_read_only = check(gettext("Drop readonly fla_g"), defaults.drop_read_only);
        let copy_time = check(gettext("Copy d_ate/time"), defaults.copy_time);
        let copy_ownership = check(gettext("Copy o_wnership"), defaults.copy_ownership);
        let reserve_space = check(gettext("_Reserve space"), defaults.reserve_space);
        let copy_on_write = check(gettext("Copy on write"), defaults.copy_on_write);

        // Reserving space checks it anyway; "drop readonly" is part of copying attributes.
        reserve_space.bind_property("active", &check_free_space, "sensitive").invert_boolean().sync_create().build();
        copy_attributes.bind_property("active", &drop_read_only, "sensitive").sync_create().build();
        // Moving never follows links and tries copy on write always.
        follow_links.button.set_visible(copying);
        drop_read_only.set_visible(copying);
        copy_on_write.set_visible(copying);

        let checks = [
            &check_free_space,
            &follow_links.button,
            &copy_attributes,
            &drop_read_only,
            &copy_time,
            &copy_ownership,
            &reserve_space,
            &copy_on_write,
        ];
        let (mut column, mut check_row) = (0, row);
        for button in checks {
            grid.attach(button, column, check_row, 1, 1);
            column = 1 - column;
            if column == 0 {
                check_row += 1;
            }
        }

        Self {
            widget: grid,
            file_exists,
            directory_exists,
            property_error,
            check_free_space,
            follow_links,
            copy_attributes,
            drop_read_only,
            copy_time,
            copy_ownership,
            reserve_space,
            copy_on_write,
        }
    }

    fn read(&self) -> TransferOptions {
        let pick = |drop_down: &gtk::DropDown| usize::try_from(drop_down.selected()).unwrap_or(0);
        let reserve_space = self.reserve_space.is_active();
        TransferOptions {
            file_exists: FILE_EXISTS.get(pick(&self.file_exists)).map_or(FileExists::Ask, |(_, value)| *value),
            directory_exists: DIRECTORY_EXISTS.get(pick(&self.directory_exists)).map_or(DirectoryExists::Ask, |(_, value)| *value),
            property_error: PROPERTY_ERROR.get(pick(&self.property_error)).map_or(PropertyError::Ask, |(_, value)| *value),
            follow_links: self.follow_links.value.get(),
            check_free_space: reserve_space || self.check_free_space.is_active(),
            reserve_space,
            copy_attributes: self.copy_attributes.is_active(),
            drop_read_only: self.copy_attributes.is_active() && self.drop_read_only.is_active(),
            copy_time: self.copy_time.is_active(),
            copy_ownership: self.copy_ownership.is_active(),
            copy_on_write: self.copy_on_write.is_active(),
        }
    }
}

/// A check box with a third, "grayed" state meaning Ask, as the LCL check
/// boxes with `AllowGrayed`: clicking goes unchecked, checked, grayed.
struct TriState {
    button: gtk::CheckButton,
    value: Rc<Cell<FollowLinks>>,
}

impl TriState {
    fn new(caption: &str, value: FollowLinks) -> Self {
        let button = gtk::CheckButton::builder().label(caption).use_underline(true).build();
        let value = Rc::new(Cell::new(value));
        show_tri_state(&button, value.get());
        let shown = Rc::clone(&value);
        // Showing a state sets `active`, which toggles again.
        let updating = Cell::new(false);
        button.connect_toggled(move |button| {
            if updating.get() {
                return;
            }
            let next = match shown.get() {
                FollowLinks::No => FollowLinks::Yes,
                FollowLinks::Yes => FollowLinks::Ask,
                FollowLinks::Ask => FollowLinks::No,
            };
            shown.set(next);
            updating.set(true);
            show_tri_state(button, next);
            updating.set(false);
        });
        Self { button, value }
    }
}

fn show_tri_state(button: &gtk::CheckButton, value: FollowLinks) {
    button.set_inconsistent(value == FollowLinks::Ask);
    button.set_active(value == FollowLinks::Yes);
}
