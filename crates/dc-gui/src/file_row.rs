//! List model item of a panel row.
//!
//! Texts are fixed for the lifetime of the row; `marked` and `size-text`
//! are `GObject` properties, so cells bound to them update themselves.

use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::OsString;

use dc_core::{FileItem, format};
use gtk::glib;
use gtk::subclass::prelude::*;
use jiff::tz::TimeZone;

/// Pre-formatted column texts.
#[derive(Debug, Default)]
pub struct RowTexts {
    /// Without the extension, for the Name column next to Ext (details).
    pub name: String,
    /// With the extension, directories in brackets (list view).
    pub full_name: String,
    /// The name as on disk, for the file type and the preview.
    pub file_name: OsString,
    pub extension: String,
    pub size: String,
    pub modified: String,
    pub attributes: String,
    /// Directory, link to one, or `..`: the name is shown in bold.
    pub directory: bool,
}

mod imp {
    use super::{Cell, OnceCell, RefCell, RowTexts, glib};
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::FileRow)]
    pub struct FileRow {
        pub texts: OnceCell<RowTexts>,
        #[property(get, set)]
        marked: Cell<bool>,
        /// The Size column; changes when a directory's size is calculated.
        #[property(get, set)]
        size_text: RefCell<String>,
        /// Preview of an image in the icons view, once made.
        #[property(get, set, nullable)]
        thumbnail: RefCell<Option<gtk::gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileRow {
        const NAME: &'static str = "DcFileRow";
        type Type = super::FileRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FileRow {}
}

glib::wrapper! {
    pub struct FileRow(ObjectSubclass<imp::FileRow>);
}

impl FileRow {
    pub fn new(item: &FileItem, tz: &TimeZone) -> Self {
        let entry = item.entry();
        let texts = RowTexts {
            name: format::name_column(item),
            full_name: if item.is_directory_like() {
                format!("[{}]", entry.name.to_string_lossy())
            } else {
                entry.name.to_string_lossy().into_owned()
            },
            file_name: entry.name.clone(),
            extension: item.extension().to_owned(),
            size: format::size_column(item),
            modified: format::modified(entry.modified, tz),
            attributes: if item.is_parent() { String::new() } else { format::attributes(entry.mode, entry.kind) },
            directory: item.is_directory_like(),
        };
        let row: Self = glib::Object::builder()
            .property("marked", item.is_selected())
            .property("size-text", &texts.size)
            .build();
        row.imp().texts.set(texts).expect("texts are set only here");
        row
    }

    pub fn texts(&self) -> &RowTexts {
        self.imp().texts.get().expect("texts are set in FileRow::new")
    }
}
