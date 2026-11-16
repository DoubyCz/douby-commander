//! Add-ons of the whole application (`dc_core::addons`): the window that
//! offers the programs a function can use, the queue of those chosen for
//! later with its bar above the function keys, and the window that installs
//! them through the package manager of the distribution, in a terminal
//! where `sudo` asks for the password.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use dc_core::addons::{self, Distribution, Feature, Hint, InstallQueue, Request};
use dc_i18n::{fill, gettext, ngettext, pgettext, plural_count};
use gtk::glib;
use gtk::prelude::*;
use vte4::prelude::*;

use crate::{dialogs, panel_terminal};

/// A function and the add-ons it can use, as offered.
pub struct Offer {
    pub feature: String,
    pub hints: Vec<Hint>,
}

pub struct AddonsUi {
    queue: RefCell<InstallQueue>,
    file: PathBuf,
    distribution: Option<Distribution>,
    bar: gtk::Box,
    bar_label: gtk::Label,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
    /// An installation runs (one at a time).
    installing: Cell<bool>,
    this: Weak<Self>,
}

impl AddonsUi {
    pub fn new() -> Rc<Self> {
        let file = glib::user_config_dir().join("doubycmd").join("addons.toml");
        // A queue that cannot be read starts empty; the file stays until
        // the queue changes.
        let mut queue = InstallQueue::load(&file).unwrap_or_default();
        queue.retain_missing(addons::is_installed);
        let bar = gtk::Box::builder().spacing(8).margin_start(6).margin_end(4).visible(false).build();
        bar.add_css_class("dc-addons-bar");
        let bar_label = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let this = Rc::new_cyclic(|this| Self {
            queue: RefCell::new(queue),
            file,
            distribution: addons::distribution(),
            bar,
            bar_label,
            listeners: RefCell::default(),
            installing: Cell::new(false),
            this: this.clone(),
        });
        this.build_bar();
        this.update_bar();
        this
    }

    /// The bar of the queue, placed right above the function keys.
    pub fn bar(&self) -> &gtk::Box {
        &self.bar
    }

