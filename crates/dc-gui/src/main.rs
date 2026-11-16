mod about;
mod addons;
mod audio_stream;
mod commander;
mod copy_dialog;
mod dialogs;
mod document_view;
mod find_files;
mod file_row;
mod function_bar;
mod history_entry;
mod keys;
mod launch;
mod menu;
mod multi_rename;
mod music_view;
mod operations;
mod panel_column;
mod panel_terminal;
mod panel_view;
mod path_edit;
mod pixels;
mod progress_window;
mod quick_details;
mod quick_view;
mod rename_edit;
mod settings;
mod splitter;
mod texts;
mod theme;
mod toolbar;
mod wave;
mod window;
mod wrap_layout;

use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::rc::Weak;

use dc_core::startup::{self, Startup};
use dc_i18n::{fill, gettext};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::commander::Commander;

const APP_ID: &str = "cz.douby.DoubyCommander";

/// Translations compiled by `build.rs` from `po/*.po`, used when running
/// from the build directory.
const BUILD_LOCALE_DIR: &str = env!("DC_LOCALE_DIR");

/// Resource prefix of the bundled icons (see `resources/resources.gresource.xml`).
const ICONS: &str = "/cz/douby/DoubyCommander/icons";

fn main() -> glib::ExitCode {
    // The locale of the environment, which GTK sets as it starts: the help
    // of the command line is made before the application starts GTK. No
    // display (`--help` over ssh) only fails here, the help still works.
    let _ = gtk::init();
    dc_i18n::init(&locale_dir());
    gtk::gio::resources_register_include!("dc-gui.gresource").expect("bundled resources are valid");
    // libadwaita: its stylesheet colors every widget through CSS variables,
    // which the palette of the theme sets (theme.rs).
    // One instance: a second `doubycmd PATH` hands its command line to the
    // running one, whose panels go there.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    add_options(&app);
    app.connect_startup(|_| load_style());
    let commander: RefCell<Weak<Commander>> = RefCell::default();
    app.connect_command_line(move |app, command_line| {
        let startup = match read_command_line(command_line) {
            Ok(startup) => startup,
            Err(err) => {
                command_line.printerr_literal(&format!("{err}\n"));
                return glib::ExitCode::FAILURE;
            }
        };
        let running = commander.borrow().upgrade();
        match running {
            Some(running) => {
                running.apply_startup(&startup);
                if let Some(window) = running.active().window() {
                    window.present();
                }
            }
            None => *commander.borrow_mut() = window::build(app.upcast_ref(), &startup),
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}

/// The options of the original (`ucmdlineparams.pas`), `--help` lists them.
fn add_options(app: &adw::Application) {
    app.set_option_context_parameter_string(Some(&gettext("[PATH] | [LEFT RIGHT]")));
    app.set_option_context_summary(Some(&gettext(
        "A path alone is shown in the active panel (the left one at start), two paths in the left and the right panel.",
    )));
    let path = gettext("PATH");
    let options = [
        ("left", b'L', glib::OptionArg::Filename, gettext("Path for the left panel"), path.as_str()),
        ("right", b'R', glib::OptionArg::Filename, gettext("Path for the right panel"), path.as_str()),
        ("active-panel", b'P', glib::OptionArg::String, gettext("Active panel: L (left) or R (right)"), "L|R"),
    ];
    for (name, short, arg, description, value) in options {
        app.add_main_option(name, glib::Char::from(short), glib::OptionFlags::NONE, arg, &description, Some(value));
    }
}

/// The command line as panels and paths. Paths are relative to where the
/// command was run (a second instance too), `file://` URIs are taken.
fn read_command_line(command_line: &gtk::gio::ApplicationCommandLine) -> Result<Startup<PathBuf>, String> {
    let options = command_line.options_dict();
    let path = |arg: &OsStr| {
        command_line
            .create_file_for_arg(arg)
            .path()
            .ok_or_else(|| fill(&gettext("Not a local path: {path}"), &[("path", &arg.to_string_lossy())]))
    };
    let option = |name: &str| options.lookup::<PathBuf>(name).ok().flatten().map(|arg| path(arg.as_os_str())).transpose();
    let (left, right) = (option("left")?, option("right")?);
    let active: Option<String> = options.lookup("active-panel").ok().flatten();
    // The first argument is the program.
    let paths = command_line.arguments().iter().skip(1).map(|arg| path(arg)).collect::<Result<Vec<_>, _>>()?;
    startup::arrange(left, right, active.as_deref(), paths).map_err(|err| err.to_string())
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
