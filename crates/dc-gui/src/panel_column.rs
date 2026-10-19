//! Everything above one file panel, as in the original:
//!
//! 1. drive bar (`dskLeft` / `dskRight`): a button per mounted file system,
//!    the one holding the current directory highlighted,
//! 2. header: drive menu button, free space, and the buttons `*` (directory
//!    hotlist), `/`, `..`, `~` and `<` / `>` (the other panel's directory),
//! 3. tabs (`nbLeft` / `nbRight`): for now the single tab of the panel.
//!
//! The buttons act on their own panel, which becomes active first.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use dc_core::command::Command;
use dc_core::format;
use dc_i18n::{fill, gettext};
use dc_vfs::mounts::{self, DriveKind, MountPoint};
use gtk::prelude::*;

use crate::commander::Commander;
use crate::texts;

/// Builds the column of panel `side` (0 left, 1 right) around its view.
pub fn build(commander: &Rc<Commander>, side: usize, drives: &[MountPoint]) -> gtk::Box {
    let panel = Rc::clone(&commander.panels()[side]);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let (drive_bar, drive_buttons) = drive_bar(commander, side, drives);
    let header = header(commander, side, drives);
    let (tabs, tab_label) = tabs();
    column.append(&drive_bar);
    column.append(&header.row);
    column.append(&tabs);
    column.append(panel.widget());

    let drives = drives.to_vec();
    panel.connect_path_changed(move |path| {
        let current = current_drive(&drives, path);
        for (index, button) in drive_buttons.iter().enumerate() {
            if Some(index) == current {
                button.add_css_class("dc-current-drive");
            } else {
                button.remove_css_class("dc-current-drive");
            }
        }
        header.show_free_space(path);
        tab_label.set_text(&tab_title(path));
    });
    column
}

fn drive_bar(commander: &Rc<Commander>, side: usize, drives: &[MountPoint]) -> (gtk::Box, Vec<gtk::Button>) {
    let bar = gtk::Box::builder().spacing(2).build();
    bar.add_css_class("dc-bar");
    let buttons: Vec<gtk::Button> = drives
        .iter()
        .map(|drive| {
            let button = drive_button(commander, side, drive, true);
            bar.append(&button);
            button
        })
        .collect();
    (bar, buttons)
}

/// Icon and name; a click shows the drive in the panel of `side`.
fn drive_button(commander: &Rc<Commander>, side: usize, drive: &MountPoint, flat: bool) -> gtk::Button {
    let content = gtk::Box::builder().spacing(4).build();
    content.append(&gtk::Image::from_icon_name(drive_icon(drive.kind)));
    content.append(&gtk::Label::new(Some(&drive.display_name())));
    let tooltip = if drive.fs_type.is_empty() {
        drive.path.display().to_string()
    } else {
        fill(&dc_i18n::pgettext("drive tooltip", "{path} ({type})"), &[("path", &drive.path.display()), ("type", &drive.fs_type)])
    };
    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(tooltip)
        .focusable(false)
        .focus_on_click(false)
        .build();
    if flat {
        button.add_css_class("flat");
    }
    let (commander, path) = (Rc::downgrade(commander), drive.path.clone());
    button.connect_clicked(move |_| {
        if let Some(commander) = commander.upgrade() {
            show_in_panel(&commander, side, path.clone());
        }
    });
    button
}

fn show_in_panel(commander: &Rc<Commander>, side: usize, path: PathBuf) {
    commander.set_active(side);
    let panel = &commander.panels()[side];
    panel.change_dir(path);
    panel.focus();
}

struct Header {
    row: gtk::Box,
    free_space: gtk::Label,
}

impl Header {
    /// "711.6 G free", the tooltip with the total (`rsFreeMsgShort` / `rsFreeMsg`).
    fn show_free_space(&self, path: &Path) {
        let Some(space) = mounts::free_space(path) else {
            self.free_space.set_text("");
            self.free_space.set_tooltip_text(None);
            return;
        };
        let (free, total) = (format::size(space.free), format::size(space.total));
        self.free_space.set_text(&fill(&gettext("{free} free"), &[("free", &free)]));
        self.free_space
            .set_tooltip_text(Some(&fill(&gettext("{free} of {total} free"), &[("free", &free), ("total", &total)])));
    }
}

