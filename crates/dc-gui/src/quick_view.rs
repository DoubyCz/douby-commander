//! Quick view (F3): the other panel's place shows the file under the cursor
//! of the active panel and follows the cursor.
//!
//! Text in Neovim (else Vim) in a terminal: its syntax highlighting, in the
//! terminal colors of the theme, without the user's configuration (no
//! plugins or language servers starting for every file passed by), read
//! only. One editor serves the whole quick view; each next file is opened
//! in it by a typed `:view`. Images, video and music in GTK, folders and
//! other files as what they are. Nothing can be changed here; the button in
//! the header opens the file in its application.
//!
//! The view never takes the keyboard: keys stay with the active panel,
//! the mouse scrolls the text and works the player.

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use dc_core::format;
use dc_core::preview::{self, HEAD_SIZE, PreviewKind};
use dc_i18n::{fill, gettext};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};
use vte4::prelude::*;

use crate::launch;
use crate::music_view::MusicView;
use crate::panel_terminal;
use crate::pixels::Pixels;

/// Images larger than this are scaled down on loading (each side), the
/// view is smaller anyway and the memory stays bounded.
const MAX_IMAGE_SIDE: i32 = 4096;

type OpenFile = Box<dyn Fn(PathBuf)>;

/// What the quick view is asked to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    File(PathBuf),
    Directory(PathBuf),
    /// A file inside an archive, not on disk.
    InArchive(PathBuf),
}

impl Subject {
    fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::Directory(path) | Self::InArchive(path) => path,
        }
    }
}

/// What a worker thread found out about a file.
struct Probe {
    kind: PreviewKind,
    content_type: String,
    size: u64,
    image: Option<(Pixels, i32, i32)>,
}

pub struct QuickView {
    root: gtk::Box,
    title: gtk::Label,
    open_button: gtk::Button,
    pages: gtk::Stack,
    text_page: gtk::Box,
    /// Started when the first text is shown, then kept.
    text: RefCell<Option<TextViewer>>,
    picture: gtk::Picture,
    video: gtk::Picture,
    controls: gtk::MediaControls,
    music: MusicView,
    info_icon: gtk::Image,
    info_name: gtk::Label,
    info_text: gtk::Label,
    status: gtk::Label,
    shown: RefCell<Option<Subject>>,
    /// Raised by every new subject: results of slower ones are dropped.
    generation: Cell<u64>,
    open: RefCell<Option<OpenFile>>,
    this: Weak<Self>,
}

impl QuickView {
    pub fn new() -> Rc<Self> {
        let title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Start).build();
        title.add_css_class("dc-path");
        let open_button = gtk::Button::builder()
            .label(gettext("Open in application"))
            .tooltip_text(gettext("Close the quick view and open the file in its application"))
            .focusable(false)
            .focus_on_click(false)
            .build();
        open_button.add_css_class("flat");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("dc-bar");
        header.append(&title);
        header.append(&open_button);

