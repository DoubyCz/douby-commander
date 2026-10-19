mod commander;
mod copy_dialog;
mod dialogs;
mod file_row;
mod function_bar;
mod keys;
mod launch;
mod menu;
mod operations;
mod panel_column;
mod panel_view;
mod progress_window;
mod settings;
mod splitter;
mod texts;
mod theme;
mod toolbar;
mod window;

use gtk::prelude::*;
use gtk::{gdk, glib};

const APP_ID: &str = "cz.douby.DoubyCommander";

/// Translations compiled by `build.rs` from `po/*.po`. An installed
/// package will point this to the system locale directory instead.
const LOCALE_DIR: &str = env!("DC_LOCALE_DIR");

/// Resource prefix of the bundled icons (see `resources/resources.gresource.xml`).
const ICONS: &str = "/cz/douby/DoubyCommander/icons";

fn main() -> glib::ExitCode {
    dc_i18n::init(std::path::Path::new(LOCALE_DIR));
    gtk::gio::resources_register_include!("dc-gui.gresource").expect("bundled resources are valid");
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_style());
    app.connect_activate(window::build);
    app.run()
}

fn load_style() {
    let display = gdk::Display::default().expect("GTK is initialised, a display must exist");
    theme::install(&display);
    // Icons of the original (cm_refresh, drive-harddisk, ...) by name.
    gtk::IconTheme::for_display(&display).add_resource_path(ICONS);
}
