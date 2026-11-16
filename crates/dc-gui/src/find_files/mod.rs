//! The window to find files (`cm_Search`, Alt+F7; `TfrmFindDlg` of the
//! original, without its plugins): pages Standard, Advanced, Load/Save and
//! Results, buttons Start, Cancel, Close, New search and Last search.
//!
//! One window for the whole application. Closing it only hides it: the
//! results stay, a search goes on. Opened again while nothing is searched,
//! it starts anew in the active panel's directory. The search runs in a
//! worker thread (`dc_core::find::Finder`); a timer takes what it found.

mod advanced;
mod results;
mod saved;
mod standard;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dc_core::addons::Feature;
use dc_core::find::templates::Template;
use dc_core::find::{FindQuery, Finder, Progress, Roots};
use dc_core::path_input;
use dc_i18n::{fill, gettext, pgettext};
use gtk::prelude::*;
use gtk::glib;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::addons::{AddonsUi, Strip};
use crate::dialogs;
use advanced::Advanced;
use results::Results;
use saved::Saved;
use standard::Standard;

/// How often the window takes what the search found.
const TICK: Duration = Duration::from_millis(100);

/// Pages in their order.
const STANDARD: u32 = 0;
const SAVED: u32 = 2;
const RESULTS: u32 = 3;

/// What the window asks the main window to do.
pub struct Actions {
    /// Show the item in the active panel, the cursor on it.
    pub go_to: Box<dyn Fn(&Path)>,
    /// Open the file in its application (F4).
    pub open: Box<dyn Fn(&Path)>,
    /// Show the items found in the active panel, under a directory.
    pub feed: Box<dyn Fn(Vec<PathBuf>, PathBuf)>,
}

/// A search going on.
struct Running {
    progress: Arc<Progress>,
    done: Arc<AtomicBool>,
    started: Instant,
    timer: glib::SourceId,
}

pub struct FindWindow {
    window: gtk::Window,
    notebook: gtk::Notebook,
    standard: Standard,
    advanced: Rc<Advanced>,
    saved: Saved,
    results: Results,
    start: gtk::Button,
    cancel: gtk::Button,
    close: gtk::Button,
    new_search: gtk::Button,
    last_search: gtk::Button,
    running: RefCell<Option<Running>>,
    /// The last search started, without its start directory.
    last_query: RefCell<Option<FindQuery>>,
    /// The directory of the active panel when opened.
    panel_dir: RefCell<PathBuf>,
    /// Its marked items then.
    selected: RefCell<Vec<PathBuf>>,
    actions: Actions,
    /// Add-ons the window would like, and installations, at its bottom.
    strip: Rc<Strip>,
}

/// A check box with a mnemonic.
fn check(label: &str) -> gtk::CheckButton {
    gtk::CheckButton::with_mnemonic(label)
}