    /// `callback` runs after an installation: functions look again for
    /// the programs they use.
    pub fn connect_installed(&self, callback: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(callback));
    }

    fn build_bar(&self) {
        let icon = gtk::Image::from_icon_name("system-software-install-symbolic");
        let install = gtk::Button::with_mnemonic(&gettext("_Install all"));
        install.set_focusable(false);
        let clear = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text(gettext("Clear the queue of add-ons"))
            .focusable(false)
            .build();
        clear.add_css_class("flat");
        let weak = self.this.clone();
        install.connect_clicked(move |button| {
            if let Some(this) = weak.upgrade() {
                let requests = this.queue.borrow().items().to_vec();
                this.install(window_of(button).as_ref(), &requests);
            }
        });
        let weak = self.this.clone();
        clear.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.change_queue(InstallQueue::clear);
            }
        });
        self.bar.append(&icon);
        self.bar.append(&self.bar_label);
        self.bar.append(&install);
        self.bar.append(&clear);
    }

    fn update_bar(&self) {
        let queue = self.queue.borrow();
        self.bar.set_visible(!queue.is_empty());
        let packages = queue.packages();
        self.bar_label.set_text(&fill(
            &ngettext(
                "Add-on waiting to be installed: {packages}",
                "Add-ons waiting to be installed ({count}): {packages}",
                plural_count(packages.len()),
            ),
            &[("count", &packages.len()), ("packages", &packages.join(", "))],
        ));
        let lines: Vec<String> = queue
            .items()
            .iter()
            .map(|item| {
                fill(
                    &gettext("{package} ({program}) - {purpose}, for {feature}"),
                    &[("package", &item.package), ("program", &item.program), ("purpose", &item.purpose), ("feature", &item.feature)],
                )
            })
            .collect();
        self.bar.set_tooltip_text(Some(&lines.join("\n")));
    }

    /// Changes the queue, keeps it on disk and shows it.
    fn change_queue(&self, change: impl FnOnce(&mut InstallQueue)) {
        change(&mut self.queue.borrow_mut());
        if let Err(err) = self.queue.borrow().save(&self.file) {
            dialogs::show_error(window_of(&self.bar).as_ref(), &err.to_string(), &err.reason);
        }
        self.update_bar();
    }

    /// The button "Install add-ons" of a function: the add-ons of `offer`
    /// that are missing.
    pub fn offer(&self, parent: Option<&gtk::Window>, offer: Offer) {
        let hints: Vec<Hint> = offer.hints.into_iter().filter(|hint| !addons::is_installed(&hint.program)).collect();
        self.choose(parent, &[Offer { feature: offer.feature, hints }], true);
    }

    /// Settings > Add-ons: everything every function can use.
    pub fn overview(&self, parent: Option<&gtk::Window>) {
        let offers: Vec<Offer> =
            Feature::ALL.into_iter().map(|feature| Offer { feature: feature.title(), hints: feature.add_ons() }).collect();
        self.choose(parent, &offers, false);
    }

    /// The window to choose add-ons: per function its add-ons with their
    /// packages and what they are for; the chosen ones go to the queue or
    /// get installed. Missing ones are checked when `check_missing`.
    fn choose(&self, parent: Option<&gtk::Window>, offers: &[Offer], check_missing: bool) {
        let window = gtk::Window::builder().title(gettext("Install add-ons")).modal(true).default_width(760).default_height(600).build();
        window.set_transient_for(parent);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(14)
            .margin_end(14)
            .build();
        let about = match self.distribution {
            Some(distribution) => fill(
                &gettext("Add-ons are programs from the packages of your distribution, {distribution}. Douby Commander uses them once they are installed; what each adds is next to it."),
                &[("distribution", &distribution.name())],
            ),
            None => gettext("Add-ons are programs from the packages of your distribution. The packages of this distribution are not known here, install the programs with its package manager."),
        };
        content.append(&gtk::Label::builder().label(about).xalign(0.0).wrap(true).build());

        let (rows, choices) = self.offer_rows(offers, check_missing);
        let scrolled = gtk::ScrolledWindow::builder().child(&rows).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        content.append(&scrolled);

        let command = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).css_classes(["monospace", "dim-label"]).build();
        content.append(&command);
        let cancel = gtk::Button::with_mnemonic(&dialogs::cancel_label());
        let enqueue = gtk::Button::with_mnemonic(&gettext("Add to _queue"));
        let install = gtk::Button::with_mnemonic(&gettext("Install _now"));
        install.add_css_class("suggested-action");
        let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
        for button in [&cancel, &enqueue, &install] {
            buttons.append(button);
        }
        content.append(&buttons);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&install));

        let choices = Rc::new(choices);
        let chosen = {
            let choices = Rc::clone(&choices);
            move || -> Vec<Request> {
                choices.iter().filter(|(check, _)| check.is_active()).map(|(_, request)| request.clone()).collect()
            }
        };
        let elevation = addons::elevation();
        let distribution = self.distribution;
        let update = {
            let (command, enqueue, install, chosen) = (command.clone(), enqueue.clone(), install.clone(), chosen.clone());
            move || {
                let requests = chosen();
                let packages = addons::packages(&requests);
                enqueue.set_sensitive(!packages.is_empty());
                install.set_sensitive(!packages.is_empty() && elevation.is_some());
                install.set_tooltip_text(elevation.is_none().then(|| gettext("Neither sudo, run0 nor pkexec is installed.")).as_deref());
                let text = match (distribution, elevation) {
                    (Some(distribution), Some(elevation)) if !packages.is_empty() => {
                        format!("{elevation} {}", distribution.install_args(&packages).join(" "))
                    }
                    _ => String::new(),
                };
                command.set_text(&text);
                command.set_visible(!text.is_empty());
            }
        };
        update();
        for (check, _) in choices.iter() {
            let update = update.clone();
            check.connect_toggled(move |_| update());
        }
        cancel.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| window.close()
        ));
        let weak = self.this.clone();
        let chosen_now = chosen.clone();
        enqueue.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    let requests = chosen_now();
                    this.change_queue(|queue| queue.add(requests));
                }
                window.close();
            }
        ));
        let weak = self.this.clone();
        install.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| {
                let requests = chosen();
                let parent = window.transient_for();
                window.close();
                if let Some(this) = weak.upgrade() {
                    this.install(parent.as_ref(), &requests);
                }
            }
        ));
        dialogs::close_on_escape(&window);
        window.present();
    }

    /// Per function its add-ons: a check, the program, what it adds, the
    /// package and whether it is installed or queued. With the checks that
    /// can be chosen and what they install.
    fn offer_rows(&self, offers: &[Offer], check_missing: bool) -> (gtk::Box, Vec<(gtk::CheckButton, Request)>) {
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 14);
    let mut choices: Vec<(gtk::CheckButton, Request)> = Vec::new();
    for offer in offers {
        let heading = gtk::Label::builder().label(&offer.feature).xalign(0.0).css_classes(["heading"]).build();
        rows.append(&heading);
        if offer.hints.is_empty() {
            rows.append(&gtk::Label::builder().label(gettext("Everything is installed.")).xalign(0.0).css_classes(["dim-label"]).build());
        }
        let grid = gtk::Grid::builder().column_spacing(12).row_spacing(6).build();
        for (index, hint) in offer.hints.iter().enumerate() {
            let row = i32::try_from(index).unwrap_or(0);
            let request = self.distribution.and_then(|distribution| Request::new(distribution, &offer.feature, hint));
            let installed = addons::is_installed(&hint.program);
            let queued = self.queue.borrow().contains(&hint.program);
            let check = gtk::CheckButton::builder().valign(gtk::Align::Start).build();
            let name = gtk::Label::builder().label(&hint.program).xalign(0.0).valign(gtk::Align::Start).css_classes(["heading"]).build();
            check.set_child(Some(&name));
            let (state, sensitive, active) = match (&request, installed, queued) {
                (_, true, _) => (gettext("installed"), false, true),
                (None, false, _) => (gettext("package not known here"), false, false),
                (Some(_), false, true) => (gettext("in the queue"), false, true),
                (Some(_), false, false) => (String::new(), true, check_missing),
            };
            check.set_sensitive(sensitive);
            check.set_active(active);
            let package = request.as_ref().map_or_else(String::new, |request| {
                fill(&gettext("package {package}"), &[("package", &request.package)])
            });
            let details = gtk::Box::new(gtk::Orientation::Vertical, 2);
            details.set_hexpand(true);
            details.append(&gtk::Label::builder().label(&hint.adds).xalign(0.0).wrap(true).build());
            let small = [package, state].into_iter().filter(|text| !text.is_empty()).collect::<Vec<_>>().join(", ");
            if !small.is_empty() {
                details.append(&gtk::Label::builder().label(small).xalign(0.0).css_classes(["dim-label", "caption"]).build());
            }
            grid.attach(&check, 0, row, 1, 1);
            grid.attach(&details, 1, row, 1, 1);
            if let Some(request) = request
                && sensitive
            {
                choices.push((check, request));
            }
        }
        rows.append(&grid);
    }
        (rows, choices)
    }

    /// Installs the packages of `requests` in a window with a terminal:
    /// the package manager runs there through `sudo` (or `run0`,
    /// `pkexec`), which asks for the password. Afterwards the queue keeps
    /// only what is still missing and the functions look again.
    pub fn install(&self, parent: Option<&gtk::Window>, requests: &[Request]) {
        let (Some(distribution), Some(elevation)) = (self.distribution, addons::elevation()) else { return };
        let packages: Vec<String> = addons::packages(requests).into_iter().map(str::to_owned).collect();
        if packages.is_empty() || self.installing.replace(true) {
            return;
        }
        let package_refs: Vec<&str> = packages.iter().map(String::as_str).collect();
        let mut argv = vec![elevation.to_owned()];
        argv.extend(distribution.install_args(&package_refs));

        let (window, terminal, status) = install_window(parent, &packages);
        // The command first, as one would type it.
        terminal.feed(format!("$ {}\r\n", argv.join(" ")).as_bytes());
        let child = Rc::new(Cell::new(None::<glib::Pid>));
        let weak = self.this.clone();
        // Outside the macro, where xgettext finds them.
        let (done_text, failed_text) =
            (gettext("Done, the add-ons are installed."), gettext("The installation did not finish, see the terminal above."));
        terminal.connect_child_exited(glib::clone!(
            #[weak]
            status,
            #[strong]
            child,
            move |_, wait_status| {
                child.set(None);
                let done = glib::spawn_check_wait_status(wait_status).is_ok();
                status.set_text(if done { &done_text } else { &failed_text });
                if let Some(this) = weak.upgrade() {
                    this.installed();
                }
            }
        ));
        // Closed while it runs: the package manager is stopped.
        let weak = self.this.clone();
        let running = Rc::clone(&child);
        window.connect_close_request(move |_| {
            if let Some(pid) = running.take() {
                let _ = rustix::process::kill_process(
                    rustix::process::Pid::from_raw(pid.0).expect("a child has a positive pid"),
                    rustix::process::Signal::TERM,
                );
            }
            if let Some(this) = weak.upgrade() {
                this.installing.set(false);
            }
            glib::Propagation::Proceed
        });
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let weak = self.this.clone();
        terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            None,
            &argv_refs,
            &[],
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            None::<&gtk::gio::Cancellable>,
            glib::clone!(
                #[weak]
                status,
                move |result| match result {
                    Ok(pid) => child.set(Some(pid)),
                    Err(err) => {
                        status.set_text(&err.to_string());
                        if let Some(this) = weak.upgrade() {
                            this.installing.set(false);
                        }
                    }
                }
            ),
        );
        window.present();
        terminal.grab_focus();
    }

    /// After an installation: the queue without what is there now, the
    /// functions told.
    fn installed(&self) {
        self.installing.set(false);
        self.change_queue(|queue| queue.retain_missing(addons::is_installed));
        for listener in self.listeners.borrow().iter() {
            listener();
        }
    }
}

