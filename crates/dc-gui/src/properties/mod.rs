//! The properties of files (`cm_FileProperties`, Alt+Enter) and changing
//! their attributes (`cm_SetFileProperties`): one window for both, for one
//! item or many. The original has a window of properties with a page of
//! attributes and besides it a dialog to change attributes with the same
//! grid again (and the attributes of Windows); here it is one place.
//!
//! Pages: General (what, where, how big, when), Permissions (owner, group,
//! `rwx`, special bits), Times (modification, access) and, for one item,
//! Extended (flags of the file system, extended attributes, ACL). Changes
//! run as an operation (`dc_ops::SetAttributesOperation`), optionally also
//! inside directories.

mod extended;
mod general;
mod permissions;
mod times;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dc_core::properties::{self, Details};
use dc_i18n::{fill, gettext, ngettext, pgettext, plural_count};
use dc_ops::AttributeChanges;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dialogs;
use permissions::Permissions;
use times::Times;

/// The page the window opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Permissions,
}

/// A row of a page: a title and a value to select and copy.
fn grid_row(grid: &gtk::Grid, row: i32, title: &str, value: &str) {
    let title = gtk::Label::builder().label(title).xalign(0.0).yalign(0.0).css_classes(["dim-label"]).build();
    let value = gtk::Label::builder()
        .label(value)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .hexpand(true)
        .build();
    grid.attach(&title, 0, row, 1, 1);
    grid.attach(&value, 1, row, 1, 1);
}

fn page(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    child.set_margin_top(14);
    child.set_margin_bottom(14);
    child.set_margin_start(14);
    child.set_margin_end(14);
    gtk::ScrolledWindow::builder().child(child).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build()
}

/// The header: the icon of the type, the name and how many items.
fn header(paths: &[PathBuf]) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::builder().pixel_size(48).build();
    let title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["title-3"]).build();
    if let [path] = paths {
        let info = gio::File::for_path(path)
            .query_info("standard::icon", gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, None::<&gio::Cancellable>)
            .ok();
        if let Some(gicon) = info.and_then(|info| info.icon()) {
            icon.set_from_gicon(&gicon);
        }
        title.set_text(&properties::display_name(path).to_string_lossy());
    } else {
        icon.set_icon_name(Some("edit-copy-symbolic"));
        title.set_text(&fill(&ngettext("{count} item", "{count} items", plural_count(paths.len())), &[("count", &paths.len())]));
    }
    header.append(&icon);
    header.append(&title);
    header
}

/// Opens the window for `paths` on `page`; `apply` starts the operation
/// with what changed.
pub fn show(parent: Option<&gtk::Window>, paths: Vec<PathBuf>, page_shown: Page, apply: impl Fn(Vec<PathBuf>, AttributeChanges) + 'static) {
    if paths.is_empty() {
        return;
    }
    let details: Vec<Details> = paths.iter().filter_map(|path| Details::read(path).ok()).collect();
    let title = match paths.as_slice() {
        [path] => fill(&gettext("Properties of {name}"), &[("name", &properties::display_name(path).to_string_lossy())]),
        _ => gettext("Properties"),
    };
    let window = gtk::Window::builder().title(title).default_width(640).default_height(600).build();
    window.set_transient_for(parent);
    // Counting the contents stops with the window.
    let cancel = Arc::new(AtomicBool::new(false));

    let notebook = gtk::Notebook::new();
    let general = match (paths.as_slice(), details.first()) {
        ([path], Some(details)) => general::single(path, details, &cancel),
        ([path], None) => {
            let error = std::fs::symlink_metadata(path).err().unwrap_or_else(|| std::io::ErrorKind::Other.into());
            general::unreadable(path, &error)
        }
        _ => general::several(&paths, &cancel),
    };
    notebook.append_page(&page(&general), Some(&tab(&gettext("_General"))));

    let any_dir = details.iter().any(Details::is_dir);
    let editable = !details.is_empty();
    let permissions = editable.then(|| {
        let modes: Vec<u32> = details.iter().map(|details| details.mode).collect();
        let uids: Vec<u32> = details.iter().map(|details| details.uid).collect();
        let gids: Vec<u32> = details.iter().map(|details| details.gid).collect();
        Permissions::new(&modes, &uids, &gids, any_dir)
    });
    let times = details.first().map(|first| Times::new(first.modified, first.accessed));
    if let (Some(permissions), Some(times)) = (&permissions, &times) {
        notebook.append_page(&page(&permissions.page), Some(&tab(&gettext("_Permissions"))));
        notebook.append_page(&page(&times.page), Some(&tab(&gettext("_Times"))));
    }
    if let [path] = paths.as_slice() {
        notebook.append_page(&page(&extended::page(path)), Some(&tab(&gettext("E_xtended"))));
    }
    if page_shown == Page::Permissions && permissions.is_some() {
        notebook.set_current_page(Some(1));
    }

    let Footer { buttons, recursive, close, apply: apply_button } = footer(any_dir, editable);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    content.append(&header(&paths));
    content.append(&notebook);
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&apply_button));

    let changes = {
        let (permissions, times, recursive) = (permissions.clone(), times.clone(), recursive.clone());
        Rc::new(move || -> AttributeChanges {
            let (mode, owner, group) = permissions.as_ref().map(|page| page.changes()).unwrap_or_default();
            let (modified, accessed) = times.as_ref().map(|page| page.changes()).unwrap_or_default();
            AttributeChanges { mode, owner, group, modified, accessed, recursive: recursive.is_active() }
        })
    };
    let update = {
        let (changes, apply_button) = (Rc::clone(&changes), apply_button.clone());
        move || apply_button.set_sensitive(!changes().is_empty())
    };
    if let Some(permissions) = &permissions {
        let update = update.clone();
        permissions.connect_changed(update);
    }
    if let Some(times) = &times {
        times.connect_changed(update);
    }
    close.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| window.close()
    ));
    apply_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            let changes = changes();
            if !changes.is_empty() {
                apply(paths.clone(), changes);
            }
            window.close();
        }
    ));
    window.connect_close_request(move |_| {
        cancel.store(true, Ordering::Relaxed);
        // The pages hold the widgets; they go with the window.
        let _ = (&permissions, &times);
        glib::Propagation::Proceed
    });
    dialogs::close_on_escape(&window);
    window.present();
}

/// The bottom of the window: "also the contents", Close, Apply.
struct Footer {
    buttons: gtk::Box,
    recursive: gtk::CheckButton,
    close: gtk::Button,
    apply: gtk::Button,
}

fn footer(any_dir: bool, editable: bool) -> Footer {
    let recursive = gtk::CheckButton::builder()
        .label(gettext("Also the _contents of directories"))
        .use_underline(true)
        .visible(any_dir && editable)
        .build();
    let close = gtk::Button::with_mnemonic(&pgettext("button", "_Close"));
    let apply = gtk::Button::with_mnemonic(&gettext("_Apply"));
    apply.add_css_class("suggested-action");
    apply.set_sensitive(false);
    apply.set_visible(editable);
    let buttons = gtk::Box::builder().spacing(8).build();
    buttons.append(&recursive);
    buttons.append(&gtk::Box::builder().hexpand(true).build());
    buttons.append(&close);
    buttons.append(&apply);
    Footer { buttons, recursive, close, apply }
}

fn tab(label: &str) -> gtk::Label {
    gtk::Label::builder().label(label).use_underline(true).build()
}
