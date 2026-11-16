//! The "Standard" page: where to search, which names, which text.

use dc_core::find::{DOCUMENT_TOOLS, ENCODINGS, FindQuery, tool_adds};
use dc_core::addons::{self, Hint};
use dc_i18n::{fill, gettext, ngettext, plural_count};
use gtk::prelude::*;

use super::{check, section, title};
use crate::history_entry::HistoryEntry;

/// Levels offered besides "all" and "current directory only".
const MAX_DEPTH: u32 = 100;

pub(super) struct Standard {
    pub page: gtk::Box,
    pub start: HistoryEntry,
    pub choose: gtk::Button,
    pub selected_only: gtk::CheckButton,
    follow_links: gtk::CheckButton,
    exclude_dirs: HistoryEntry,
    depth: gtk::DropDown,
    pub masks: HistoryEntry,
    in_archives: gtk::CheckButton,
    partial_names: gtk::CheckButton,
    name_regex: gtk::CheckButton,
    exclude_files: HistoryEntry,
    pub find_text: gtk::CheckButton,
    pub text: HistoryEntry,
    replace_check: gtk::CheckButton,
    replace: HistoryEntry,
    not_containing: gtk::CheckButton,
    case_sensitive: gtk::CheckButton,
    text_regex: gtk::CheckButton,
    hex: gtk::CheckButton,
    documents: gtk::CheckButton,
    encoding: gtk::DropDown,
    tools: gtk::Box,
    tools_text: gtk::Label,
    pub install_addons: gtk::Button,
}

fn depth_titles() -> Vec<String> {
    let mut titles = vec![gettext("all (unlimited depth)"), gettext("current directory only")];
    titles.extend((1..=MAX_DEPTH).map(|levels| {
        fill(&ngettext("{count} level", "{count} levels", plural_count(levels as usize)), &[("count", &levels)])
    }));
    titles
}

fn drop_down(titles: &[String]) -> gtk::DropDown {
    gtk::DropDown::from_strings(&titles.iter().map(String::as_str).collect::<Vec<_>>())
}

impl Standard {
    pub fn new() -> Self {
        let mut encodings = vec![gettext("Automatic (UTF-8, UTF-16 by its mark)")];
        encodings.extend(ENCODINGS.iter().map(|name| (*name).to_owned()));
        let this = Self {
            page: gtk::Box::new(gtk::Orientation::Vertical, 10),
            start: HistoryEntry::new(),
            choose: gtk::Button::builder().icon_name("folder-open-symbolic").tooltip_text(gettext("Choose a directory")).build(),
            selected_only: check(&gettext("Selected directories and _files")),
            follow_links: check(&gettext("Follow s_ymlinks")),
            exclude_dirs: HistoryEntry::new(),
            depth: drop_down(&depth_titles()),
            masks: HistoryEntry::new(),
            in_archives: check(&gettext("Search in _archives")),
            partial_names: check(&gettext("Searc_h for part of file name")),
            name_regex: check(&gettext("_Regular expression")),
            exclude_files: HistoryEntry::new(),
            find_text: check(&gettext("Find _text in file")),
            text: HistoryEntry::new(),
            replace_check: check(&gettext("Re_place by")),
            replace: HistoryEntry::new(),
            not_containing: check(&gettext("Find files N_OT containing the text")),
            case_sensitive: check(&gettext("Case sens_itive")),
            text_regex: check(&gettext("Reg_ular expression")),
            hex: check(&gettext("Hexadeci_mal")),
            documents: check(&gettext("Text of _documents")),
            encoding: drop_down(&encodings),
            tools: gtk::Box::new(gtk::Orientation::Vertical, 4),
            tools_text: gtk::Label::builder().xalign(0.0).wrap(true).build(),
            install_addons: gtk::Button::with_mnemonic(&gettext("Install _add-ons...")),
        };
        this.lay_out();
        this
    }

