//! Add-ons of the whole application (`dc_core::addons`).
//!
//! Every window that has functions using add-ons (the main window, the
//! window to find files) has a [`Strip`] at its bottom. A function that
//! would do more with a missing program asks its strip to offer it; the
//! strip gathers what the functions of its window asked for and offers to
//! install it. The add-ons chosen go to the queue of the application,
//! which installs at once, one installation after another, through the
//! package manager of the distribution; meanwhile every strip shows how
//! the installation goes. The password for `sudo` is asked for in a window
//! of the application (none when sudo does not need one now).

use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dc_core::addons::{self, Distribution, Feature, Hint, InstallQueue, Request};
use dc_i18n::{fill, gettext, pgettext};
use gtk::glib;
use gtk::prelude::*;

use crate::dialogs;

/// How often a running installation is looked at.
const TICK: Duration = Duration::from_millis(200);
/// A successful installation stays on the strips so long.
const DONE_SHOWN: Duration = Duration::from_secs(8);

/// A function and the add-ons it can use, as offered.
pub struct Offer {
    pub feature: String,
    pub hints: Vec<Hint>,
}

/// Output of the package manager and how it ended, filled by the threads
/// that run it.
#[derive(Default)]
struct Output {
    text: String,
    /// `Some(success)` once it ended.
    ended: Option<bool>,
}

/// What the queue does now.
enum State {
    Idle,
    /// The password for sudo is being asked for.
    Waiting { packages: Vec<String> },
    Running { packages: Vec<String>, output: Arc<Mutex<Output>> },
    Finished { packages: Vec<String>, success: bool },
}

pub struct AddonsUi {
    distribution: Option<Distribution>,
    queue: RefCell<InstallQueue>,
    state: RefCell<State>,
    /// What the last installation wrote, for "Details".
    log: RefCell<String>,
    strips: RefCell<Vec<Weak<Strip>>>,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
    details: RefCell<Option<(gtk::Window, gtk::TextBuffer)>>,
    /// Windows the add-ons in the queue were chosen in.
    queued_from: RefCell<Vec<glib::WeakRef<gtk::Window>>>,
    /// Windows the installation now running (or just ended) was chosen
    /// in: only their strips show it, the others keep their own offers.
    shown_in: RefCell<Vec<glib::WeakRef<gtk::Window>>>,
    this: Weak<Self>,
}