        let text_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::ScaleDown).can_shrink(true).vexpand(true).build();
        let video = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).vexpand(true).build();
        let controls = gtk::MediaControls::new(None::<&gtk::MediaStream>);
        let media = gtk::Box::new(gtk::Orientation::Vertical, 0);
        media.append(&video);
        media.append(&controls);
        let music = MusicView::new();

        let info_icon = gtk::Image::builder().pixel_size(96).build();
        // Long names are shortened in the middle (the extension stays), the
        // whole name is in the tooltip and in the header: a label that wraps
        // cannot break a name without spaces and would widen the window.
        let info_name = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).build();
        info_name.add_css_class("title-3");
        let info_text = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .build();
        info_text.add_css_class("dim-label");
        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .margin_start(24)
            .margin_end(24)
            .build();
        info.append(&info_icon);
        info.append(&info_name);
        info.append(&info_text);

        let pages = gtk::Stack::builder().vexpand(true).hexpand(true).build();
        pages.add_named(&info, Some("info"));
        pages.add_named(&text_page, Some("text"));
        pages.add_named(&picture, Some("image"));
        pages.add_named(&media, Some("video"));
        pages.add_named(music.widget(), Some("music"));

        let status = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        status.add_css_class("dc-status");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("dc-panel");
        root.add_css_class("dc-quick-view");
        root.append(&header);
        root.append(&pages);
        root.append(&status);

        let view = Rc::new_cyclic(|this| Self {
            root,
            title,
            open_button,
            pages,
            text_page,
            text: RefCell::default(),
            picture,
            video,
            controls,
            music,
            info_icon,
            info_name,
            info_text,
            status,
            shown: RefCell::default(),
            generation: Cell::new(0),
            open: RefCell::default(),
            this: this.clone(),
        });
        let weak = Rc::downgrade(&view);
        view.open_button.connect_clicked(move |_| {
            let Some(view) = weak.upgrade() else { return };
            let file = match view.shown.borrow().as_ref() {
                Some(Subject::File(path)) => Some(path.clone()),
                _ => None,
            };
            if let (Some(file), Some(open)) = (file, view.open.borrow().as_ref()) {
                open(file);
            }
        });
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// `callback` opens a file in its application, for the button in the header.
    pub fn connect_open(&self, callback: impl Fn(PathBuf) + 'static) {
        *self.open.borrow_mut() = Some(Box::new(callback));
    }

    /// Shows `subject`, unless it is shown already; `None` shows nothing.
    pub fn show(&self, subject: Option<Subject>) {
        if *self.shown.borrow() == subject {
            return;
        }
        self.generation.set(self.generation.get() + 1);
        self.stop_media();
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        self.open_button.set_sensitive(matches!(subject, Some(Subject::File(_))));
        self.title.set_text(&subject.as_ref().map(|subject| subject.path().display().to_string()).unwrap_or_default());
        self.shown.borrow_mut().clone_from(&subject);
        match subject {
            None => self.show_info("", "", "", None),
            Some(Subject::Directory(path)) => self.show_info("folder", &file_name(&path), &gettext("Folder"), None),
            Some(Subject::InArchive(path)) => {
                let (content_type, _) = gio::content_type_guess(Some(&path), None);
                self.show_info(
                    &icon_name(&content_type),
                    &file_name(&path),
                    &gettext("Files inside archives cannot be shown yet."),
                    None,
                );
            }
            Some(Subject::File(path)) => self.probe(path),
        }
    }

    /// The quick view is closed: the player stops, the editor lets go of
    /// the file. What was shown is forgotten, the next time starts anew.
    pub fn hide(&self) {
        self.generation.set(self.generation.get() + 1);
        self.stop_media();
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        if let Some(text) = self.text.borrow().as_ref() {
            text.release();
        }
        self.shown.replace(None);
    }

    /// Finds out in a worker thread what the file is, then shows it.
    fn probe(&self, path: PathBuf) {
        let generation = self.generation.get();
        let weak = self.this.clone();
        glib::spawn_future_local(async move {
            let probed = path.clone();
            let probe = gio::spawn_blocking(move || probe(&probed)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match probe {
                Ok(Ok(probe)) => view.show_probed(&path, probe),
                Ok(Err(err)) => view.show_info("dialog-error", &file_name(&path), &err.to_string(), None),
                Err(_) => view.show_info("dialog-error", &file_name(&path), "", None),
            }
        });
    }

    fn show_probed(&self, path: &Path, probe: Probe) {
        let description = gio::content_type_get_description(&probe.content_type).to_string();
        let size = format::size(probe.size);
        let status = fill(&gettext("{type}, {size}"), &[("type", &description), ("size", &size)]);
        match probe.kind {
            PreviewKind::Text => match self.show_text(path) {
                Ok(()) => self.status.set_text(&status),
                Err(message) => self.show_info(&icon_name(&probe.content_type), &file_name(path), &message, Some(&status)),
            },
            PreviewKind::Image => match probe.image {
                Some((pixels, width, height)) => {
                    self.picture.set_paintable(Some(&pixels.texture()));
                    self.pages.set_visible_child_name("image");
                    self.status.set_text(&fill(
                        &gettext("{type}, {width} × {height}, {size}"),
                        &[("type", &description), ("width", &width), ("height", &height), ("size", &size)],
                    ));
                }
                None => self.show_info(
                    &icon_name(&probe.content_type),
                    &file_name(path),
                    &gettext("The image cannot be read."),
                    Some(&status),
                ),
            },
            PreviewKind::Video => {
                let media = gtk::MediaFile::for_filename(path);
                self.video.set_paintable(Some(&media));
                self.controls.set_media_stream(Some(&media));
                play_when_prepared(&media);
                self.pages.set_visible_child_name("video");
                self.status.set_text(&status);
            }
            PreviewKind::Audio => {
                self.music.load(path, &icon_name(&probe.content_type));
                self.pages.set_visible_child_name("music");
                self.status.set_text(&status);
            }
            PreviewKind::Other => self.show_info(&icon_name(&probe.content_type), &file_name(path), &description, Some(&status)),
        }
    }

    /// The file in the editor; a message why not when there is none.
    fn show_text(&self, path: &Path) -> Result<(), String> {
        let command = view_command(path).ok_or_else(|| gettext("The file name cannot be passed to the viewer."))?;
        let mut text = self.text.borrow_mut();
        if let Some(viewer) = text.as_ref() {
            viewer.terminal.feed_child(&command);
        } else {
            let viewer = TextViewer::start(path, self.this.clone())
                .ok_or_else(|| gettext("Install Neovim (or Vim) to view text files."))?;
            self.text_page.append(&viewer.terminal);
            *text = Some(viewer);
        }
        self.pages.set_visible_child_name("text");
        Ok(())
    }

    /// The editor ended (it should not, nobody can type `:q` there): the
    /// next text starts a new one.
    fn text_exited(&self) {
        if let Some(viewer) = self.text.take() {
            self.text_page.remove(&viewer.terminal);
        }
        if self.pages.visible_child_name().as_deref() == Some("text") {
            self.shown.replace(None);
            self.show_info("dialog-error", "", &gettext("The text viewer ended."), None);
        }
    }

    fn show_info(&self, icon: &str, name: &str, text: &str, status: Option<&str>) {
        self.info_icon.set_icon_name((!icon.is_empty()).then_some(icon));
        self.info_name.set_text(name);
        self.info_name.set_tooltip_text((!name.is_empty()).then_some(name));
        self.info_text.set_text(text);
        self.status.set_text(status.unwrap_or_default());
        self.pages.set_visible_child_name("info");
    }

    fn stop_media(&self) {
        if let Some(stream) = self.controls.media_stream() {
            stream.pause();
        }
        self.controls.set_media_stream(None::<&gtk::MediaStream>);
        self.video.set_paintable(None::<&gtk::gdk::Paintable>);
        self.music.stop();
    }
}

