//! What the quick view tells of a file it has no viewer for, from the
//! programs of the system: a picture by a thumbnailer of the desktop, what
//! the installed tools find out, and the missing ones with the command
//! that installs them. With nothing installed it stays the type and size.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dc_core::preview::pages::PageRenderer;
use dc_core::preview::thumbnailer::Thumbnailer;
use dc_core::preview::tools::{self, Distribution, Hint, InfoTool, Section};
use dc_i18n::{fill, gettext};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::launch;
use crate::pixels::Pixels;

/// A thumbnailer that does not finish by then is stopped.
const PICTURE_TIMEOUT: Duration = Duration::from_secs(5);
/// More of the output of a program is not read, the program is stopped.
const MAX_OUTPUT: usize = 1 << 20;
/// The size thumbnailers are asked for, the largest of the specification.
const PICTURE_SIZE: u32 = 1024;
/// Lines of a list beyond this are left out (banks with thousands of
/// instruments).
const MAX_LIST_ITEMS: usize = 1000;

pub struct Details {
    pub picture: Option<Pixels>,
    pub sections: Vec<Section>,
    pub hints: Vec<Hint>,
    /// The command that installs the missing programs, where it is known.
    pub install: Option<String>,
}

/// The page of a file without a viewer: its icon (or picture), name and
/// type, then what the programs told and which would tell more.
pub struct InfoPage {
    root: gtk::ScrolledWindow,
    icon: gtk::Image,
    /// A thumbnailer's picture instead of the icon.
    picture: gtk::Picture,
    name: gtk::Label,
    text: gtk::Label,
    details: gtk::Box,
}

impl InfoPage {
    pub fn new() -> Self {
        let icon = gtk::Image::builder().pixel_size(96).build();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::ScaleDown)
            .can_shrink(true)
            .height_request(320)
            .visible(false)
            .build();
        // Long names are shortened in the middle (the extension stays), the
        // whole name is in the tooltip and in the header: a label that wraps
        // cannot break a name without spaces and would widen the window.
        let name = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).build();
        name.add_css_class("title-3");
        let text = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .build();
        text.add_css_class("dim-label");
        let details = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .margin_start(24)
            .margin_end(24)
            .margin_top(24)
            .margin_bottom(24)
            .build();
        content.append(&icon);
        content.append(&picture);
        content.append(&name);
        content.append(&text);
        content.append(&details);
        let root = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&content).build();
        Self { root, icon, picture, name, text, details }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Only the icon, name and `text`: what was told before is gone.
    pub fn show(&self, icon: &str, name: &str, text: &str) {
        self.icon.set_icon_name((!icon.is_empty()).then_some(icon));
        self.icon.set_visible(true);
        self.picture.set_visible(false);
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        self.name.set_text(name);
        self.name.set_tooltip_text((!name.is_empty()).then_some(name));
        self.text.set_text(text);
        self.root.vadjustment().set_value(0.0);
    }

    pub fn add_details(&self, details: Details) {
        if let Some(picture) = details.picture {
            self.picture.set_paintable(Some(&picture.texture()));
            self.picture.set_visible(true);
            self.icon.set_visible(false);
        }
        for section in &details.sections {
            self.details.append(&section_widget(section));
        }
        if !details.hints.is_empty() {
            self.details.append(&hints_widget(&details.hints, details.install.as_deref()));
        }
    }
}

