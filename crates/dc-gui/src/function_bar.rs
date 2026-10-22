//! Row of function key buttons at the bottom of the window (`pnlKeys` of the
//! original): "View F3" ... "Exit Alt+X". Captions take the shortcut from
//! the keymap, so they stay right after rebinding.

use std::rc::Rc;
use std::time::Duration;

use dc_core::command::{Command, TrashChoice};
use dc_core::keymap::{Key, Shortcut};
use dc_i18n::{NC_, pgettext};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::commander::Commander;
use crate::texts;

/// Captions are translated when the buttons are built.
const BUTTONS: [((&str, &str), Command); 8] = [
    (NC_("function key", "View"), Command::QuickView { close: false }),
    (NC_("function key", "Open"), Command::ShellExecute),
    (NC_("function key", "Copy"), Command::Copy),
    (NC_("function key", "Move"), Command::Rename),
    (NC_("function key", "Directory"), Command::MakeDir),
    (NC_("function key", "Delete"), Command::Delete { trash: TrashChoice::Setting }),
    (NC_("function key", "Panel mode"), Command::CycleView),
    (NC_("function key", "Exit"), Command::Exit),
];

/// Startup wave: first button lights up this long after the window shows...
const WAVE_START: Duration = Duration::from_millis(400);
/// ...each next one this much later (the wave runs left to right)...
const WAVE_STEP: Duration = Duration::from_millis(70);
/// ...and stays lit this long. Fading in and out is a CSS transition
/// (`.dc-function-bar` in `theme.rs`).
const WAVE_HOLD: Duration = Duration::from_millis(280);

pub struct FunctionBar {
    bar: gtk::Box,
    buttons: Vec<gtk::Button>,
}

impl FunctionBar {
    pub fn widget(&self) -> &gtk::Box {
        &self.bar
    }

    /// Runs the wave over the buttons once, unless the system turned
    /// animations off (`gtk-enable-animations`).
    pub fn play_wave(&self) {
        if !gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations()) {
            return;
        }
        let mut lit_at = WAVE_START;
        for button in &self.buttons {
            let (on, off) = (button.clone(), button.clone());
            glib::timeout_add_local_once(lit_at, move || on.add_css_class("dc-wave"));
            glib::timeout_add_local_once(lit_at + WAVE_HOLD, move || off.remove_css_class("dc-wave"));
            lit_at += WAVE_STEP;
        }
    }
}

pub fn build(commander: &Rc<Commander>) -> FunctionBar {
    let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).homogeneous(true).build();
    bar.add_css_class("dc-function-bar");
    let mut buttons = Vec::with_capacity(BUTTONS.len());

    for ((context, caption), command) in BUTTONS {
        let caption = pgettext(context, caption);
        let button = gtk::Button::builder()
            // Clicking must not take the keyboard focus away from the panel.
            .focusable(false)
            .focus_on_click(false)
            .build();
        button.add_css_class("flat");

        let key = shortcut_caption(&commander.keymap().shortcuts(command));
        button.set_label(&texts::with_shortcut(&caption, &key));
        let commander = Rc::downgrade(commander);
        button.connect_clicked(move |_| {
            if let Some(commander) = commander.upgrade() {
                commander.execute(with_held_shift(command));
            }
        });
        bar.append(&button);
        buttons.push(button);
    }
    FunctionBar { bar, buttons }
}

/// The shortcut shown on a button: a plain function key if there is one
/// (F8 rather than Del), otherwise the first shortcut.
fn shortcut_caption(shortcuts: &[Shortcut]) -> String {
    shortcuts
        .iter()
        .find(|shortcut| shortcut.modifiers.is_empty() && matches!(shortcut.key, Key::F(_)))
        .or_else(|| shortcuts.first())
        .map(ToString::to_string)
        .unwrap_or_default()
}

/// Shift+click on "Delete" deletes the other way than the trash option says,
/// like Shift+F8 (`btnF8Click` of the original).
fn with_held_shift(command: Command) -> Command {
    let shift_held = gdk::Display::default()
        .and_then(|display| display.default_seat())
        .and_then(|seat| seat.keyboard())
        .is_some_and(|keyboard| keyboard.modifier_state().contains(gdk::ModifierType::SHIFT_MASK));
    match command {
        Command::Delete { .. } if shift_held => Command::Delete { trash: TrashChoice::ReverseSetting },
        other => other,
    }
}