/// Video and music start on their own (Jirka): once the stream is ready,
/// unless it was let go of meanwhile.
pub fn play_when_prepared(stream: &impl IsA<gtk::MediaStream>) {
    if stream.is_prepared() {
        stream.play();
        return;
    }
    let handler = Rc::new(Cell::new(None::<glib::SignalHandlerId>));
    let disconnect = Rc::clone(&handler);
    let id = stream.connect_prepared_notify(move |stream| {
        if stream.is_prepared() {
            stream.play();
            if let Some(id) = disconnect.take() {
                stream.disconnect(id);
            }
        }
    });
    handler.set(Some(id));
}

/// Neovim, or Vim, in a terminal of its own.
struct TextViewer {
    terminal: vte4::Terminal,
}

impl TextViewer {
    /// Starts the editor with `path`; `None` when there is none.
    fn start(path: &Path, view: Weak<QuickView>) -> Option<Self> {
        let argv = editor_argv(path)?;
        let terminal = vte4::Terminal::builder().hexpand(true).vexpand(true).focusable(false).build();
        terminal.set_audible_bell(false);
        terminal.set_mouse_autohide(true);
        panel_terminal::follow_theme(&terminal);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let dir = path.parent().and_then(Path::to_str);
        terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            dir,
            &argv,
            &[],
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            None::<&gio::Cancellable>,
            |_| {},
        );
        terminal.connect_child_exited(move |_, _| {
            if let Some(view) = view.upgrade() {
                view.text_exited();
            }
        });
        Some(Self { terminal })
    }

    /// Lets go of the file (`:enew`, the file's buffer is wiped).
    fn release(&self) {
        self.terminal.feed_child(b"\x1c\x0e:silent! enew\r");
    }
}

