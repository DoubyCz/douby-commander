//! The terminal of a panel (F9): the user's shell shown in the panel's
//! place, in the colors of the theme.
//!
//! It is made when first shown and then lives on behind the files: every
//! directory the panel moves to, the shell follows with `cd`, so showing
//! it again finds it where the panel is. Only at its prompt: while a
//! program runs in it the `cd` waits, it would be typed into the program.
//! Shown, it lists the directory with `ls`, unless the last command was
//! that already. `exit` ends it, the panel then shows its files again.
//!
//! The shell's directory and whether it is at its prompt are read from
//! `/proc`, no cooperation of the shell (OSC 7 or the like) is needed.

use std::cell::{Cell, RefCell};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use vte4::prelude::*;

use crate::theme;

/// Lines kept above the screen.
const SCROLLBACK_LINES: i64 = 10_000;

pub struct PanelTerminal {
    root: gtk::Box,
    terminal: vte4::Terminal,
    /// The shell, once started.
    shell: Cell<Option<glib::Pid>>,
    /// Where the shell was last seen; once it exited, `/proc` no longer knows.
    last_dir: RefCell<PathBuf>,
    /// Where the shell should go when it is back at its prompt.
    pending_dir: RefCell<Option<PathBuf>>,
    /// The panel's directory last followed: a mere reload of it does not
    /// take back a `cd` the user made in the terminal.
    panel_dir: RefCell<PathBuf>,
    /// The line the user is typing, as far as it can be followed; `None`
    /// after keys that edit it in ways not followed (arrows, history, Tab).
    typed: RefCell<Option<String>>,
    /// The last command run was `ls`: showing the terminal lists nothing new.
    listed: Cell<bool>,
    /// `ls` once the shell is at its prompt (the terminal was shown).
    pending_list: Cell<bool>,
    /// Set while we type into the shell ourselves.
    feeding: Cell<bool>,
    this: Weak<Self>,
}

impl PanelTerminal {
    /// Starts the user's shell in `dir`. `exited` runs when the shell ends,
    /// with the directory it was in last.
    pub fn new(dir: &Path, exited: impl Fn(PathBuf) + 'static) -> Rc<Self> {
        let terminal = vte4::Terminal::builder().hexpand(true).vexpand(true).build();
        terminal.set_scrollback_lines(SCROLLBACK_LINES);
        terminal.set_mouse_autohide(true);
        terminal.set_audible_bell(false);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_font(Some(&monospace_font(&terminal)));
        let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, terminal.vadjustment().as_ref());
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("dc-terminal");
        root.append(&terminal);
        root.append(&scrollbar);

        let view = Rc::new_cyclic(|this| Self {
            root,
            terminal,
            shell: Cell::new(None),
            last_dir: RefCell::new(dir.to_owned()),
            pending_dir: RefCell::new(None),
            panel_dir: RefCell::new(dir.to_owned()),
            typed: RefCell::new(Some(String::new())),
            listed: Cell::new(false),
            pending_list: Cell::new(false),
            feeding: Cell::new(false),
            this: this.clone(),
        });
        view.apply_colors();
        let weak = Rc::downgrade(&view);
        theme::connect_changed(move || {
            let Some(view) = weak.upgrade() else { return false };
            view.apply_colors();
            true
        });
        view.connect_signals(exited);
        view.spawn(dir);
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn grab_focus(&self) {
        self.terminal.grab_focus();
    }

    pub fn has_focus(&self) -> bool {
        self.terminal.has_focus()
    }

    /// The panel moved to `dir`: the shell goes there too, now or once it
    /// is back at its prompt.
    pub fn follow(&self, dir: &Path) {
        if *self.panel_dir.borrow() == dir {
            return;
        }
        dir.clone_into(&mut self.panel_dir.borrow_mut());
        *self.pending_dir.borrow_mut() = Some(dir.to_owned());
        self.run_pending();
    }

    /// The terminal comes to the front: unless the last command was `ls`,
    /// it lists the directory, now or once the shell is at its prompt.
    pub fn shown(&self) {
        self.pending_list.set(true);
        self.run_pending();
    }

