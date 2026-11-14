//! The path row above a panel's list: the path of its directory, which a
//! double click (or `cm_EditPath`) turns into an entry with two buttons at
//! its right end, what Enter will do ("Go", "Create (N) and go") and a
//! cross that cancels. What the panel does with the text is the panel's
//! business (`panel_view.rs`).

use dc_i18n::gettext;
use gtk::prelude::*;

pub struct PathEdit {
    /// The row: the label, the entry with its buttons laid over it.
    pub row: gtk::Overlay,
    pub label: gtk::Label,
    /// The entry with its buttons, shown while editing.
    pub edit: gtk::Box,
    pub entry: gtk::Entry,
    pub go: gtk::Button,
    /// In the button: a key when directories cannot be created there.
    go_key: gtk::Image,
    go_label: gtk::Label,
    pub cancel: gtk::Button,
}

impl PathEdit {
    pub fn new() -> Self {
        let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Start).build();
        label.add_css_class("dc-path");
        let entry = gtk::Entry::builder().hexpand(true).build();
        // The look (one frame around all) is `.dc-path-frame` in the base style.
        // Buttons not focusable: a click on one must not take the keyboard
        // from the entry, which would end the editing.
        let go_key = gtk::Image::builder().icon_name("dialog-password-symbolic").visible(false).build();
        let go_label = gtk::Label::new(None);
        let go_content = gtk::Box::builder().spacing(4).build();
        go_content.append(&go_key);
        go_content.append(&go_label);
        let go = gtk::Button::builder().child(&go_content).focusable(false).focus_on_click(false).build();
        let cancel = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text(gettext("Cancel editing, stay here (Esc)"))
            .focusable(false)
            .focus_on_click(false)
            .build();
        let edit = gtk::Box::builder().visible(false).valign(gtk::Align::Fill).css_classes(["dc-path-frame"]).build();
        edit.append(&entry);
        edit.append(&go);
        edit.append(&cancel);
        // The entry lies over the label: the row keeps the label's height
        // whichever is shown, the list below does not move.
        let row = gtk::Overlay::builder().child(&label).build();
        row.add_overlay(&edit);
        row.set_clip_overlay(&edit, true);
        Self { row, label, edit, entry, go, go_key, go_label, cancel }
    }

    pub fn is_active(&self) -> bool {
        self.edit.is_visible()
    }

    /// The entry instead of the label; the label stays, unseen, to keep
    /// the row's height.
    pub fn show(&self, text: &str) {
        self.entry.set_text(text);
        self.label.set_opacity(0.0);
        self.edit.set_visible(true);
        self.entry.grab_focus();
    }

    /// What the button says, in Pango markup; `locked` puts the key before
    /// it (no permission to do it).
    pub fn set_go(&self, markup: &str, locked: bool) {
        self.go_label.set_markup(markup);
        self.go_key.set_visible(locked);
    }

    pub fn hide(&self) {
        self.edit.set_visible(false);
        self.label.set_opacity(1.0);
    }
}