    fn lay_out(&self) {
        self.exclude_dirs.entry().set_tooltip_text(Some(&gettext(
            "Names of directories not to search, separated by \";\": a name (node_modules), a path under the start directory (build/cache) or a whole path (/proc); masks allowed.",
        )));
        self.masks.entry().set_tooltip_text(Some(&gettext("Names separated by \";\", masks with * and ? allowed")));
        self.exclude_files.entry().set_tooltip_text(Some(&gettext("Names not to find, separated by \";\"")));
        self.hex.set_tooltip_text(Some(&gettext("The text is bytes in hexadecimal, e.g. 4D 5A")));
        self.documents.set_tooltip_text(Some(&gettext(
            "Searches the text of documents, not their bytes: OpenDocument and Microsoft Office files, EPUB and FictionBook by itself; PDF, DjVu, RTF and old Word documents through programs of the system.",
        )));
        self.encoding.set_tooltip_text(Some(&gettext("Encoding of the files searched")));

        let directories = gtk::Grid::builder().row_spacing(4).column_spacing(12).build();
        let start_label = title(&gettext("Start in _directory"), self.start.entry());
        let start_checks = gtk::Box::builder().spacing(12).halign(gtk::Align::End).build();
        start_checks.append(&self.selected_only);
        start_checks.append(&self.follow_links);
        directories.attach(&start_label, 0, 0, 1, 1);
        directories.attach(&start_checks, 1, 0, 2, 1);
        let start_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        start_row.append(self.start.widget());
        start_row.append(&self.choose);
        directories.attach(&start_row, 0, 1, 3, 1);
        directories.attach(&title(&gettext("E_xclude subdirectories"), self.exclude_dirs.entry()), 0, 2, 2, 1);
        directories.attach(&title(&gettext("Search su_bdirectories:"), &self.depth), 2, 2, 1, 1);
        directories.attach(self.exclude_dirs.widget(), 0, 3, 2, 1);
        directories.attach(&self.depth, 2, 3, 1, 1);

        let files = gtk::Grid::builder().row_spacing(4).column_spacing(12).build();
        let mask_label = title(&gettext("File _mask"), self.masks.entry());
        mask_label.add_css_class("heading");
        let mask_checks = gtk::Box::builder().spacing(12).halign(gtk::Align::End).hexpand(true).build();
        for check in [&self.in_archives, &self.partial_names, &self.name_regex] {
            mask_checks.append(check);
        }
        files.attach(&mask_label, 0, 0, 1, 1);
        files.attach(&mask_checks, 1, 0, 1, 1);
        files.attach(self.masks.widget(), 0, 1, 2, 1);
        files.attach(&title(&gettext("_Exclude files"), self.exclude_files.entry()), 0, 2, 2, 1);
        files.attach(self.exclude_files.widget(), 0, 3, 2, 1);

        let content = gtk::Grid::builder().row_spacing(6).column_spacing(12).build();
        content.attach(&self.find_text, 0, 0, 1, 1);
        content.attach(self.text.widget(), 1, 0, 1, 1);
        content.attach(&self.replace_check, 0, 1, 1, 1);
        content.attach(self.replace.widget(), 1, 1, 1, 1);
        let options = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(16)
            .row_spacing(2)
            .max_children_per_line(4)
            .build();
        for check in [&self.not_containing, &self.case_sensitive, &self.text_regex, &self.hex, &self.documents] {
            options.append(check);
        }
        content.attach(&options, 0, 2, 2, 1);
        let encoding_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        encoding_row.append(&title(&gettext("Encodin_g:"), &self.encoding));
        encoding_row.append(&self.encoding);
        content.attach(&encoding_row, 0, 3, 2, 1);
        self.install_addons.set_halign(gtk::Align::Start);
        self.tools.append(&self.tools_text);
        self.tools.append(&self.install_addons);
        self.tools.set_visible(false);
        content.attach(&self.tools, 0, 4, 2, 1);

        self.page.append(&section(&gettext("Directories"), &directories));
        self.page.append(&section(&gettext("Files"), &files));
        self.page.append(&section(&gettext("Find in contents"), &content));
    }

    /// Fields enabled as the checks say; called after every change.
    pub fn update(&self) {
        let text = self.find_text.is_active();
        let hex = self.hex.is_active();
        for widget in [
            self.text.widget().upcast_ref::<gtk::Widget>(),
            self.not_containing.upcast_ref(),
            self.text_regex.upcast_ref(),
            self.hex.upcast_ref(),
            self.documents.upcast_ref(),
        ] {
            widget.set_sensitive(text);
        }
        self.case_sensitive.set_sensitive(text && !hex);
        self.encoding.set_sensitive(text && !hex);
        // Replacing: UTF-8 text in the files themselves.
        let replace = text && !hex && !self.documents.is_active() && !self.not_containing.is_active() && self.encoding.selected() == 0;
        self.replace_check.set_sensitive(replace);
        self.replace.widget().set_sensitive(replace && self.replace_check.is_active());
        self.partial_names.set_sensitive(!self.name_regex.is_active());
        self.update_tools(text && self.documents.is_active());
    }

