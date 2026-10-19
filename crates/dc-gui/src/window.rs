//! Main window, laid out as the original's.

use std::path::PathBuf;
use std::rc::Rc;

use dc_core::keymap::Keymap;
use dc_vfs::mounts;
use gtk::glib;
use gtk::prelude::*;

use crate::commander::Commander;
use crate::operations::OperationsUi;
use crate::panel_view::PanelView;
use crate::settings::Settings;
use crate::splitter::Splitter;
use crate::{function_bar, menu, panel_column, texts, toolbar};

pub fn build(app: &gtk::Application) {
    let start_dir = glib::home_dir();
    let operations = OperationsUi::new();
    let commander = Rc::new(Commander::new(
        [PanelView::new(PathBuf::from(&start_dir)), PanelView::new(start_dir)],
        Keymap::with_defaults(),
        Settings::default(),
        Rc::clone(&operations),
    ));
    let weak = Rc::downgrade(&commander);
    operations.connect_finished(move || {
        if let Some(commander) = weak.upgrade() {
            commander.refresh_panels();
        }
    });

    let drives = mounts::mount_points();
    let columns = [panel_column::build(&commander, 0, &drives), panel_column::build(&commander, 1, &drives)];
    let splitter = Splitter::new(&columns[0], &columns[1]);
    commander.set_splitter(Rc::clone(&splitter));
    for (index, panel) in commander.panels().iter().enumerate() {
        let weak = Rc::downgrade(&commander);
        panel.connect_focus_enter(move || {
            if let Some(commander) = weak.upgrade() {
                commander.set_active(index);
            }
        });
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(texts::APP_NAME)
        .default_width(1200)
        .default_height(750)
        .build();

    // Layout of the original from top to bottom: menu, toolbar, panels (each
    // with drive bar, header and tabs), operations row, function keys.
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&menu::build(&window, &commander));
    content.append(&toolbar::build(&commander));
    content.append(splitter.widget());
    content.append(operations.row());
    let function_bar = function_bar::build(&commander);
    content.append(function_bar.widget());
    window.set_child(Some(&content));

    window.present();
    if commander.settings().startup_wave {
        function_bar.play_wave();
    }
    commander.set_active(0);
    // Only once the window is laid out: scrolling to the cursor earlier
    // leaves the list scrolled somewhere in the middle.
    let first_focus = Rc::clone(&commander);
    glib::idle_add_local_once(move || first_focus.active().focus());

    // Keys are caught before the list views see them, so commands win over
    // the default list behaviour. The window owns the commander through this
    // handler, everything else holds only weak references to it.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let released = Rc::downgrade(&commander);
    keys.connect_key_released(move |_, key, _, _| {
        if let Some(commander) = released.upgrade() {
            commander.handle_key_released(key);
        }
    });
    keys.connect_key_pressed(move |_, key, _, state| commander.handle_key(key, state));
    window.add_controller(keys);
}