/// A part the programs told: a title and names with values, or lines.
fn section_widget(section: &Section) -> gtk::Box {
    let (title, body): (&str, gtk::Widget) = match section {
        Section::Fields { title, fields } => {
            let grid = gtk::Grid::builder().column_spacing(12).row_spacing(4).build();
            for (row, (name, value)) in (0..).zip(fields) {
                let name = gtk::Label::builder().label(name).xalign(1.0).yalign(0.0).build();
                name.add_css_class("dim-label");
                let value = gtk::Label::builder()
                    .label(value)
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .hexpand(true)
                    .build();
                grid.attach(&name, 0, row, 1, 1);
                grid.attach(&value, 1, row, 1, 1);
            }
            (title, grid.upcast())
        }
        Section::List { title, items } => {
            let mut text = items.iter().take(MAX_LIST_ITEMS).map(String::as_str).collect::<Vec<_>>().join("\n");
            if items.len() > MAX_LIST_ITEMS {
                text.push_str("\n…");
            }
            let list = gtk::Label::builder().label(text).xalign(0.0).build();
            list.add_css_class("monospace");
            (title, list.upcast())
        }
        Section::Text { title, text } => {
            let text = gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build();
            (title, text.upcast())
        }
    };
    titled(title, &body)
}

/// Programs that would tell more, with the command that installs them on
/// this distribution and a button that copies it.
fn hints_widget(hints: &[Hint], install: Option<&str>) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for hint in hints {
        let line = gtk::Label::builder()
            .label(fill(&gettext("{program} - {adds}"), &[("program", &hint.program), ("adds", &hint.adds)]))
            .xalign(0.0)
            .wrap(true)
            .build();
        body.append(&line);
    }
    if let Some(install) = install {
        let command = gtk::Label::builder()
            .label(install)
            .xalign(0.0)
            .hexpand(true)
            .selectable(true)
            .focusable(false)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        command.add_css_class("monospace");
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text(gettext("Copy the command"))
            .focusable(false)
            .focus_on_click(false)
            .valign(gtk::Align::Center)
            .build();
        copy.add_css_class("flat");
        let text = install.to_owned();
        copy.connect_clicked(move |button| button.clipboard().set_text(&text));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("dc-command");
        row.append(&command);
        row.append(&copy);
        body.append(&row);
    }
    titled(&gettext("For a better preview install"), &body.upcast())
}

fn titled(title: &str, body: &gtk::Widget) -> gtk::Box {
    let heading = gtk::Label::builder().label(title).xalign(0.0).build();
    heading.add_css_class("heading");
    let part = gtk::Box::new(gtk::Orientation::Vertical, 6);
    part.append(&heading);
    part.append(body);
    part
}

/// Everything the installed programs tell about `path` of `mime`; in a
/// worker thread, it runs them. Raising `cancel` stops the one running,
/// the rest do not start (the quick view went on to another file).
pub fn find(path: &Path, mime: &str, cancel: &AtomicBool) -> Details {
    let mut hints = Vec::new();
    if let Some(renderer) = PageRenderer::for_mime(mime)
        && !renderer_installed(renderer)
    {
        hints.push(Hint { program: renderer.render_program().to_owned(), adds: renderer.adds() });
    }
    let picture = picture(path, mime, &mut hints, cancel);
    let mut sections = Vec::new();
    for tool in InfoTool::ALL.into_iter().filter(|tool| tool.applies(mime)) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(program) = launch::find_program(tool.program()) else {
            hints.push(Hint { program: tool.program().to_owned(), adds: tool.adds() });
            continue;
        };
        let languages = glib::language_names();
        let language = tools::exiftool_language(languages.iter().map(glib::GString::as_str));
        let args = tool.args(path.as_os_str(), mime, language);
        let output = run(&program, &args, tool.input(), tool.timeout(), MAX_OUTPUT, cancel);
        sections.extend(output.map(|output| tool.parse(&String::from_utf8_lossy(&output))).unwrap_or_default());
    }
    let install = install_command(&hints);
    Details { picture, sections, hints, install }
}

/// Whether both programs of `renderer` are installed.
pub fn renderer_installed(renderer: PageRenderer) -> bool {
    launch::find_program(renderer.info_program()).is_some() && launch::find_program(renderer.render_program()).is_some()
}

