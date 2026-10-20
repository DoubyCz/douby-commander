//! Renaming in place (Shift+F6, F2, `cm_RenameOnly`): an entry laid over
//! the name of the item under the cursor, in the list itself, no dialog.
//!
//! As the original (`ShowRenameFileEdit`): the whole name is selected,
//! Shift+F6 or F2 again select the name without the extension, the
//! extension, the whole name again. What happens on Enter, Esc and the
//! arrows is the panel's business (`panel_view.rs`).

use std::cell::{Cell, RefCell};

use gtk::graphene;
use gtk::prelude::*;

/// Part of the name selected, in the order Shift+F6 steps through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Whole,
    Name,
    Extension,
}

pub struct RenameEdit {
    overlay: gtk::Overlay,
    entry: gtk::Entry,
    part: Cell<Part>,
    /// The name when editing started.
    original: RefCell<String>,
}

impl RenameEdit {
    /// `content` is what the entry is laid over.
    pub fn new(content: &impl IsA<gtk::Widget>) -> Self {
        let overlay = gtk::Overlay::builder().child(content).vexpand(true).build();
        let entry = gtk::Entry::builder().halign(gtk::Align::Start).valign(gtk::Align::Start).visible(false).build();
        entry.add_css_class("dc-rename");
        overlay.add_overlay(&entry);
        Self { overlay, entry, part: Cell::new(Part::Whole), original: RefCell::default() }
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.overlay
    }

    pub fn entry(&self) -> &gtk::Entry {
        &self.entry
    }

    pub fn is_active(&self) -> bool {
        WidgetExt::is_visible(&self.entry)
    }

    /// The name when editing started.
    pub fn original(&self) -> String {
        self.original.borrow().clone()
    }

    pub fn text(&self) -> String {
        self.entry.text().to_string()
    }

    /// Shows the entry over `area` (in the overlay's coordinates) with
    /// `name`, all of it selected, and gives it the keyboard.
    pub fn show(&self, area: &graphene::Rect, name: &str) {
        #[expect(clippy::cast_possible_truncation, reason = "on-screen coordinates")]
        let (x, y, width, height) = (
            area.x().round() as i32,
            area.y().round() as i32,
            area.width().round() as i32,
            area.height().round() as i32,
        );
        self.entry.set_margin_start(x.max(0));
        self.entry.set_margin_top(y.max(0));
        self.entry.set_width_request(width.max(40));
        self.entry.set_height_request(height.max(1));
        self.original.replace(name.to_owned());
        self.entry.set_text(name);
        self.entry.set_visible(true);
        self.entry.grab_focus();
        self.part.set(Part::Whole);
        self.entry.select_region(0, -1);
    }

    pub fn hide(&self) {
        self.entry.set_visible(false);
    }

    /// Shift+F6 / F2 while editing: the next part of the name selected.
    /// A name without an extension (or a directory's) has only the whole.
    pub fn select_next_part(&self, has_extension: bool) {
        let next = match (self.part.get(), has_extension) {
            (_, false) | (Part::Extension, true) => Part::Whole,
            (Part::Whole, true) => Part::Name,
            (Part::Name, true) => Part::Extension,
        };
        self.part.set(next);
        let text = self.text();
        let length = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
        let dot = text.rfind('.').map_or(length, |dot| i32::try_from(text[..dot].chars().count()).unwrap_or(length));
        match next {
            Part::Whole => self.entry.select_region(0, -1),
            Part::Name => self.entry.select_region(0, dot),
            Part::Extension => self.entry.select_region(dot + 1, length),
        }
    }
}