impl AddonsUi {
    pub fn new() -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            distribution: addons::distribution(),
            queue: RefCell::default(),
            state: RefCell::new(State::Idle),
            log: RefCell::default(),
            strips: RefCell::default(),
            listeners: RefCell::default(),
            details: RefCell::default(),
            queued_from: RefCell::default(),
            shown_in: RefCell::default(),
            this: this.clone(),
        })
    }

    /// A new strip for the bottom of a window.
    pub fn strip(&self) -> Rc<Strip> {
        let strip = Strip::new(self.this.clone());
        self.strips.borrow_mut().push(Rc::downgrade(&strip));
        strip
    }

    /// `callback` runs after an installation: functions look again for
    /// the programs they use.
    pub fn connect_installed(&self, callback: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(callback));
    }

    fn strips(&self) -> Vec<Rc<Strip>> {
        self.strips.borrow().iter().filter_map(Weak::upgrade).collect()
    }

    fn refresh_strips(&self) {
        for strip in self.strips() {
            strip.refresh();
        }
    }

    /// Whether the installation is shown on the strip in `window`: only
    /// in the windows it was chosen in.
    fn shown_in(&self, window: Option<&gtk::Window>) -> bool {
        window.is_some_and(|window| self.shown_in.borrow().iter().any(|shown| shown.upgrade().as_ref() == Some(window)))
    }

    /// Whether `program` is queued or its package is being installed.
    fn is_pending(&self, program: &str, package: Option<&str>) -> bool {
        let running = match &*self.state.borrow() {
            State::Running { packages, .. } | State::Waiting { packages } => {
                package.is_some_and(|package| packages.iter().any(|running| running == package))
            }
            _ => false,
        };
        running || self.queue.borrow().contains(program)
    }

    /// Settings > Add-ons: everything every function can use.
    pub fn overview(&self, parent: Option<&gtk::Window>) {
        let offers: Vec<Offer> =
            Feature::ALL.into_iter().map(|feature| Offer { feature: feature.title(), hints: feature.add_ons() }).collect();
        self.choose(parent, &offers, false);
    }

    /// The window to choose add-ons: per function its add-ons with their
    /// packages and what they are for; the chosen ones get installed.
    /// Missing ones are checked when `check_missing`.
    fn choose(&self, parent: Option<&gtk::Window>, offers: &[Offer], check_missing: bool) {
        let window =
            gtk::Window::builder().title(gettext("Install add-ons")).modal(true).default_width(760).default_height(560).build();
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
        let install = gtk::Button::with_mnemonic(&gettext("_Install"));
        install.add_css_class("suggested-action");
        let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
        buttons.append(&cancel);
        buttons.append(&install);
        content.append(&buttons);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&install));

        let checks: Vec<gtk::CheckButton> = choices.iter().map(|(check, _)| check.clone()).collect();
        let chosen = Rc::new(move || -> Vec<Request> {
            choices.iter().filter(|(check, _)| check.is_active()).map(|(_, request)| request.clone()).collect()
        });
        let elevation = addons::elevation();
        let distribution = self.distribution;
        let update = {
            let (command, install, chosen) = (command.clone(), install.clone(), Rc::clone(&chosen));
            move || {
                let requests = chosen();
                let packages = addons::packages(&requests);
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
        for check in checks {
            let update = update.clone();
            check.connect_toggled(move |_| update());
        }
        cancel.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| window.close()
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
                    this.enqueue(parent.as_ref(), requests);
                }
            }
        ));
        dialogs::close_on_escape(&window);
        window.present();
    }

    /// Per function its add-ons: a check, the program, what it adds, the
    /// package and whether it is installed or on its way. With the checks
    /// that can be chosen and what they install.
    fn offer_rows(&self, offers: &[Offer], check_missing: bool) -> (gtk::Box, Vec<(gtk::CheckButton, Request)>) {
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let mut choices: Vec<(gtk::CheckButton, Request)> = Vec::new();
        for offer in offers {
            rows.append(&gtk::Label::builder().label(&offer.feature).xalign(0.0).css_classes(["heading"]).build());
            if offer.hints.is_empty() {
                rows.append(&gtk::Label::builder().label(gettext("Everything is installed.")).xalign(0.0).css_classes(["dim-label"]).build());
            }
            let grid = gtk::Grid::builder().column_spacing(12).row_spacing(6).build();
            for (index, hint) in offer.hints.iter().enumerate() {
                let row = i32::try_from(index).unwrap_or(0);
                let request = self.distribution.and_then(|distribution| Request::new(distribution, &offer.feature, hint));
                let installed = addons::is_installed(&hint.program);
                let pending = self.is_pending(&hint.program, request.as_ref().map(|request| request.package.as_str()));
                let check = gtk::CheckButton::builder().valign(gtk::Align::Start).build();
                let name = gtk::Label::builder().label(&hint.program).xalign(0.0).css_classes(["heading"]).build();
                check.set_child(Some(&name));
                let (state, sensitive, active) = match (&request, installed, pending) {
                    (_, true, _) => (gettext("installed"), false, true),
                    (None, false, _) => (gettext("package not known here"), false, false),
                    (Some(_), false, true) => (gettext("being installed"), false, true),
                    (Some(_), false, false) => (String::new(), true, check_missing),
                };
                check.set_sensitive(sensitive);
                check.set_active(active);
                let package =
                    request.as_ref().map_or_else(String::new, |request| fill(&gettext("package {package}"), &[("package", &request.package)]));
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

    /// Puts `requests` into the queue, which starts at once unless an
    /// installation runs (then it goes on with them afterwards).
    fn enqueue(&self, parent: Option<&gtk::Window>, requests: Vec<Request>) {
        self.queue.borrow_mut().add(requests);
        if let Some(parent) = parent {
            self.queued_from.borrow_mut().push(parent.downgrade());
        }
        if !matches!(*self.state.borrow(), State::Running { .. } | State::Waiting { .. }) {
            self.start_next(parent);
        }
        self.refresh_strips();
    }

    /// Installs everything queued: with sudo, without a password when it
    /// needs none now, else with the password asked for in a window.
    fn start_next(&self, parent: Option<&gtk::Window>) {
        let (Some(distribution), Some(elevation)) = (self.distribution, addons::elevation()) else { return };
        let requests = self.queue.borrow_mut().take_all();
        *self.shown_in.borrow_mut() = std::mem::take(&mut *self.queued_from.borrow_mut());
        let packages: Vec<String> = addons::packages(&requests).into_iter().map(str::to_owned).collect();
        if packages.is_empty() {
            return;
        }
        let package_refs: Vec<&str> = packages.iter().map(String::as_str).collect();
        let install = distribution.install_args(&package_refs);
        let job = Job { shown: format!("{elevation} {}", install.join(" ")), packages, install };
        if elevation != "sudo" {
            // pkexec and run0 ask through the agent of the desktop.
            let mut argv = vec![elevation.to_owned()];
            argv.extend(job.install.iter().cloned());
            self.run(job, argv, None);
        } else if sudo_ok(&["-n", "true"], None) {
            // Not `-v`: it asks for the password whenever some rule needs
            // one, also with NOPASSWD for everything else.
            let mut argv = vec!["sudo".to_owned(), "-n".to_owned()];
            argv.extend(job.install.iter().cloned());
            self.run(job, argv, None);
        } else {
            self.ask_password(parent, job, None);
        }
    }

    /// The window asking for the password of sudo; a wrong one asks again,
    /// cancelling drops the installation.
    fn ask_password(&self, parent: Option<&gtk::Window>, job: Job, error: Option<&str>) {
        *self.state.borrow_mut() = State::Waiting { packages: job.packages.clone() };
        self.refresh_strips();
        let parent = parent.cloned().or_else(|| self.shown_in.borrow().iter().find_map(glib::WeakRef::upgrade));
        let message = fill(
            &gettext("Installing add-ons ({packages}) needs the rights of the administrator. Password for sudo:"),
            &[("packages", &job.packages.join(", "))],
        );
        let wrong = gettext("Wrong password, try again.");
        let weak = self.this.clone();
        let asking = parent.clone();
        ask_secret(asking.as_ref(), &message, error, move |password| {
            let Some(this) = weak.upgrade() else { return };
            let Some(password) = password else {
                *this.state.borrow_mut() = State::Idle;
                this.shown_in.borrow_mut().clear();
                this.refresh_strips();
                return;
            };
            let weak = this.this.clone();
            glib::spawn_future_local(async move {
                let check = password.clone();
                let valid = gtk::gio::spawn_blocking(move || sudo_ok(&["-S", "-p", "", "-v"], Some(&check))).await.unwrap_or(false);
                let Some(this) = weak.upgrade() else { return };
                if valid {
                    let mut argv: Vec<String> = ["sudo", "-S", "-p", ""].map(str::to_owned).to_vec();
                    argv.extend(job.install.iter().cloned());
                    this.run(job, argv, Some(password));
                } else {
                    this.ask_password(parent.as_ref(), job, Some(&wrong));
                }
            });
        });
    }

    /// Runs `argv` with `password` on its input, reading what it writes;
    /// the strips show it.
    fn run(&self, job: Job, argv: Vec<String>, password: Option<String>) {
        let output = Arc::new(Mutex::new(Output { text: format!("$ {}\n", job.shown), ended: None }));
        let worker = Arc::clone(&output);
        std::thread::spawn(move || run_command(&argv, password, &worker));
        *self.state.borrow_mut() = State::Running { packages: job.packages, output };
        self.refresh_strips();
        let weak = self.this.clone();
        glib::timeout_add_local(TICK, move || {
            let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
            if this.tick() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }

    /// Shows how the installation goes; `false` once it ended.
    fn tick(&self) -> bool {
        let (text, ended, packages) = match &*self.state.borrow() {
            State::Running { packages, output } => {
                let output = output.lock().unwrap_or_else(PoisonError::into_inner);
                (output.text.clone(), output.ended, packages.clone())
            }
            _ => return false,
        };
        *self.log.borrow_mut() = text;
        self.update_details();
        let Some(success) = ended else {
            self.refresh_strips();
            return true;
        };
        *self.state.borrow_mut() = State::Finished { packages, success };
        for listener in self.listeners.borrow().iter() {
            listener();
        }
        if !self.queue.borrow().is_empty() {
            self.start_next(None);
        }
        self.refresh_strips();
        if success {
            let weak = self.this.clone();
            glib::timeout_add_local_once(DONE_SHOWN, move || {
                if let Some(this) = weak.upgrade() {
                    this.acknowledge(true);
                }
            });
        }
        false
    }

    /// A finished installation off the strips; with `only_success` a
    /// failed one stays until closed.
    fn acknowledge(&self, only_success: bool) {
        let finished = match *self.state.borrow() {
            State::Finished { success, .. } => success || !only_success,
            _ => false,
        };
        if finished {
            *self.state.borrow_mut() = State::Idle;
            self.shown_in.borrow_mut().clear();
        }
        self.refresh_strips();
    }

    /// "Details": everything the package manager wrote, growing while it runs.
    fn show_details(&self, parent: Option<&gtk::Window>) {
        if let Some((window, _)) = &*self.details.borrow() {
            window.present();
            return;
        }
        let buffer = gtk::TextBuffer::new(None);
        let view = gtk::TextView::builder().buffer(&buffer).editable(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).build();
        let scrolled = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
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
        content.append(&scrolled);
        content.append(&buttons);
        let window =
            gtk::Window::builder().title(gettext("Installing add-ons")).default_width(820).default_height(480).child(&content).build();
        window.set_transient_for(parent);
        close.connect_clicked(glib::clone!(
            #[weak]
            window,
            move |_| window.close()
        ));
        let weak = self.this.clone();
        window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                this.details.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        dialogs::close_on_escape(&window);
        *self.details.borrow_mut() = Some((window.clone(), buffer));
        self.update_details();
        window.present();
    }

    fn update_details(&self) {
        if let Some((_, buffer)) = &*self.details.borrow() {
            let log = as_terminal(&self.log.borrow());
            if buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).as_str() != log {
                buffer.set_text(&log);
            }
        }
    }
}

/// One installation: its packages, the package manager with its
/// arguments, and the command as shown.
struct Job {
    packages: Vec<String>,
    install: Vec<String>,
    shown: String,
}

/// Runs `argv`, `password` and a new line on its input, its output and
/// errors into `output` as they come, then how it ended.
fn run_command(argv: &[String], password: Option<String>, output: &Arc<Mutex<Output>>) {
    let child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(err) => {
            let mut output = output.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = writeln!(output.text, "{err}");
            output.ended = Some(false);
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take()
        && let Some(password) = password
    {
        let _ = stdin.write_all(format!("{password}\n").as_bytes());
    }
    let streams: [Option<Box<dyn Read + Send>>; 2] = [
        child.stdout.take().map(|out| Box::new(out) as Box<dyn Read + Send>),
        child.stderr.take().map(|err| Box::new(err) as Box<dyn Read + Send>),
    ];
    let readers: Vec<_> = streams
        .into_iter()
        .flatten()
        .map(|mut stream| {
            let output = Arc::clone(output);
            std::thread::spawn(move || {
                let mut block = [0_u8; 4096];
                while let Ok(read) = stream.read(&mut block) {
                    if read == 0 {
                        break;
                    }
                    output.lock().unwrap_or_else(PoisonError::into_inner).text.push_str(&String::from_utf8_lossy(&block[..read]));
                }
            })
        })
        .collect();
    let success = child.wait().is_ok_and(|status| status.success());
    for reader in readers {
        let _ = reader.join();
    }
    output.lock().unwrap_or_else(PoisonError::into_inner).ended = Some(success);
}

/// Whether `sudo` with `args` succeeds (`password` on its input).
fn sudo_ok(args: &[&str], password: Option<&str>) -> bool {
    let child = Command::new("sudo").args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let Ok(mut child) = child else { return false };
    if let Some(mut stdin) = child.stdin.take()
        && let Some(password) = password
    {
        let _ = stdin.write_all(format!("{password}\n").as_bytes());
    }
    child.wait().is_ok_and(|status| status.success())
}

/// `text` as a terminal shows it: a `\r` (a progress bar) returns to the
/// start of its line, what follows writes over it.
fn as_terminal(text: &str) -> String {
    text.split('\n')
        .map(|line| line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The last line written, `\r` (progress bars) counted as an end of line.
fn last_line(text: &str) -> &str {
    text.rsplit(['\n', '\r']).map(str::trim).find(|line| !line.is_empty()).unwrap_or_default()
}

/// The password window: `message` above the field, `error` (a wrong
/// password) under it; `on_answer` gets the password or `None`.
fn ask_secret(parent: Option<&gtk::Window>, message: &str, error: Option<&str>, on_answer: impl FnOnce(Option<String>) + 'static) {
    let dialog =
        gtk::Window::builder().title(gettext("Administrator password")).modal(true).resizable(false).default_width(440).build();
    dialog.set_transient_for(parent);
    let label = gtk::Label::builder().label(message).xalign(0.0).wrap(true).build();
    let entry = gtk::PasswordEntry::builder().show_peek_icon(true).activates_default(true).hexpand(true).build();
    let cancel = gtk::Button::with_mnemonic(&dialogs::cancel_label());
    let ok = gtk::Button::with_mnemonic(&dialogs::ok_label());
    ok.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).margin_top(8).build();
    buttons.append(&cancel);
    buttons.append(&ok);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    let icon = gtk::Image::builder().icon_name("dialog-password-symbolic").pixel_size(32).halign(gtk::Align::Start).build();
    content.append(&icon);
    content.append(&label);
    content.append(&entry);
    if let Some(error) = error {
        content.append(&gtk::Label::builder().label(error).xalign(0.0).css_classes(["error"]).build());
    }
    content.append(&buttons);
    dialog.set_child(Some(&content));
    dialog.set_default_widget(Some(&ok));
    dialogs::close_on_escape(&dialog);

    let on_answer = Rc::new(RefCell::new(Some(on_answer)));
    let answer = move |password: Option<String>| {
        if let Some(on_answer) = on_answer.borrow_mut().take() {
            on_answer(password);
        }
    };
    let answered = answer.clone();
    ok.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        entry,
        move |_| {
            let password = entry.text().to_string();
            entry.set_text("");
            answered(Some(password));
            dialog.close();
        }
    ));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| dialog.close()
    ));
    // Closed in any other way: cancelled (after OK nothing more happens).
    dialog.connect_close_request(move |_| {
        answer(None);
        glib::Propagation::Proceed
    });
    dialog.present();
    entry.grab_focus();
}

/// The strip at the bottom of a window: what the functions of the window
/// would like to have, and how an installation goes.
pub struct Strip {
    root: gtk::Box,
    spinner: gtk::Spinner,
    icon: gtk::Image,
    label: gtk::Label,
    install: gtk::Button,
    details: gtk::Button,
    close: gtk::Button,
    /// Add-ons the functions of the window asked for, with the function.
    offers: RefCell<Vec<(String, Hint)>>,
    /// Programs the user closed the offer of; not offered again here.
    dismissed: RefCell<Vec<String>>,
    addons: Weak<AddonsUi>,
}

impl Strip {
    fn new(addons: Weak<AddonsUi>) -> Rc<Self> {
        let root = gtk::Box::builder().spacing(8).margin_start(6).margin_end(4).visible(false).build();
        root.add_css_class("dc-addons-bar");
        let button = |label: &str| {
            let button = gtk::Button::with_mnemonic(label);
            button.set_focusable(false);
            button
        };
        let close = gtk::Button::builder().icon_name("window-close-symbolic").focusable(false).build();
        close.add_css_class("flat");
        let strip = Rc::new(Self {
            root,
            spinner: gtk::Spinner::new(),
            icon: gtk::Image::from_icon_name("system-software-install-symbolic"),
            label: gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build(),
            install: button(&gettext("_Install...")),
            details: button(&gettext("_Details")),
            close,
            offers: RefCell::default(),
            dismissed: RefCell::default(),
            addons,
        });
        for widget in [
            strip.spinner.upcast_ref::<gtk::Widget>(),
            strip.icon.upcast_ref(),
            strip.label.upcast_ref(),
            strip.install.upcast_ref(),
            strip.details.upcast_ref(),
            strip.close.upcast_ref(),
        ] {
            strip.root.append(widget);
        }
        strip.connect();
        strip
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.install.connect_clicked(move |button| {
            let Some(this) = weak.upgrade() else { return };
            let Some(addons) = this.addons.upgrade() else { return };
            let mut offers: Vec<Offer> = Vec::new();
            for (feature, hint) in this.offers.borrow().iter() {
                match offers.iter_mut().find(|offer| offer.feature == *feature) {
                    Some(offer) => offer.hints.push(hint.clone()),
                    None => offers.push(Offer { feature: feature.clone(), hints: vec![hint.clone()] }),
                }
            }
            addons.choose(window_of(button).as_ref(), &offers, true);
        });
        let weak = Rc::downgrade(self);
        self.details.connect_clicked(move |button| {
            if let Some(addons) = weak.upgrade().and_then(|this| this.addons.upgrade()) {
                addons.show_details(window_of(button).as_ref());
            }
        });
        let weak = Rc::downgrade(self);
        self.close.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let Some(addons) = this.addons.upgrade() else { return };
            if matches!(*addons.state.borrow(), State::Finished { .. }) && addons.shown_in(window_of(&this.root).as_ref()) {
                addons.acknowledge(false);
                return;
            }
            let closed: Vec<String> = this.offers.borrow_mut().drain(..).map(|(_, hint)| hint.program).collect();
            this.dismissed.borrow_mut().extend(closed);
            this.refresh();
        });
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// A function of the window would do more with the programs of
    /// `hints`: those missing are offered (unless closed before here).
    pub fn request(&self, feature: &str, hints: &[Hint]) {
        {
            let mut offers = self.offers.borrow_mut();
            let dismissed = self.dismissed.borrow();
            for hint in hints {
                let known = offers.iter().any(|(_, offered)| offered.program == hint.program);
                if !known && !dismissed.contains(&hint.program) && !addons::is_installed(&hint.program) {
                    offers.push((feature.to_owned(), hint.clone()));
                }
            }
        }
        self.refresh();
    }

    /// What the strip says now: an installation running or just ended,
    /// else the offer; `None` when there is nothing to say.
    fn content(&self, addons: &AddonsUi) -> Option<(String, Option<&'static str>, bool)> {
        // (text, icon or a spinner, an offer)
        let state = addons.state.borrow();
        // An installation chosen in another window: this one keeps its offer.
        let idle = State::Idle;
        let state = if addons.shown_in(window_of(&self.root).as_ref()) { &*state } else { &idle };
        Some(match state {
            State::Running { packages, output } => {
                let output = output.lock().unwrap_or_else(PoisonError::into_inner);
                let text = fill(
                    &gettext("Installing add-ons: {packages}  -  {doing}"),
                    &[("packages", &packages.join(", ")), ("doing", &last_line(&output.text))],
                );
                (text, None, false)
            }
            State::Waiting { packages } => (
                fill(
                    &gettext("Installing add-ons: {packages}  -  {doing}"),
                    &[("packages", &packages.join(", ")), ("doing", &gettext("waiting for the administrator password"))],
                ),
                None,
                false,
            ),
            State::Finished { packages, success: true } => {
                (fill(&gettext("Add-ons installed: {packages}"), &[("packages", &packages.join(", "))]), Some("emblem-ok-symbolic"), false)
            }
            State::Finished { packages, success: false } => (
                fill(&gettext("Installing add-ons failed: {packages}"), &[("packages", &packages.join(", "))]),
                Some("dialog-error-symbolic"),
                false,
            ),
            State::Idle => {
                let names: Vec<String> = self
                    .offers
                    .borrow()
                    .iter()
                    .filter(|(_, hint)| !addons.is_pending(&hint.program, None))
                    .map(|(_, hint)| fill(&gettext("{program} ({adds})"), &[("program", &hint.program), ("adds", &hint.adds)]))
                    .collect();
                if names.is_empty() {
                    return None;
                }
                let text = fill(&gettext("Add-ons would help here: {add_ons}"), &[("add_ons", &names.join(", "))]);
                (text, Some("system-software-install-symbolic"), true)
            }
        })
    }

    /// Shows the installation when one runs or just ended, else the offer.
    fn refresh(&self) {
        let Some(addons) = self.addons.upgrade() else { return };
        self.offers.borrow_mut().retain(|(_, hint)| !addons::is_installed(&hint.program));
        let Some((text, icon, offer)) = self.content(&addons) else {
            self.root.set_visible(false);
            return;
        };
        let busy = icon.is_none();
        self.root.set_visible(true);
        self.label.set_text(&text);
        self.label.set_tooltip_text(Some(&text));
        self.spinner.set_visible(busy);
        self.spinner.set_spinning(busy);
        self.icon.set_visible(!busy);
        self.icon.set_icon_name(icon);
        self.install.set_visible(offer);
        let waiting = matches!(*addons.state.borrow(), State::Waiting { .. }) && addons.shown_in(window_of(&self.root).as_ref());
        self.details.set_visible(!offer && !waiting);
        self.close.set_visible(!busy);
        self.close.set_tooltip_text(Some(&if offer { gettext("Do not offer these add-ons here") } else { gettext("Hide") }));
    }
}

/// The window `widget` is in.
pub fn window_of(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    widget.root().and_downcast::<gtk::Window>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_shows_the_last_line() {
        let text = "a\nb\r(1/2) instaluji catdoc   [---]  10%\r(1/2) instaluji catdoc   [###] 100%\n\n";
        assert_eq!(last_line(text), "(1/2) instaluji catdoc   [###] 100%");
        assert_eq!(last_line(""), "");
        assert_eq!(as_terminal("a\n(1) 10%\r(1) 40%\r(1) 70%\nb"), "a\n(1) 70%\nb");
    }
}
