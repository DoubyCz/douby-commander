//! The "Extended" page of one item: what Linux keeps besides the
//! permissions. The flags of the file system (`lsattr`), the extended
//! attributes (`getfattr`) and the access control list (`getfacl`, when
//! it is installed).

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use dc_core::{program, properties};
use dc_i18n::{fill, gettext};
use gtk::prelude::*;

use super::grid_row;

/// The name of the extended attribute that holds an access control list.
const ACL: &str = "system.posix_acl_access";

pub(super) fn page(path: &Path) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 14);

    page.append(&heading(&gettext("Flags of the file system (lsattr)")));
    match properties::fs_flags(path) {
        Some(flags) if !flags.is_empty() => {
            let grid = gtk::Grid::builder().row_spacing(4).column_spacing(16).build();
            for (row, (letter, meaning)) in flags.iter().enumerate() {
                grid_row(&grid, i32::try_from(row).unwrap_or(0), &letter.to_string(), meaning);
            }
            page.append(&grid);
        }
        Some(_) => page.append(&dim(&gettext("None."))),
        None => page.append(&dim(&gettext("This file system keeps no flags for it."))),
    }

    let xattrs = properties::xattrs(path);
    page.append(&heading(&gettext("Extended attributes (xattr)")));
    let shown: Vec<_> = xattrs.iter().filter(|(name, _)| name != ACL).collect();
    if shown.is_empty() {
        page.append(&dim(&gettext("None.")));
    } else {
        let grid = gtk::Grid::builder().row_spacing(4).column_spacing(16).build();
        for (row, (name, value)) in shown.iter().enumerate() {
            grid_row(&grid, i32::try_from(row).unwrap_or(0), name, &properties::xattr_text(value));
        }
        page.append(&grid);
    }

    page.append(&heading(&gettext("Access control list (ACL)")));
    if xattrs.iter().any(|(name, _)| name == ACL) {
        page.append(&acl(path));
    } else {
        page.append(&dim(&gettext("None, the permissions alone decide.")));
    }
    page
}

/// What `getfacl` says, or that it would say it.
fn acl(path: &Path) -> gtk::Label {
    let output = program::find("getfacl").and_then(|getfacl| {
        let args: Vec<OsString> = vec!["--omit-header".into(), "--".into(), path.into()];
        program::run(&getfacl, &args, None, Duration::from_secs(5), 1 << 20, &AtomicBool::new(false))
    });
    match output {
        Some(output) => gtk::Label::builder()
            .label(String::from_utf8_lossy(&output).trim())
            .xalign(0.0)
            .selectable(true)
            .css_classes(["monospace"])
            .build(),
        None => dim(&fill(&gettext("It has one; {program} would show it."), &[("program", &"getfacl")])),
    }
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading"]).build()
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["dim-label"]).build()
}
