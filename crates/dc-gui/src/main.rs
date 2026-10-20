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
mod panel_terminal;
mod panel_view;
mod progress_window;
mod rename_edit;
mod settings;
mod splitter;
mod texts;
mod theme;
mod toolbar;
mod window;
mod wrap_layout;

use std::path::PathBuf;

use gtk::prelude::*;
use gtk::{gdk, glib};

const APP_ID: &str = "cz.douby.DoubyCommander";

/// Translations compiled by `build.rs` from `po/*.po`, used when running
/// from the build directory.
const BUILD_LOCALE_DIR: &str = env!("DC_LOCALE_DIR");

/// Resource prefix of the bundled icons (see `resources/resources.gresource.xml`).
const ICONS: &str = "/cz/douby/DoubyCommander/icons";

fn main() -> glib::ExitCode {
    dc_i18n::init(&locale_dir());
    gtk::gio::resources_register_include!("dc-gui.gresource").expect("bundled resources are valid");
    // libadwaita: its stylesheet colors every widget through CSS variables,
    // which the palette of the theme sets (theme.rs).
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_style());
    app.connect_activate(|app| window::build(app.upcast_ref()));
    app.run()
}

/// Installed (`install.sh`, packages): `<prefix>/bin/doubycmd` finds its
/// translations in `<prefix>/share/locale`. Otherwise those of the build.
fn locale_dir() -> PathBuf {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .and_then(|exe| Some(exe.parent()?.parent()?.join("share/locale")))
        .filter(|dir| dir.join(format!("cs/LC_MESSAGES/{}.mo", dc_i18n::DOMAIN)).is_file());
    installed.unwrap_or_else(|| PathBuf::from(BUILD_LOCALE_DIR))
}

fn load_style() {
    let display = gdk::Display::default().expect("GTK is initialised, a display must exist");
    theme::install(&display);
    // Icons of the original (cm_refresh, drive-harddisk, ...) by name.
    gtk::IconTheme::for_display(&display).add_resource_path(ICONS);
    // The installed application icon, named after the application.
    gtk::Window::set_default_icon_name(APP_ID);
}
