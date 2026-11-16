//! The "Load/Save" page: searches saved under a name, in
//! `~/.config/doubycmd/searches.toml` (`dc_core::find::templates`).

use std::cell::RefCell;
use std::path::PathBuf;

use dc_core::find::templates::{Template, Templates};
use dc_i18n::{fill, gettext};
use gtk::glib;
use gtk::prelude::*;

pub(super) struct Saved {
    pub page: gtk::Box,
    list: gtk::ListBox,
    summary: gtk::Label,
    pub load: gtk::Button,
    pub save: gtk::Button,
    pub save_with_start: gtk::Button,
    pub delete: gtk::Button,
    templates: RefCell<Templates>,
    file: PathBuf,
}

impl Saved {
    pub fn new() -> Self {
        let file = glib::user_config_dir().join("doubycmd").join("searches.toml");
        let this = Self {
            page: gtk::Box::new(gtk::Orientation::Vertical, 8),
            list: gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["boxed-list"]).build(),
            summary: gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).build(),
            load: gtk::Button::with_mnemonic(&gettext("L_oad")),
            save: gtk::Button::with_mnemonic(&gettext("S_ave")),
            save_with_start: gtk::Button::with_mnemonic(&gettext("Sa_ve with \"Start in directory\"")),
            delete: gtk::Button::with_mnemonic(&gettext("_Delete")),
            templates: RefCell::default(),
            file,
        };
        this.save_with_start.set_tooltip_text(Some(&gettext(
            "If saved then \"Start in directory\" will be restored when loading template. Use it if you want to fix searching to a certain directory",
        )));
        let title = gtk::Label::builder()
            .label(gettext("_Previous searches:"))
            .use_underline(true)
            .mnemonic_widget(&this.list)
            .xalign(0.0)
            .build();
        let scrolled = gtk::ScrolledWindow::builder().child(&this.list).vexpand(true).build();
        let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
        for button in [&this.load, &this.save, &this.save_with_start, &this.delete] {
            buttons.append(button);
        }
        let where_saved = gtk::Label::builder()
            .label(fill(&gettext("Saved in {path}"), &[("path", &this.file.display())]))
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        this.page.append(&title);
        this.page.append(&scrolled);
        this.page.append(&this.summary);
        this.page.append(&buttons);
        this.page.append(&where_saved);
        this.reload();
        this
    }

    /// Reads the file again (it may have been edited by hand).
    pub fn reload(&self) {
        match Templates::load(&self.file) {
            Ok(templates) => {
                *self.templates.borrow_mut() = templates;
                self.summary.set_text("");
            }
            Err(err) => self.summary.set_text(&format!("{err}\n{}", err.reason)),
        }
        self.fill_list(None);
    }

    fn fill_list(&self, select: Option<&str>) {
        self.list.remove_all();
        for template in &self.templates.borrow().list {
            let label = gtk::Label::builder().label(&template.name).xalign(0.0).margin_top(6).margin_bottom(6).margin_start(8).build();
            self.list.append(&label);
            if select == Some(template.name.as_str())
                && let Some(row) = self.list.last_child().and_downcast::<gtk::ListBoxRow>()
            {
                self.list.select_row(Some(&row));
            }
        }
        self.update_buttons();
    }

    /// Buttons that need a chosen search only with one.
    pub fn update_buttons(&self) {
        let chosen = self.chosen();
        self.load.set_sensitive(chosen.is_some());
        self.delete.set_sensitive(chosen.is_some());
        if let Some(template) = chosen {
            let query = &template.query;
            let masks = if query.masks.is_empty() { "*" } else { &query.masks };
            let text = if query.start.is_empty() {
                fill(&gettext("\"{masks}\""), &[("masks", &masks)])
            } else {
                fill(&gettext("\"{masks}\" -> \"{start}\""), &[("masks", &masks), ("start", &query.start)])
            };
            self.summary.set_text(&text);
        }
    }

    pub fn connect_selected(&self, changed: impl Fn() + 'static) {
        self.list.connect_selected_rows_changed(move |_| changed());
    }

    pub fn connect_activated(&self, activated: impl Fn() + 'static) {
        self.list.connect_row_activated(move |_, _| activated());
    }

    pub fn chosen(&self) -> Option<Template> {
        let index = usize::try_from(self.list.selected_row()?.index()).ok()?;
        self.templates.borrow().list.get(index).cloned()
    }

    /// Saves `template` (over one of the same name).
    ///
    /// # Errors
    /// The message when the file cannot be written.
    pub fn store(&self, template: Template) -> Result<(), String> {
        let name = template.name.clone();
        self.templates.borrow_mut().set(template);
        self.write()?;
        self.fill_list(Some(&name));
        Ok(())
    }

    /// Deletes the chosen search.
    ///
    /// # Errors
    /// The message when the file cannot be written.
    pub fn delete_chosen(&self) -> Result<(), String> {
        let Some(template) = self.chosen() else { return Ok(()) };
        self.templates.borrow_mut().remove(&template.name);
        self.write()?;
        self.fill_list(None);
        Ok(())
    }

    fn write(&self) -> Result<(), String> {
        self.templates.borrow().save(&self.file).map_err(|err| format!("{err}\n{}", err.reason))
    }
}