/// Settings of the editor as a viewer: read only, line numbers, the mouse
/// scrolls, nothing around the text. Every buffer is wiped once left.
const VIEW_SETTINGS: &str = "set nomodifiable number mouse=a laststatus=0 noruler noshowcmd noshowmode shortmess+=IF";
const VIEW_AUTOCMD: &str = "autocmd BufWinEnter * setlocal nomodifiable bufhidden=wipe";

/// The command line of Neovim, or Vim, as a viewer of `path`. Without the
/// user's configuration; colors from the terminal's palette, which is the
/// theme's (`notermguicolors`).
fn editor_argv(path: &Path) -> Option<Vec<String>> {
    let path = path.to_str()?.to_owned();
    let args = |program: &str, clean: &[&str], extra: &str| {
        let mut argv: Vec<String> = std::iter::once(program).chain(clean.iter().copied()).map(str::to_owned).collect();
        argv.extend(["-n", "-R", "-c"].map(str::to_owned));
        argv.push(format!("{VIEW_SETTINGS}{extra}"));
        argv.extend(["-c".to_owned(), VIEW_AUTOCMD.to_owned(), "--".to_owned(), path.clone()]);
        argv
    };
    if launch::find_program("nvim").is_some() {
        Some(args("nvim", &["--clean"], " notermguicolors cmdheight=0"))
    } else if launch::find_program("vim").is_some() {
        Some(args("vim", &["-u", "DEFAULTS", "-i", "NONE"], ""))
    } else {
        None
    }
}

/// Keys that open `path` in the running editor: to normal mode
/// (Ctrl+\ Ctrl+N), then `:view` with the name as a quoted string, so
/// that no character of it means anything. `None` for names the command
/// line cannot carry (not UTF-8, control characters).
fn view_command(path: &Path) -> Option<Vec<u8>> {
    let path = path.to_str().filter(|path| !path.chars().any(char::is_control))?;
    let quoted = path.replace('\'', "''");
    Some(format!("\x1c\x0e:silent! exe 'view' fnameescape('{quoted}')\r").into_bytes())
}

/// Type, size and, for images, the pixels; in a worker thread.
fn probe(path: &Path) -> io::Result<Probe> {
    let metadata = std::fs::metadata(path)?;
    let mut head = Vec::with_capacity(HEAD_SIZE);
    // Only regular files are read: a pipe or a device would never end.
    if metadata.is_file() {
        File::open(path)?.take(HEAD_SIZE as u64).read_to_end(&mut head)?;
    }
    let (content_type, _) = gio::content_type_guess(Some(path), Some(head.as_slice()));
    let kind = if metadata.is_file() {
        let mime = gio::content_type_get_mime_type(&content_type).map_or_else(|| content_type.to_string(), Into::into);
        preview::classify(&mime, gio::content_type_is_a(&content_type, "text/plain"), &head)
    } else {
        PreviewKind::Other
    };
    let image = (kind == PreviewKind::Image).then(|| load_image(path)).flatten();
    Ok(Probe { kind, content_type: content_type.to_string(), size: metadata.len(), image })
}

/// The image turned as the camera says (EXIF), at most [`MAX_IMAGE_SIDE`],
/// with its own size.
fn load_image(path: &Path) -> Option<(Pixels, i32, i32)> {
    let (_, width, height) = Pixbuf::file_info(path)?;
    let pixbuf = Pixbuf::from_file_at_scale(path, MAX_IMAGE_SIDE.min(width), MAX_IMAGE_SIDE.min(height), true).ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    Some((Pixels::new(&pixbuf)?, width, height))
}

fn icon_name(content_type: &str) -> String {
    gio::content_type_get_generic_icon_name(content_type).map_or_else(|| "text-x-generic".to_owned(), Into::into)
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_command_quotes_the_name() {
        let command = view_command(Path::new("/tmp/it's a | b %.txt")).unwrap();
        assert_eq!(command, b"\x1c\x0e:silent! exe 'view' fnameescape('/tmp/it''s a | b %.txt')\r");
        assert_eq!(view_command(Path::new("/tmp/line\nbreak")), None);
    }
}