    /// Back behind the files: no `ls` while nobody sees it.
    pub fn hidden(&self) {
        self.pending_list.set(false);
    }

    /// The directory the shell is in.
    pub fn current_dir(&self) -> PathBuf {
        self.remember_dir();
        self.last_dir.borrow().clone()
    }

    fn spawn(&self, dir: &Path) {
        let shell = user_shell();
        let weak = self.this.clone();
        self.terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            dir.to_str(),
            &[shell.as_str()],
            // Added to the environment of the application, which is kept.
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let Some(view) = weak.upgrade() else { return };
                match result {
                    Ok(pid) => {
                        view.shell.set(Some(pid));
                        view.run_pending();
                    }
                    Err(err) => view.terminal.feed(format!("{err}\r\n").as_bytes()),
                }
            },
        );
    }

    fn connect_signals(&self, exited: impl Fn(PathBuf) + 'static) {
        let weak = self.this.clone();
        self.terminal.connect_commit(move |_, text, _| {
            if let Some(view) = weak.upgrade()
                && !view.feeding.get()
            {
                view.user_typed(text);
            }
        });
        // A program that ended brings the prompt back, with new contents.
        let weak = self.this.clone();
        self.terminal.connect_contents_changed(move |_| {
            if let Some(view) = weak.upgrade() {
                view.run_pending();
            }
        });
        let weak = self.this.clone();
        self.terminal.connect_child_exited(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.shell.set(None);
                exited(view.last_dir.borrow().clone());
            }
        });

        // Copying and pasting as in the desktop's terminals; VTE has no
        // keys of its own for that.
        let keys = gtk::EventControllerKey::new();
        let weak = self.this.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else { return glib::Propagation::Proceed };
            let modifiers = state & gtk::accelerator_get_default_mod_mask();
            if modifiers != gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK {
                return glib::Propagation::Proceed;
            }
            match key.to_lower() {
                gdk::Key::c => view.terminal.copy_clipboard_format(vte4::Format::Text),
                gdk::Key::v => view.terminal.paste_clipboard(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.terminal.add_controller(keys);
    }

    /// Notes where the shell is, while it runs.
    fn remember_dir(&self) {
        if let Some(dir) = self.shell.get().and_then(|pid| std::fs::read_link(format!("/proc/{}/cwd", pid.0)).ok()) {
            *self.last_dir.borrow_mut() = dir;
        }
    }

    /// Keys the user sent to the shell: follows the line being typed, to
    /// know whether the command run was `ls`.
    fn user_typed(&self, text: &str) {
        let mut typed = self.typed.borrow_mut();
        for ch in text.chars() {
            match ch {
                // Enter, or Ctrl+D, which may end the shell: its directory
                // is noted while it is still there (`exit` after `cd` takes
                // the panel there).
                '\r' | '\n' => {
                    self.listed.set(typed.as_deref().is_some_and(is_ls));
                    *typed = Some(String::new());
                    self.remember_dir();
                }
                '\u{4}' => self.remember_dir(),
                // Ctrl+C, Ctrl+U: an empty line again (as far as known).
                '\u{3}' | '\u{15}' => *typed = Some(String::new()),
                '\u{7f}' | '\u{8}' => {
                    if let Some(line) = typed.as_mut() {
                        line.pop();
                    }
                }
                ch if ch.is_control() => *typed = None,
                ch => {
                    if let Some(line) = typed.as_mut() {
                        line.push(ch);
                    }
                }
            }
        }
    }

    /// Types what waits into the shell, if it is at its prompt: the `cd`
    /// after the panel, then `ls` when the terminal was shown.
    fn run_pending(&self) {
        let Some(pid) = self.shell.get() else { return };
        if (self.pending_dir.borrow().is_none() && !self.pending_list.get()) || !at_prompt(pid) {
            return;
        }
        if let Some(dir) = self.pending_dir.take() {
            self.remember_dir();
            if *self.last_dir.borrow() != dir {
                self.send(&cd_command(&dir));
                self.listed.set(false);
                *self.last_dir.borrow_mut() = dir;
            }
        }
        if self.pending_list.replace(false) && !self.listed.get() {
            self.send(LS_COMMAND);
            self.listed.set(true);
        }
    }

    /// Types `command` into the shell, cutting the line typed so far.
    fn send(&self, command: &[u8]) {
        self.feeding.set(true);
        self.terminal.feed_child(command);
        self.feeding.set(false);
        *self.typed.borrow_mut() = Some(String::new());
    }

    fn apply_colors(&self) {
        if let Some(colors) = theme::terminal_colors() {
            let palette: Vec<&gdk::RGBA> = colors.palette.iter().collect();
            self.terminal.set_colors(Some(&colors.foreground), Some(&colors.background), &palette);
            self.terminal.set_color_cursor(colors.cursor.as_ref());
            self.terminal.set_color_highlight(colors.selection_background.as_ref());
            self.terminal.set_color_highlight_foreground(colors.selection_foreground.as_ref());
        } else {
            // Without a theme: the text color of the application on the
            // panel's own background, VTE's palette for the rest.
            let transparent = gdk::RGBA::new(0.0, 0.0, 0.0, 0.0);
            self.terminal.set_colors(Some(&self.terminal.color()), Some(&transparent), &[]);
        }
    }
}