fn header(commander: &Rc<Commander>, side: usize, drives: &[MountPoint]) -> Header {
    let row = gtk::Box::builder().spacing(2).build();
    row.add_css_class("dc-bar");

    // Drive menu (`actLeftOpenDrives`): the root and all drives.
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let popover = gtk::Popover::builder().child(&menu).build();
    let root = MountPoint {
        path: PathBuf::from("/"),
        device: String::new(),
        fs_type: String::new(),
        kind: DriveKind::HardDisk,
    };
    for drive in std::iter::once(&root).chain(drives) {
        let button = drive_button(commander, side, drive, true);
        let popover = popover.downgrade();
        button.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });
        menu.append(&button);
    }
    let drive_menu = gtk::MenuButton::builder()
        .icon_name("drive-harddisk")
        .popover(&popover)
        .tooltip_text(gettext("Open drive list"))
        .focusable(false)
        .build();
    drive_menu.add_css_class("flat");
    row.append(&drive_menu);

    let free_space = gtk::Label::builder().hexpand(true).build();
    row.append(&free_space);

    let equal = if side == 0 {
        ("<", gettext("Show current directory of the right panel in the left panel"), Some(Command::LeftEqualRight))
    } else {
        (">", gettext("Show current directory of the left panel in the right panel"), Some(Command::RightEqualLeft))
    };
    let buttons = [
        ("*", gettext("Directory Hotlist"), None),
        ("/", gettext("Go to root directory"), Some(Command::ChangeDirToRoot)),
        ("..", gettext("Go to parent directory"), Some(Command::ChangeDirToParent)),
        ("~", gettext("Go to home directory"), Some(Command::ChangeDirToHome)),
        equal,
    ];
    for (caption, hint, command) in buttons {
        let button = gtk::Button::builder().label(caption).focusable(false).focus_on_click(false).build();
        button.add_css_class("flat");
        if let Some(command) = command {
            button.set_tooltip_text(Some(&hint));
            let commander = Rc::downgrade(commander);
            button.connect_clicked(move |_| {
                if let Some(commander) = commander.upgrade() {
                    commander.execute_on(side, command);
                }
            });
        } else {
            button.set_sensitive(false);
            button.set_tooltip_text(Some(&fill(
                &dc_i18n::pgettext("tooltip of a disabled button", "{hint} - {reason}"),
                &[("hint", &hint), ("reason", &texts::not_implemented())],
            )));
        }
        row.append(&button);
    }

    Header { row, free_space }
}

/// Tab strip with the panel's only tab until tabs exist (`nbLeft` of the original).
fn tabs() -> (gtk::Box, gtk::Label) {
    let strip = gtk::Box::builder().margin_start(4).margin_top(2).build();
    let label = gtk::Label::new(None);
    label.add_css_class("dc-tab");
    strip.append(&label);
    (strip, label)
}

/// Tab caption: the directory name, `/` for the root.
fn tab_title(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// The drive holding `path`: the one with the longest matching mount point.
fn current_drive(drives: &[MountPoint], path: &Path) -> Option<usize> {
    drives
        .iter()
        .enumerate()
        .filter(|(_, drive)| path.starts_with(&drive.path))
        .max_by_key(|(_, drive)| drive.path.components().count())
        .map(|(index, _)| index)
}

/// Icons of the original's theme (`devices`).
fn drive_icon(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::HardDisk => "drive-harddisk",
        DriveKind::Removable => "drive-removable-media-usb",
        DriveKind::Optical => "drive-optical",
        DriveKind::Network => "network-wired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(path: &str) -> MountPoint {
        MountPoint { path: path.into(), device: String::new(), fs_type: String::new(), kind: DriveKind::HardDisk }
    }

    #[test]
    fn current_drive_is_the_deepest_mount_point() {
        let drives = [drive("/home"), drive("/var/log"), drive("/home/jirka/mnt")];
        assert_eq!(current_drive(&drives, Path::new("/home/jirka/x")), Some(0));
        assert_eq!(current_drive(&drives, Path::new("/home/jirka/mnt/y")), Some(2));
        assert_eq!(current_drive(&drives, Path::new("/usr")), None);
        assert_eq!(tab_title(Path::new("/usr/lib/doublecmd")), "doublecmd");
        assert_eq!(tab_title(Path::new("/")), "/");
    }
}