    /// The programs for documents that are not installed, with the button
    /// that offers them.
    fn update_tools(&self, wanted: bool) {
        let hints = if wanted { missing_tools() } else { Vec::new() };
        self.tools.set_visible(!hints.is_empty());
        if hints.is_empty() {
            return;
        }
        let lines: Vec<String> =
            hints.iter().map(|hint| fill(&gettext("{program} - {adds}"), &[("program", &hint.program), ("adds", &hint.adds)])).collect();
        self.tools_text.set_text(&fill(
            &gettext("These documents need programs that are not installed:\n{list}"),
            &[("list", &lines.join("\n"))],
        ));
    }

    /// Calls `changed` whenever a check that enables others changes.
    pub fn connect_changed(&self, changed: impl Fn() + Clone + 'static) {
        for check in [&self.find_text, &self.replace_check, &self.hex, &self.documents, &self.not_containing, &self.name_regex] {
            let changed = changed.clone();
            check.connect_toggled(move |_| changed());
        }
        self.encoding.connect_selected_notify(move |_| changed());
    }

    /// Writes the fields into `query` (all but the start directory).
    pub fn fill(&self, query: &mut FindQuery) {
        query.exclude_dirs = self.exclude_dirs.text();
        query.depth = match self.depth.selected() {
            0 => None,
            level => Some(level - 1),
        };
        query.follow_links = self.follow_links.is_active();
        query.masks = self.masks.text();
        query.exclude_files = self.exclude_files.text();
        query.in_archives = self.in_archives.is_active();
        query.partial_names = self.partial_names.is_active();
        query.name_regex = self.name_regex.is_active();
        let text = &mut query.text;
        text.enabled = self.find_text.is_active();
        text.text = self.text.text();
        text.replace = (self.replace_check.is_sensitive() && self.replace_check.is_active()).then(|| self.replace.text());
        text.not_containing = self.not_containing.is_active();
        text.case_sensitive = self.case_sensitive.is_active();
        text.regex = self.text_regex.is_active();
        text.hex = self.hex.is_active();
        text.documents = self.documents.is_active();
        text.encoding = match self.encoding.selected() {
            0 => String::new(),
            index => ENCODINGS.get(index as usize - 1).copied().unwrap_or_default().to_owned(),
        };
    }

    /// Shows `query`; its start directory only when it has one. The text
    /// to find stays when the query has none (as the original keeps it).
    pub fn load(&self, query: &FindQuery) {
        if !query.start.is_empty() {
            self.start.set_text(&query.start);
        }
        self.exclude_dirs.set_text(&query.exclude_dirs);
        self.depth.set_selected(query.depth.map_or(0, |depth| depth.min(MAX_DEPTH) + 1));
        self.follow_links.set_active(query.follow_links);
        self.masks.set_text(&query.masks);
        self.exclude_files.set_text(&query.exclude_files);
        self.in_archives.set_active(query.in_archives);
        self.partial_names.set_active(query.partial_names);
        self.name_regex.set_active(query.name_regex);
        let text = &query.text;
        self.find_text.set_active(text.enabled);
        if !text.text.is_empty() {
            self.text.set_text(&text.text);
        }
        self.replace_check.set_active(text.replace.is_some());
        if let Some(replace) = &text.replace {
            self.replace.set_text(replace);
        }
        self.not_containing.set_active(text.not_containing);
        self.case_sensitive.set_active(text.case_sensitive);
        self.text_regex.set_active(text.regex);
        self.hex.set_active(text.hex);
        self.documents.set_active(text.documents);
        let encoding = ENCODINGS.iter().position(|name| *name == text.encoding).map_or(0, |index| index + 1);
        self.encoding.set_selected(u32::try_from(encoding).unwrap_or(0));
        self.update();
    }

    /// Texts used go into the histories.
    pub fn remember(&self) {
        for field in [&self.start, &self.exclude_dirs, &self.masks, &self.exclude_files] {
            field.remember();
        }
        if self.find_text.is_active() {
            self.text.remember();
            if self.replace_check.is_active() {
                self.replace.remember();
            }
        }
    }
}

/// The programs for documents that are not installed.
pub(super) fn missing_tools() -> Vec<Hint> {
    DOCUMENT_TOOLS
        .iter()
        .filter(|tool| !addons::is_installed(tool))
        .map(|tool| Hint { program: (*tool).to_owned(), adds: tool_adds(tool) })
        .collect()
}