/// A label with a mnemonic for `widget`.
fn title(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Label {
    gtk::Label::builder().label(label).use_underline(true).mnemonic_widget(widget).xalign(0.0).build()
}

/// A titled group of fields.
fn section(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    child.set_margin_top(6);
    child.set_margin_bottom(8);
    child.set_margin_start(10);
    child.set_margin_end(10);
    let label = gtk::Label::builder().label(title).css_classes(["heading"]).build();
    gtk::Frame::builder().label_widget(&label).child(child).build()
}

fn page(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    child.set_margin_top(10);
    child.set_margin_bottom(10);
    child.set_margin_start(10);
    child.set_margin_end(10);
    gtk::ScrolledWindow::builder().child(child).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build()
}

impl FindWindow {
    pub fn new(parent: Option<&gtk::Window>, actions: Actions, addons: &AddonsUi) -> Rc<Self> {
        let window = gtk::Window::builder().title(gettext("Find files")).default_width(1000).default_height(760).build();
        window.set_transient_for(parent);
        let this = Rc::new(Self {
            window,
            notebook: gtk::Notebook::new(),
            standard: Standard::new(),
            advanced: Rc::new(Advanced::new()),
            saved: Saved::new(),
            results: Results::new(),
            start: gtk::Button::with_mnemonic(&pgettext("button", "_Find")),
            cancel: gtk::Button::with_mnemonic(&pgettext("button", "C_ancel")),
            close: gtk::Button::with_mnemonic(&pgettext("button", "_Close")),
            new_search: gtk::Button::with_mnemonic(&gettext("_New search")),
            last_search: gtk::Button::with_mnemonic(&gettext("_Last search")),
            running: RefCell::default(),
            last_query: RefCell::default(),
            panel_dir: RefCell::default(),
            selected: RefCell::default(),
            actions,
            strip: addons.strip(),
        });
        this.lay_out();
        this.connect();
        this.connect_results();
        this.connect_saved();
        this.update_buttons();
        this
    }

    fn lay_out(&self) {
        for (child, label) in [
            (page(&self.standard.page).upcast::<gtk::Widget>(), gettext("_Standard")),
            (page(&self.advanced.page).upcast(), gettext("Ad_vanced")),
            (page(&self.saved.page).upcast(), gettext("Load/Sa_ve")),
            (page(&self.results.page).upcast(), gettext("_Results")),
        ] {
            let label = gtk::Label::builder().label(label).use_underline(true).build();
            self.notebook.append_page(&child, Some(&label));
        }
        self.notebook.set_hexpand(true);

        self.start.add_css_class("suggested-action");
        self.cancel.set_tooltip_text(Some("Esc"));
        let buttons = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).width_request(170).build();
        for button in [&self.start, &self.cancel, &self.close] {
            buttons.append(button);
        }
        buttons.append(&gtk::Box::builder().height_request(24).build());
        buttons.append(&self.new_search);
        buttons.append(&self.last_search);

        let content = gtk::Box::builder()
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&self.notebook);
        content.append(&buttons);
        content.set_vexpand(true);
        let window_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window_content.append(&content);
        window_content.append(self.strip.widget());
        self.window.set_child(Some(&window_content));
        self.window.set_default_widget(Some(&self.start));
    }

    fn connect(self: &Rc<Self>) {
        let on = |this: &Rc<Self>, action: fn(&Rc<Self>)| {
            let weak = Rc::downgrade(this);
            move || {
                if let Some(this) = weak.upgrade() {
                    action(&this);
                }
            }
        };
        let start = on(self, Self::start_search);
        self.start.connect_clicked(move |_| start());
        let cancel = on(self, Self::cancel_search);
        self.cancel.connect_clicked(move |_| cancel());
        let close = on(self, Self::hide);
        self.close.connect_clicked(move |_| close());
        let new_search = on(self, Self::new_search);
        self.new_search.connect_clicked(move |_| new_search());
        let last_search = on(self, Self::last_search);
        self.last_search.connect_clicked(move |_| last_search());

        let weak = Rc::downgrade(self);
        self.window.connect_map(move |_| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(this) = weak.upgrade() {
                    this.focus_mask();
                }
            });
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                this.cancel_search();
                this.hide();
            }
            glib::Propagation::Stop
        });
        // Esc stops a search, otherwise closes (`cm_CancelClose`).
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if key != gtk::gdk::Key::Escape || this.has_open_popover() {
                return glib::Propagation::Proceed;
            }
            if this.running.borrow().is_some() { this.cancel_search() } else { this.hide() }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.standard.connect_changed(move || {
            if let Some(this) = weak.upgrade() {
                this.standard_changed();
            }
        });
        self.advanced.connect();

        let weak = Rc::downgrade(self);
        self.standard.choose.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.choose_directory();
            }
        });
        let weak = Rc::downgrade(self);
        self.notebook.connect_switch_page(move |_, _, page| {
            if let Some(this) = weak.upgrade()
                && page == SAVED
            {
                this.saved.reload();
            }
        });
    }

    /// An open popover (a history, a calendar) takes Esc itself.
    fn has_open_popover(&self) -> bool {
        let Some(focus) = gtk::prelude::RootExt::focus(&self.window) else { return false };
        focus.ancestor(gtk::Popover::static_type()).is_some()
    }

    fn connect_results(self: &Rc<Self>) {
        let results = &self.results;
        let weak = Rc::downgrade(self);
        results.view.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.go_to(true);
            }
        });
        let weak = Rc::downgrade(self);
        results.go_to.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.go_to(true);
            }
        });
        let weak = Rc::downgrade(self);
        results.open.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.open();
            }
        });
        let weak = Rc::downgrade(self);
        results.remove.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.results.remove_selected();
            }
        });
        let weak = Rc::downgrade(self);
        results.feed.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.feed();
            }
        });
        // As the original: Space goes to the file and keeps the window,
        // Delete takes the item off the list; F4 opens as in the panels.
        // Before the list itself, which would select on Space.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if !(state & gtk::accelerator_get_default_mod_mask()).is_empty() {
                return glib::Propagation::Proceed;
            }
            match key {
                gtk::gdk::Key::space => this.go_to(false),
                gtk::gdk::Key::Delete | gtk::gdk::Key::KP_Delete => this.results.remove_selected(),
                gtk::gdk::Key::F4 => this.open(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        results.view.add_controller(keys);
    }

    fn connect_saved(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.saved.connect_selected(move || {
            if let Some(this) = weak.upgrade() {
                this.saved.update_buttons();
            }
        });
        let load = {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(this) = weak.upgrade()
                    && let Some(template) = this.saved.chosen()
                {
                    this.load_query(&template.query);
                    this.notebook.set_current_page(Some(STANDARD));
                    this.standard.masks.entry().grab_focus();
                }
            }
        };
        let activated = load.clone();
        self.saved.connect_activated(activated);
        self.saved.load.connect_clicked(move |_| load());
        for (button, with_start) in [(&self.saved.save, false), (&self.saved.save_with_start, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.save_search(with_start);
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.saved.delete.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let Some(template) = this.saved.chosen() else { return };
            let question = fill(&gettext("Delete the saved search \"{name}\"?"), &[("name", &template.name)]);
            let weak = Rc::downgrade(&this);
            dialogs::yes_no(Some(&this.window), &question, move |yes| {
                if let Some(this) = weak.upgrade()
                    && yes
                    && let Err(message) = this.saved.delete_chosen()
                {
                    dialogs::show_error(Some(&this.window), &message, "");
                }
            });
        });
    }

    /// Alt+F7: the window, a new search in `panel_dir` unless one runs.
    pub fn show(self: &Rc<Self>, panel_dir: PathBuf, selected: Vec<PathBuf>) {
        if self.running.borrow().is_none() {
            let query = FindQuery { start: panel_dir.display().to_string(), ..FindQuery::default() };
            self.load_query(&query);
            self.standard.selected_only.set_active(false);
            self.standard.selected_only.set_sensitive(!selected.is_empty());
            *self.panel_dir.borrow_mut() = panel_dir;
            *self.selected.borrow_mut() = selected;
            self.notebook.set_current_page(Some(STANDARD));
        }
        // A window being mapped picks a focus of its own, the mask gets it
        // once mapped (`connect`); a window shown already gets it now.
        if self.window.is_mapped() {
            self.focus_mask();
        }
        self.window.present();
    }

    /// A new search starts with the mask (the notebook turns to the page of
    /// the focused widget).
    fn focus_mask(&self) {
        if self.running.borrow().is_none() {
            self.notebook.set_current_page(Some(STANDARD));
            self.standard.masks.entry().grab_focus();
        }
    }

    /// Add-ons were installed: the hint about missing ones looks again.
    pub fn addons_installed(&self) {
        self.standard_changed();
    }

    /// A check of the Standard page changed: fields enabled as it says, the
    /// programs for documents offered on the strip when they matter.
    fn standard_changed(&self) {
        self.standard.update();
        if self.standard.wants_document_tools() {
            self.strip.request(&Feature::DocumentText.title(), &standard::missing_tools());
        }
    }

    fn hide(self: &Rc<Self>) {
        self.window.set_visible(false);
    }

    fn load_query(&self, query: &FindQuery) {
        self.standard.load(query);
        self.advanced.load(query);
    }

    /// The query as the pages show it.
    fn query(&self) -> FindQuery {
        let mut query = FindQuery { start: self.standard.start.text(), ..FindQuery::default() };
        self.standard.fill(&mut query);
        self.advanced.fill(&mut query);
        query
    }

    /// Start directories as typed (`;` between them, `~`, `$VAR`, relative
    /// to the panel), the panel's own when none.
    fn roots(&self, query: &FindQuery) -> Roots {
        if self.standard.selected_only.is_active() {
            return Roots::Selected(self.selected.borrow().clone());
        }
        let base = self.panel_dir.borrow().clone();
        let home = glib::home_dir();
        let dirs: Vec<PathBuf> = query
            .start
            .split(';')
            .filter_map(|part| path_input::expand(part, &base, &home, |name| std::env::var_os(name)))
            .collect();
        Roots::Directories(if dirs.is_empty() { vec![base] } else { dirs })
    }

    fn start_search(self: &Rc<Self>) {
        if self.running.borrow().is_some() {
            return;
        }
        let query = self.query();
        let roots = self.roots(&query);
        let finder = match Finder::new(&query, roots, Timestamp::now(), TimeZone::system()) {
            Ok(finder) => finder,
            Err(err) => {
                dialogs::show_error(Some(&self.window), &err.to_string(), "");
                return;
            }
        };
        self.standard.remember();
        *self.last_query.borrow_mut() = Some(FindQuery { start: String::new(), ..query.clone() });

        self.results.clear(query.duplicates.is_some());
        self.results.set_status(0, 0, "");
        self.notebook.set_current_page(Some(RESULTS));
        self.window.set_title(Some(&gettext("Find files - Scanning")));

        let progress = Arc::new(Progress::default());
        let done = Arc::new(AtomicBool::new(false));
        let (worker_progress, worker_done) = (Arc::clone(&progress), Arc::clone(&done));
        std::thread::spawn(move || {
            finder.run(&worker_progress);
            worker_done.store(true, Ordering::Release);
        });
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local(TICK, move || {
            let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
            if this.tick() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
        *self.running.borrow_mut() = Some(Running { progress, done, started: Instant::now(), timer });
        self.update_buttons();
        self.results.view.grab_focus();
    }

    /// Takes what was found; `false` once the search is over.
    fn tick(&self) -> bool {
        let (progress, done, started) = {
            let running = self.running.borrow();
            let Some(running) = running.as_ref() else { return false };
            (Arc::clone(&running.progress), running.done.load(Ordering::Acquire), running.started)
        };
        self.results.append(&progress.take_found());
        let current = if progress.is_comparing() {
            gettext("Comparing files for duplicates...")
        } else {
            fill(&gettext("Scanning: {path}"), &[("path", &progress.current().display())])
        };
        if !done {
            self.results.set_status(progress.scanned(), progress.found_count(), &current);
            return true;
        }
        let seconds = started.elapsed().as_secs_f64();
        let finished = if progress.is_cancelled() {
            fill(&gettext("Cancelled after {seconds} s"), &[("seconds", &format!("{seconds:.1}"))])
        } else {
            fill(&gettext("Finished in {seconds} s"), &[("seconds", &format!("{seconds:.1}"))])
        };
        self.results.set_status(progress.scanned(), progress.found_count(), &finished);
        // The timer ends by returning `false`; its id must not be removed again.
        self.running.borrow_mut().take();
        self.window.set_title(Some(&gettext("Find files - Finished")));
        self.update_buttons();
        self.results.focus();
        false
    }

    fn cancel_search(self: &Rc<Self>) {
        if let Some(running) = self.running.borrow().as_ref() {
            running.progress.cancel();
        }
    }

    /// Stops a search and empties the results; the fields stay (as the
    /// original keeps the filters).
    fn new_search(self: &Rc<Self>) {
        self.stop_now();
        self.results.clear(false);
        self.results.set_status(0, 0, "");
        self.window.set_title(Some(&gettext("Find files")));
        self.notebook.set_current_page(Some(STANDARD));
        self.standard.masks.entry().grab_focus();
    }

    /// Ends a search at once, without waiting for the worker.
    fn stop_now(&self) {
        if let Some(running) = self.running.borrow_mut().take() {
            running.progress.cancel();
            running.timer.remove();
        }
        self.update_buttons();
    }

    fn last_search(self: &Rc<Self>) {
        let query = self.last_query.borrow().clone();
        if let Some(query) = query {
            self.load_query(&query);
            self.notebook.set_current_page(Some(STANDARD));
            self.standard.masks.entry().grab_focus();
        }
    }

    fn update_buttons(&self) {
        let running = self.running.borrow().is_some();
        self.start.set_sensitive(!running);
        self.cancel.set_sensitive(running);
        self.close.set_sensitive(!running);
        self.new_search.set_sensitive(true);
        self.last_search.set_sensitive(!running && self.last_query.borrow().is_some());
        self.window.set_default_widget(Some(if running { &self.cancel } else { &self.start }));
    }

    /// The folder button: a directory chosen in the desktop's dialog.
    fn choose_directory(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title(gettext("Start in directory")).modal(true).build();
        let current = PathBuf::from(self.standard.start.text().split(';').next().unwrap_or_default().trim());
        if current.is_dir() {
            dialog.set_initial_folder(Some(&gtk::gio::File::for_path(&current)));
        }
        let weak = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), None::<&gtk::gio::Cancellable>, move |chosen| {
            if let (Some(this), Ok(folder)) = (weak.upgrade(), chosen)
                && let Some(path) = folder.path()
            {
                this.standard.start.set_text(&path.display().to_string());
            }
        });
    }

    /// Enter, a double click: the item in the active panel; `close` the window too.
    fn go_to(self: &Rc<Self>, close: bool) {
        let Some(row) = self.results.current() else { return };
        let path = row.path();
        if std::fs::symlink_metadata(&path).is_err() && !is_in_archive(&path) {
            let message = fill(&gettext("{path} is not there any more."), &[("path", &path.display())]);
            dialogs::show_error(Some(&self.window), &message, "");
            return;
        }
        if close {
            self.hide();
        }
        (self.actions.go_to)(&path);
    }

    fn open(self: &Rc<Self>) {
        if let Some(row) = self.results.current()
            && !row.is_dir()
        {
            (self.actions.open)(&row.path());
        }
    }

    /// "Feed to panel": everything found in the active panel.
    fn feed(self: &Rc<Self>) {
        let paths = self.results.paths();
        if paths.is_empty() {
            return;
        }
        let base = results::common_dir(&paths).unwrap_or_else(|| self.panel_dir.borrow().clone());
        self.stop_now();
        self.hide();
        (self.actions.feed)(paths, base);
    }

    fn save_search(self: &Rc<Self>, with_start: bool) {
        let name = self.saved.chosen().map(|template| template.name).unwrap_or_default();
        let weak = Rc::downgrade(self);
        dialogs::ask_text(Some(&self.window), &gettext("Save search template"), &gettext("_Template name:"), &name, move |name| {
            let Some(this) = weak.upgrade() else { return };
            let name = name.trim().to_owned();
            if name.is_empty() {
                return;
            }
            let mut query = this.query();
            if !with_start {
                query.start.clear();
            }
            if let Err(message) = this.saved.store(Template { name, query }) {
                dialogs::show_error(Some(&this.window), &message, "");
            }
        });
    }
}

/// A path inside an archive is not on disk as itself.
fn is_in_archive(path: &Path) -> bool {
    path.ancestors().skip(1).any(|ancestor| std::fs::metadata(ancestor).is_ok_and(|metadata| metadata.is_file()))
}
