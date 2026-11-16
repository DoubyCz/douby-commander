//! A text field with what was written in it before under an arrow next to
//! it (`TComboBoxWithDelItems` of the original): a click puts an earlier
//! text back, newest first.

use std::cell::RefCell;
use std::rc::Rc;

use dc_i18n::gettext;
use gtk::glib;
use gtk::prelude::*;

/// Texts kept per field.
const KEPT: usize = 25;

#[derive(Clone)]
pub struct HistoryEntry {
    row: gtk::Box,
    entry: gtk::Entry,
    button: gtk::MenuButton,
    list: gtk::Box,
    popover: gtk::Popover,
    history: Rc<RefCell<Vec<String>>>,
}

impl HistoryEntry {
    pub fn new() -> Self {
        let entry = gtk::Entry::builder().hexpand(true).activates_default(true).build();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(360)
            .build();
        let popover = gtk::Popover::builder().child(&scrolled).build();
        let button = gtk::MenuButton::builder()
            .popover(&popover)
            .tooltip_text(gettext("Earlier texts"))
            .sensitive(false)
            .can_focus(false)
            .build();
        let row = gtk::Box::builder().css_classes(["linked"]).hexpand(true).build();
        row.append(&entry);
        row.append(&button);
        Self { row, entry, button, list, popover, history: Rc::default() }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.row
    }

    pub fn entry(&self) -> &gtk::Entry {
        &self.entry
    }

    pub fn text(&self) -> String {
        self.entry.text().to_string()
    }

    pub fn set_text(&self, text: &str) {
        self.entry.set_text(text);
    }

    /// Puts the current text first in the history (an empty one not).
    pub fn remember(&self) {
        let text = self.text();
        if text.trim().is_empty() {
            return;
        }
        {
            let mut history = self.history.borrow_mut();
            history.retain(|earlier| *earlier != text);
            history.insert(0, text);
            history.truncate(KEPT);
        }
        self.fill();
    }

    fn fill(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for text in self.history.borrow().iter() {
            let item = gtk::Button::builder().label(text).build();
            item.add_css_class("flat");
            if let Some(label) = item.child().and_downcast::<gtk::Label>() {
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_max_width_chars(70);
            }
            item.connect_clicked(glib::clone!(
                #[weak(rename_to = entry)]
                self.entry,
                #[weak(rename_to = popover)]
                self.popover,
                move |item| {
                    entry.set_text(&item.label().unwrap_or_default());
                    popover.popdown();
                    entry.grab_focus();
                    entry.select_region(0, -1);
                }
            ));
            self.list.append(&item);
        }
        self.button.set_sensitive(!self.history.borrow().is_empty());
    }
}