/// `ls` typed into the shell when the terminal is shown (the user's alias
/// of it applies), the line typed so far cut as by [`cd_command`].
const LS_COMMAND: &[u8] = b"\x05\x15 ls\r";

/// Whether the command line `line` runs `ls`.
fn is_ls(line: &str) -> bool {
    line.split_whitespace().next() == Some("ls")
}

/// The user's login shell: `$SHELL`, else `/bin/sh`.
fn user_shell() -> String {
    std::env::var("SHELL").ok().filter(|shell| !shell.is_empty()).unwrap_or_else(|| "/bin/sh".to_owned())
}

/// The system's monospace font at the size of the application's text.
fn monospace_font(widget: &impl IsA<gtk::Widget>) -> gtk::pango::FontDescription {
    let mut font = widget.pango_context().font_description().unwrap_or_default();
    font.set_family("monospace");
    font
}

/// Whether the shell `pid` is at its prompt: nothing else runs in the
/// foreground of its terminal (field `tpgid` of `/proc/<pid>/stat`).
fn at_prompt(pid: glib::Pid) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid.0)) else { return false };
    foreground_group(&stat) == Some(pid.0)
}

/// `tpgid` of a `/proc/<pid>/stat` line. The command name in parentheses
/// may contain spaces, the fields are counted after it.
fn foreground_group(stat: &str) -> Option<i32> {
    let (_, fields) = stat.rsplit_once(')')?;
    // state ppid pgrp session tty_nr tpgid
    fields.split_whitespace().nth(5)?.parse().ok()
}

/// What is typed into the shell to go to `dir`: the line typed so far is
/// cut (Ctrl+E Ctrl+U, back with Ctrl+Y), the leading space keeps the `cd`
/// out of the history, the path is quoted for any shell.
fn cd_command(dir: &Path) -> Vec<u8> {
    let mut command = b"\x05\x15 cd -- '".to_vec();
    for &byte in dir.as_os_str().as_bytes() {
        if byte == b'\'' {
            command.extend_from_slice(b"'\\''");
        } else {
            command.push(byte);
        }
    }
    command.extend_from_slice(b"'\r");
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_group_after_the_command_name() {
        let stat = "4242 (my (odd) sh) S 1 4242 4242 34817 5000 4194304 0 0";
        assert_eq!(foreground_group(stat), Some(5000));
        assert_eq!(foreground_group("garbage"), None);
    }

    #[test]
    fn cd_quotes_the_path() {
        assert_eq!(cd_command(Path::new("/tmp/a b")), b"\x05\x15 cd -- '/tmp/a b'\r");
        assert_eq!(cd_command(Path::new("/it's")), b"\x05\x15 cd -- '/it'\\''s'\r");
    }

    #[test]
    fn ls_with_or_without_arguments() {
        assert!(is_ls("ls"));
        assert!(is_ls("  ls -la /tmp"));
        assert!(!is_ls("lsblk"));
        assert!(!is_ls(""));
    }
}
