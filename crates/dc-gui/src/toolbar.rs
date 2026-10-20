//! Main toolbar under the menu, the default one of the original
//! (`TfrmMain.CreateDefaultToolbar` in `src/fmain.pas`). Icons are those of
//! the original (`cm_<command>` in lower case); buttons of commands that do
//! not exist yet are disabled, as in the menu.

use std::rc::Rc;

use dc_core::command::Command;
use dc_i18n::{NC_, fill, pgettext};
use gtk::prelude::*;

use crate::commander::Commander;
use crate::texts;

enum Button {
    /// Hint (translation context and text) and command text.
    Command((&'static str, &'static str), &'static str),
    Separator,
}

use Button::Separator;

const BUTTONS: &[Button] = &[
    Button::Command(NC_("toolbar", "Refresh"), "cm_Refresh"),
    Button::Command(NC_("toolbar", "Options..."), "cm_Options"),
    Separator,
    // The ways a panel shows its directory; the terminal is one of them
    // here (the original has "Run Terminal" in the first group).
    Button::Command(NC_("toolbar", "List"), "cm_BriefView"),
    Button::Command(NC_("toolbar", "Details"), "cm_ColumnsView"),
    Button::Command(NC_("toolbar", "Icons"), "cm_ThumbnailsView"),
    Button::Command(NC_("toolbar", "Terminal"), "cm_TerminalView"),
    Separator,
    Button::Command(NC_("toolbar", "Flat view"), "cm_FlatView"),
    Separator,
    Button::Command(NC_("toolbar", "Go to previous entry in history"), "cm_ViewHistoryPrev"),
    Button::Command(NC_("toolbar", "Go to next entry in history"), "cm_ViewHistoryNext"),
    Separator,
    Button::Command(NC_("toolbar", "Select a Group..."), "cm_MarkPlus"),
    Button::Command(NC_("toolbar", "Unselect a Group..."), "cm_MarkMinus"),
    Button::Command(NC_("toolbar", "Invert Selection"), "cm_MarkInvert"),
    Separator,
    Button::Command(NC_("toolbar", "Pack Files..."), "cm_PackFiles"),
    Button::Command(NC_("toolbar", "Extract Files..."), "cm_ExtractFiles"),
    Separator,
    Button::Command(NC_("toolbar", "Network Connect..."), "cm_NetworkConnect"),
    Button::Command(NC_("toolbar", "Search..."), "cm_Search"),
    Button::Command(NC_("toolbar", "Multi-Rename Tool"), "cm_MultiRename"),
    Button::Command(NC_("toolbar", "Synchronize dirs..."), "cm_SyncDirs"),
    Button::Command(NC_("toolbar", "Copy Filename(s) with Full Path"), "cm_CopyFullNamesToClip"),
];

pub fn build(commander: &Rc<Commander>) -> gtk::Box {
    let bar = gtk::Box::builder().spacing(2).margin_start(2).build();
    bar.add_css_class("dc-bar");
    for button in BUTTONS {
        match button {
            Separator => bar.append(&gtk::Separator::new(gtk::Orientation::Vertical)),
            Button::Command((context, hint), command_text) => {
                bar.append(&command_button(commander, &pgettext(*context, *hint), command_text));
            }
        }
    }
    bar
}

fn command_button(commander: &Rc<Commander>, hint: &str, command_text: &str) -> gtk::Button {
    // Icon names of the original are the command names in lower case; our
    // own commands borrow one.
    let icon = match command_text {
        "cm_TerminalView" => "cm_runterm".to_owned(),
        _ => command_text.to_lowercase(),
    };
    let button = gtk::Button::builder().icon_name(icon).focusable(false).focus_on_click(false).build();
    button.add_css_class("flat");

    let Ok(command) = command_text.parse::<Command>() else {
        let tooltip = fill(
            &pgettext("tooltip of a disabled button", "{hint} - {reason}"),
            &[("hint", &hint), ("reason", &texts::not_implemented())],
        );
        button.set_tooltip_text(Some(&tooltip));
        button.set_sensitive(false);
        return button;
    };

    let shortcut = commander.keymap().shortcuts(command).first().map(ToString::to_string);
    let tooltip = match shortcut {
        Some(shortcut) => {
            fill(&pgettext("tooltip with shortcut", "{hint} ({shortcut})"), &[("hint", &hint), ("shortcut", &shortcut)])
        }
        None => hint.to_owned(),
    };
    button.set_tooltip_text(Some(&tooltip));
    let commander = Rc::downgrade(commander);
    button.connect_clicked(move |_| {
        if let Some(commander) = commander.upgrade() {
            commander.execute(command);
            commander.active().focus();
        }
    });
    button
}