/// The desktop's cached thumbnail, else one drawn by an installed
/// thumbnailer. A missing one goes to `hints`.
fn picture(path: &Path, mime: &str, hints: &mut Vec<Hint>, cancel: &AtomicBool) -> Option<Pixels> {
    let file = gio::File::for_path(path);
    let cached = file
        .query_info("thumbnail::path,thumbnail::is-valid", gio::FileQueryInfoFlags::NONE, None::<&gio::Cancellable>)
        .ok()
        .filter(|info| info.boolean("thumbnail::is-valid"))
        .and_then(|info| info.attribute_byte_string("thumbnail::path"));
    if let Some(cached) = cached.and_then(|cached| load(Path::new(cached.as_str()))) {
        return Some(cached);
    }
    let thumbnailers: Vec<Thumbnailer> = thumbnailers().into_iter().filter(|t| t.handles(mime)).collect();
    let installed: Vec<(PathBuf, &Thumbnailer)> =
        thumbnailers.iter().filter_map(|t| Some((launch::find_program(t.program())?, t))).collect();
    if installed.is_empty() {
        let missing = thumbnailers.first().map(|t| (t.program().to_owned(), gettext("picture of the file")));
        if let Some((program, adds)) = missing.or_else(|| tools::known_thumbnailer(mime).map(|(p, adds)| (p.to_owned(), adds))) {
            hints.push(Hint { program, adds });
        }
        return None;
    }
    let (input, uri) = (path.to_str()?, file.uri());
    let dir = tempfile::Builder::new().prefix("doubycmd-preview").tempdir().ok()?;
    let output = dir.path().join("picture.png");
    installed.into_iter().find_map(|(program, thumbnailer)| {
        let command = thumbnailer.command(input, &uri, output.to_str()?, PICTURE_SIZE);
        let args: Vec<OsString> = command.into_iter().skip(1).map(Into::into).collect();
        run(&program, &args, None, PICTURE_TIMEOUT, MAX_OUTPUT, cancel)?;
        load(&output)
    })
}

/// The `.thumbnailer` files of the data directories, the user's first;
/// a name found earlier hides the same name later.
fn thumbnailers() -> Vec<Thumbnailer> {
    let mut seen = Vec::new();
    let mut found = Vec::new();
    let dirs = std::iter::once(glib::user_data_dir()).chain(glib::system_data_dirs());
    for dir in dirs.map(|dir| dir.join("thumbnailers")) {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if Path::new(&name).extension().is_none_or(|ext| ext != "thumbnailer") || seen.contains(&name) {
                continue;
            }
            seen.push(name);
            found.extend(std::fs::read_to_string(entry.path()).ok().as_deref().and_then(Thumbnailer::parse));
        }
    }
    found
}

fn load(picture: &Path) -> Option<Pixels> {
    let side = i32::try_from(PICTURE_SIZE).ok()?;
    Pixels::new(&Pixbuf::from_file_at_scale(picture, side, side, true).ok()?)
}

/// Runs `program` without a shell, `input` on its standard input; its
/// standard output (at most `max_output` bytes), unless it did not start,
/// ran out of `timeout` or was cancelled.
pub fn run(
    program: &Path,
    args: &[OsString],
    input: Option<&[u8]>,
    timeout: Duration,
    max_output: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // Small, it fits the pipe; a program that does not read it fails.
        stdin.write_all(input).ok();
    }
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.by_ref().take(max_output as u64).read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let told = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            // All that is read is read: the rest is not needed.
            Ok(None) if reader.is_finished() => {
                child.kill().ok();
                child.wait().ok();
                break true;
            }
            Ok(None) if Instant::now() < deadline && !cancel.load(Ordering::Relaxed) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                child.kill().ok();
                child.wait().ok();
                break false;
            }
        }
    };
    let output = reader.join().ok()?.ok()?;
    told.then_some(output)
}

/// One command for all the missing programs whose packages are known on
/// this distribution.
fn install_command(hints: &[Hint]) -> Option<String> {
    let os_release = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release"));
    let distribution = Distribution::from_os_release(&os_release.ok()?)?;
    let packages: Vec<&str> = hints.iter().filter_map(|hint| distribution.package(&hint.program)).collect();
    (!packages.is_empty()).then(|| distribution.install_command(&packages))
}