/// The window of an installation: what is installed, the terminal it
/// runs in, a line of how it went and Close.
fn install_window(parent: Option<&gtk::Window>, packages: &[String]) -> (gtk::Window, vte4::Terminal, gtk::Label) {
    let window = gtk::Window::builder().title(gettext("Installing add-ons")).default_width(860).default_height(520).build();
    window.set_transient_for(parent);
    let what = gtk::Label::builder()
        .label(fill(&gettext("Installing: {packages}"), &[("packages", &packages.join(", "))]))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["heading"])
        .build();
    let terminal = vte4::Terminal::builder().vexpand(true).hexpand(true).build();
    panel_terminal::follow_theme(&terminal);
    let status = gtk::Label::builder()
        .label(gettext("The package manager asks for your password in the terminal (Ctrl+C stops it)."))
        .xalign(0.0)
        .wrap(true)
        .build();
    let close = gtk::Button::with_mnemonic(&pgettext("button", "_Close"));
    let buttons = gtk::Box::builder().halign(gtk::Align::End).build();
    buttons.append(&close);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&what);
    let frame = gtk::Frame::builder().child(&terminal).build();
    content.append(&frame);
    content.append(&status);
    content.append(&buttons);
    window.set_child(Some(&content));
    close.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| window.close()
    ));
    (window, terminal, status)
}

/// The window `widget` is in.
pub fn window_of(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    widget.root().and_downcast::<gtk::Window>()
}
