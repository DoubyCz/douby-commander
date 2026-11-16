//! The "General" page: what the items are, where, how big, when.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use dc_core::format;
use dc_core::properties::{self, Content, Details};
use dc_i18n::{fill, gettext, ngettext, plural_count};
use gtk::prelude::*;
use gtk::{gio, glib};
use jiff::tz::TimeZone;

use super::grid_row;

/// A size with its bytes: `1.2 M (1 234 567 bytes)`.
fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        return fill(&ngettext("{count} byte", "{count} bytes", plural_count(usize::try_from(bytes).unwrap_or(0))), &[("count", &bytes)]);
    }
    fill(&gettext("{size} ({bytes} bytes)"), &[("size", &format::size(bytes)), ("bytes", &format::group_thousands(bytes))])
}

fn time_text(time: SystemTime, tz: &TimeZone) -> String {
    format::modified(Some(time), tz)
}

/// What a set of items holds, for the eye.
fn content_text(content: &Content) -> String {
    let files = fill(&ngettext("{count} file", "{count} files", plural_count(usize::try_from(content.files).unwrap_or(usize::MAX))), &[("count", &content.files)]);
    let dirs = fill(
        &ngettext("{count} directory", "{count} directories", plural_count(usize::try_from(content.dirs).unwrap_or(usize::MAX))),
        &[("count", &content.dirs)],
    );
    fill(&gettext("{files}, {dirs}: {size}"), &[("files", &files), ("dirs", &dirs), ("size", &size_text(content.bytes))])
}

/// The page for one item.
pub(super) fn single(path: &Path, details: &Details, cancel: &Arc<AtomicBool>) -> gtk::Grid {
    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(16).build();
    let tz = TimeZone::system();
    let row = std::cell::Cell::new(0);
    let add = |title: &str, value: &str| {
        grid_row(&grid, row.get(), title, value);
        row.set(row.get() + 1);
    };
    add(&gettext("Location:"), &path.parent().map(|dir| dir.display().to_string()).unwrap_or_default());
    let info = gio::File::for_path(path)
        .query_info("standard::content-type", gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, None::<&gio::Cancellable>)
        .ok();
    if let Some(info) = &info {
        let content_type = info.content_type().unwrap_or_default();
        let mime = gio::content_type_get_mime_type(&content_type).map_or_else(|| content_type.to_string(), |mime| mime.to_string());
        let description = gio::content_type_get_description(&content_type).to_string();
        add(&gettext("Type:"), &fill(&gettext("{description} ({mime})"), &[("description", &description), ("mime", &mime)]));
    }
    if let Some((target, works)) = &details.link {
        let target = target.display().to_string();
        let text = if *works { target } else { fill(&gettext("{target} (does not exist)"), &[("target", &target)]) };
        add(&gettext("Link to:"), &text);
    }
    let content_row = if details.is_dir() {
        add(&gettext("Contains:"), &gettext("counting..."));
        Some(row.get() - 1)
    } else {
        add(&gettext("Size:"), &size_text(details.size));
        add(&gettext("On disk:"), &size_text(details.on_disk));
        None
    };
    if let Some((point, device, fs_type)) = properties::mount_of(path) {
        add(
            &gettext("File system:"),
            &fill(&gettext("{point} ({fs_type}, {device})"), &[("point", &point.display()), ("fs_type", &fs_type), ("device", &device)]),
        );
    }
    add(&gettext("Inode:"), &fill(&gettext("{inode} on device {major}:{minor}"), &[
        ("inode", &details.inode),
        ("major", &details.device.0),
        ("minor", &details.device.1),
    ]));
    add(&gettext("Hard links:"), &details.links.to_string());
    if let Some(born) = details.born {
        add(&gettext("Created:"), &time_text(born, &tz));
    }
    add(&gettext("Modified:"), &time_text(details.modified, &tz));
    add(&gettext("Status changed:"), &time_text(details.changed, &tz));
    add(&gettext("Accessed:"), &time_text(details.accessed, &tz));
    if let Some(row) = content_row {
        // What is inside, not the directory itself.
        let inside: Vec<PathBuf> =
            std::fs::read_dir(path).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default();
        count(&grid, row, inside, cancel);
    }
    grid
}

/// The page for several items.
pub(super) fn several(paths: &[PathBuf], cancel: &Arc<AtomicBool>) -> gtk::Grid {
    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(16).build();
    let dirs = paths.iter().filter(|path| std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())).count();
    let files = paths.len() - dirs;
    let items = fill(
        &ngettext("{count} item: {files} files, {dirs} directories", "{count} items: {files} files, {dirs} directories", plural_count(paths.len())),
        &[("count", &paths.len()), ("files", &files), ("dirs", &dirs)],
    );
    grid_row(&grid, 0, &gettext("Selected:"), &items);
    let common = paths.first().and_then(|first| first.parent()).filter(|dir| paths.iter().all(|path| path.parent() == Some(*dir)));
    let location = common.map_or_else(|| gettext("various directories"), |dir| dir.display().to_string());
    grid_row(&grid, 1, &gettext("Location:"), &location);
    grid_row(&grid, 2, &gettext("Contains:"), &gettext("counting..."));
    count(&grid, 2, paths.to_vec(), cancel);
    grid
}

/// Counts `paths` in a worker thread and writes the result into `row`
/// (and the space on disk into the row after it).
fn count(grid: &gtk::Grid, row: i32, paths: Vec<PathBuf>, cancel: &Arc<AtomicBool>) {
    let cancel = Arc::clone(cancel);
    let grid = grid.downgrade();
    glib::spawn_future_local(async move {
        let stop = Arc::clone(&cancel);
        let Ok(content) = gio::spawn_blocking(move || properties::content(&paths, &stop)).await else { return };
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Some(grid) = grid.upgrade() else { return };
        if let Some(value) = grid.child_at(1, row).and_downcast::<gtk::Label>() {
            value.set_text(&content_text(&content));
        }
        grid.insert_row(row + 1);
        grid_row(&grid, row + 1, &gettext("On disk:"), &size_text(content.on_disk));
    });
}

/// The page shows these, also for an item that cannot be read now.
pub(super) fn unreadable(path: &Path, error: &std::io::Error) -> gtk::Grid {
    let grid = gtk::Grid::builder().row_spacing(6).column_spacing(16).build();
    grid_row(&grid, 0, &gettext("Location:"), &path.display().to_string());
    grid_row(&grid, 1, &gettext("Error:"), &error.to_string());
    grid
}
