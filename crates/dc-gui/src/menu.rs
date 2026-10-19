//! Main menu, as `mnuMain` of the original (`src/fmain.lfm`).
//!
//! Items name their command (`cm_*`). Items whose command exists run it
//! through the commander and show its shortcut from the keymap; the others
//! are shown disabled and come alive by themselves once the command is
//! implemented.

use std::rc::Rc;

use dc_core::command::Command;
use dc_i18n::{NC_, pgettext};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::commander::Commander;
use crate::keys;

/// Caption with its translation context, see [`NC_`].
type Caption = (&'static str, &'static str);

enum Entry {
    /// Caption and command text (`cm_Delete trashcan=...` allowed).
    Item(Caption, &'static str),
    Separator,
    Submenu(Caption, &'static [Entry]),
}

use Entry::{Item, Separator, Submenu};

/// Window action running the command given as its string parameter.
const RUN_ACTION: &str = "run-command";
/// Disabled window action of items whose command does not exist yet.
const NOT_YET_ACTION: &str = "not-implemented";

const MENU: &[(Caption, &[Entry])] = &[
    (
        NC_("main menu", "_Files"),
        &[
            Item(NC_("main menu", "Create Symbolic _Link..."), "cm_SymLink"),
            Item(NC_("main menu", "Create _Hard Link..."), "cm_HardLink"),
            Item(NC_("main menu", "Create _Directory"), "cm_MakeDir"),
            Separator,
            Item(NC_("main menu", "Change _Attributes..."), "cm_SetFileProperties"),
            Item(NC_("main menu", "Show _File Properties"), "cm_FileProperties"),
            Item(NC_("main menu", "Edit Co_mment..."), "cm_EditComment"),
            Item(NC_("main menu", "Calculate _Occupied Space"), "cm_CalculateSpace"),
            Item(NC_("main menu", "Compare by _Contents"), "cm_CompareContents"),
            Item(NC_("main menu", "Multi-_Rename Tool"), "cm_MultiRename"),
            Separator,
            Item(NC_("main menu", "_Pack Files..."), "cm_PackFiles"),
            Item(NC_("main menu", "_Extract Files..."), "cm_ExtractFiles"),
            Item(NC_("main menu", "_Test Archive(s)"), "cm_TestArchive"),
            Item(NC_("main menu", "Spl_it File..."), "cm_FileSpliter"),
            Item(NC_("main menu", "Com_bine Files..."), "cm_FileLinker"),
            Item(NC_("main menu", "Calculate Check_sum..."), "cm_CheckSumCalc"),
            Item(NC_("main menu", "_Verify Checksum..."), "cm_CheckSumVerify"),
            Separator,
            Item(NC_("main menu", "Wipe"), "cm_Wipe"),
            Item(NC_("main menu", "Delete"), "cm_Delete"),
            Separator,
            Item(NC_("main menu", "E_xit"), "cm_Exit"),
        ],
    ),
    (
        NC_("main menu", "_Mark"),
        &[
            Item(NC_("main menu", "Select a _Group..."), "cm_MarkPlus"),
            Item(NC_("main menu", "Unselect a Gro_up..."), "cm_MarkMinus"),
            Item(NC_("main menu", "_Select All"), "cm_MarkMarkAll"),
            Item(NC_("main menu", "_Unselect All"), "cm_MarkUnmarkAll"),
            Item(NC_("main menu", "_Invert Selection"), "cm_MarkInvert"),
            Item(NC_("main menu", "Select All with the Same E_xtension"), "cm_MarkCurrentExtension"),
            Item(NC_("main menu", "Unselect All with the Same Ex_tension"), "cm_UnmarkCurrentExtension"),
            Separator,
            Item(NC_("main menu", "Sa_ve Selection"), "cm_SaveSelection"),
            Item(NC_("main menu", "_Restore Selection"), "cm_RestoreSelection"),
            Item(NC_("main menu", "Save S_election to File..."), "cm_SaveSelectionToFile"),
            Item(NC_("main menu", "_Load Selection from File..."), "cm_LoadSelectionFromFile"),
            Item(NC_("main menu", "Load Selection from Clip_board"), "cm_LoadSelectionFromClip"),
            Separator,
            Item(NC_("main menu", "Copy _Filename(s) to Clipboard"), "cm_CopyNamesToClip"),
            Item(NC_("main menu", "Copy Filename(s) with Full _Path"), "cm_CopyFullNamesToClip"),
            Item(NC_("main menu", "Copy all shown _columns"), "cm_CopyFileDetailsToClip"),
            Separator,
            Item(NC_("main menu", "Compare Directories"), "cm_CompareDirectories"),
        ],
    ),
    (
        NC_("main menu", "_Commands"),
        &[
            Item(NC_("main menu", "_Search..."), "cm_Search"),
            Item(NC_("main menu", "New search instance..."), "cm_AddNewSearch"),
            Item(NC_("main menu", "View current search instances"), "cm_ViewSearches"),
            Item(NC_("main menu", "Directory _Hotlist"), "cm_DirHotList"),
            Item(NC_("main menu", "Syn_chronize dirs..."), "cm_SyncDirs"),
            Separator,
            Item(NC_("main menu", "Run _Terminal"), "cm_RunTerm"),
            Item(NC_("main menu", "Execute _internal command..."), "cm_DoAnyCmCommand"),
            Separator,
            Item(NC_("main menu", "_Flat view"), "cm_FlatView"),
            Item(NC_("main menu", "Open _VFS List"), "cm_OpenVirtualFileSystemList"),
            // The original opens the stash through the VFS list, not by a command of its own.
            Item(NC_("main menu", "Open Stash"), ""),
            Item(NC_("main menu", "Swap _Panels"), "cm_Exchange"),
            Item(NC_("main menu", "Target _= Source"), "cm_TargetEqualSource"),
            Separator,
            Item(NC_("main menu", "Sho_w Occupied Space"), "cm_CountDirContent"),
        ],
    ),
    (
        NC_("main menu", "_Network"),
        &[
            Item(NC_("main menu", "Network _Connect..."), "cm_NetworkConnect"),
            Item(NC_("main menu", "Network _Disconnect"), "cm_NetworkDisconnect"),
        ],
    ),
    (
        NC_("main menu", "_Tabs"),
        &[
            Item(NC_("main menu", "_New Tab"), "cm_NewTab"),
            Item(NC_("main menu", "_Rename Tab"), "cm_RenameTab"),
            Item(NC_("main menu", "Open _Folder in a New Tab"), "cm_OpenDirInNewTab"),
            Separator,
            Item(NC_("main menu", "_Close Tab"), "cm_CloseTab"),
            Item(NC_("main menu", "Close _All Tabs"), "cm_CloseAllTabs"),
            Item(NC_("main menu", "Close Duplicate Tabs"), "cm_CloseDuplicateTabs"),
            Separator,
            Submenu(
                NC_("main menu", "Tab _Options"),
                &[
                    Item(NC_("main menu", "_Normal"), "cm_SetTabOptionNormal"),
                    Item(NC_("main menu", "_Locked"), "cm_SetTabOptionPathLocked"),
                    Item(NC_("main menu", "Locked with _Directory Changes Allowed"), "cm_SetTabOptionPathResets"),
                    Item(NC_("main menu", "Locked with Directories Opened in New _Tabs"), "cm_SetTabOptionDirsInNewTab"),
                    Separator,
                    Item(NC_("main menu", "Set all tabs to Normal"), "cm_SetAllTabsOptionNormal"),
                    Item(NC_("main menu", "Set all tabs to Locked"), "cm_SetAllTabsOptionPathLocked"),
                    Item(NC_("main menu", "All tabs Locked with Dir Changes Allowed"), "cm_SetAllTabsOptionPathResets"),
                    Item(NC_("main menu", "All tabs Locked with Dir Opened in New Tabs"), "cm_SetAllTabsOptionDirsInNewTab"),
                ],
            ),
            Separator,
            Item(NC_("main menu", "Switch to Nex_t Tab"), "cm_NextTab"),
            Item(NC_("main menu", "Switch to _Previous Tab"), "cm_PrevTab"),
            Separator,
            Item(NC_("main menu", "_Save Tabs to File"), "cm_SaveTabs"),
            Item(NC_("main menu", "_Load Tabs from File"), "cm_LoadTabs"),
            Item(NC_("main menu", "Save current tabs to a New Favorite Tabs"), "cm_SaveFavoriteTabs"),
            Item(NC_("main menu", "Load tabs from Favorite Tabs"), "cm_LoadFavoriteTabs"),
            Separator,
            Item(NC_("main menu", "Configuration of folder tabs"), "cm_ConfigFolderTabs"),
            Item(NC_("main menu", "Configuration of Favorite Tabs"), "cm_ConfigFavoriteTabs"),
        ],
    ),
    (
        NC_("main menu", "F_avorites"),
        &[
            Item(NC_("main menu", "Save current tabs to a New Favorite Tabs"), "cm_SaveFavoriteTabs"),
            Item(NC_("main menu", "Resave on the last Favorite Tabs loaded"), "cm_ResaveFavoriteTabs"),
            Item(NC_("main menu", "Reload the last Favorite Tabs loaded"), "cm_ReloadFavoriteTabs"),
            Item(NC_("main menu", "Configuration of Favorite Tabs"), "cm_ConfigFavoriteTabs"),
            // The saved favorite tabs follow here once they exist.
        ],
    ),
    (
        NC_("main menu", "_Show"),
        &[
            // "Brief view", "Full", "Thumbnails" in the original.
            Item(NC_("main menu", "_List"), "cm_BriefView"),
            Item(NC_("main menu", "_Details"), "cm_ColumnsView"),
            Item(NC_("main menu", "_Icons"), "cm_ThumbnailsView"),
            Separator,
            Item(NC_("main menu", "_Quick View Panel"), "cm_QuickView"),
            Item(NC_("main menu", "_Tree View Panel"), "cm_TreeView"),
            Separator,
            Item(NC_("main menu", "Sort by _Name"), "cm_SortByName"),
            Item(NC_("main menu", "Sort by _Extension"), "cm_SortByExt"),
            Item(NC_("main menu", "Sort by _Size"), "cm_SortBySize"),
            Item(NC_("main menu", "Sort by _Date"), "cm_SortByDate"),
            Item(NC_("main menu", "Sort by _Attributes"), "cm_SortByAttr"),
            Separator,
            Item(NC_("main menu", "Re_verse Order"), "cm_ReverseOrder"),
            Item(NC_("main menu", "_Refresh"), "cm_Refresh"),
            Separator,
            Item(NC_("main menu", "Show _Hidden/System Files"), "cm_ShowSysFiles"),
            Separator,
            Item(NC_("main menu", "_Horizontal Panels Mode"), "cm_HorizontalFilePanels"),
            Separator,
            Item(NC_("main menu", "Operations _Viewer"), "cm_OperationsViewer"),
        ],
    ),
    (
        NC_("main menu", "C_onfiguration"),
        &[
            Item(NC_("main menu", "_Options..."), "cm_Options"),
            Separator,
            Item(NC_("main menu", "Configuration of Directory Hotlist"), "cm_ConfigDirHotList"),
            Item(NC_("main menu", "Configuration of Favorite Tabs"), "cm_ConfigFavoriteTabs"),
            Item(NC_("main menu", "Configuration of File _Associations"), "cm_FileAssoc"),
            Item(NC_("main menu", "Configuration of folder tabs"), "cm_ConfigFolderTabs"),
            Item(NC_("main menu", "Configuration of Archivers"), "cm_ConfigArchivers"),
            Separator,
            Item(NC_("main menu", "Save Position"), "cm_ConfigSavePos"),
            Item(NC_("main menu", "Save Settings"), "cm_ConfigSaveSettings"),
        ],
    ),
    (
        NC_("main menu", "_Help"),
        &[
            Item(NC_("main menu", "_Contents"), "cm_HelpIndex"),
            Item(NC_("main menu", "_Keyboard"), "cm_Keyboard"),
            Item(NC_("main menu", "_Visit Douby Commander Website"), "cm_VisitHomePage"),
            Separator,
            Item(NC_("main menu", "_About"), "cm_About"),
        ],
    ),
];

/// The menu bar. Adds the actions its items use to `window`.
pub fn build(window: &gtk::ApplicationWindow, commander: &Rc<Commander>) -> gtk::PopoverMenuBar {
    install_actions(window, commander);
    let model = gio::Menu::new();
    for ((context, caption), entries) in MENU {
        model.append_submenu(Some(&pgettext(*context, *caption)), &menu_model(entries, commander));
    }
    let bar = gtk::PopoverMenuBar::from_model(Some(&model));
    return_focus_on_close(&bar, commander);
    bar
}

/// However a menu closes (Esc, a click outside, an item chosen), the active
/// panel gets the keyboard focus back. Each item of the bar has a popover of
/// its own; after it closes GTK hands the focus to the bar, so ours comes
/// right after that.
fn return_focus_on_close(bar: &gtk::PopoverMenuBar, commander: &Rc<Commander>) {
    let mut pending = vec![bar.clone().upcast::<gtk::Widget>()];
    while let Some(widget) = pending.pop() {
        if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
            let commander = Rc::downgrade(commander);
            popover.connect_closed(move |_| {
                let commander = commander.clone();
                glib::idle_add_local_once(move || {
                    if let Some(commander) = commander.upgrade() {
                        commander.active().focus();
                    }
                });
            });
            // Submenus live inside the same popover.
            continue;
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            pending.push(current);
        }
    }
}

/// Items between separators go into sections, which GTK draws separated.
fn menu_model(entries: &[Entry], commander: &Commander) -> gio::Menu {
    let menu = gio::Menu::new();
    let mut section = gio::Menu::new();
    for entry in entries {
        match entry {
            Item((context, caption), command) => section.append_item(&menu_item(&pgettext(*context, *caption), command, commander)),
            Separator => {
                menu.append_section(None, &section);
                section = gio::Menu::new();
            }
            Submenu((context, caption), entries) => {
                section.append_submenu(Some(&pgettext(*context, *caption)), &menu_model(entries, commander));
            }
        }
    }
    menu.append_section(None, &section);
    menu
}

fn menu_item(caption: &str, command_text: &str, commander: &Commander) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(caption), None);
    match command_text.parse::<Command>() {
        Ok(command) => {
            item.set_action_and_target_value(Some(&format!("win.{RUN_ACTION}")), Some(&command_text.to_variant()));
            if let Some(accel) = commander.keymap().shortcuts(command).iter().find_map(keys::accelerator) {
                item.set_attribute_value("accel", Some(&accel.to_variant()));
            }
        }
        Err(_) => item.set_detailed_action(&format!("win.{NOT_YET_ACTION}")),
    }
    item
}

fn install_actions(window: &gtk::ApplicationWindow, commander: &Rc<Commander>) {
    let run = gio::SimpleAction::new(RUN_ACTION, Some(glib::VariantTy::STRING));
    let weak = Rc::downgrade(commander);
    run.connect_activate(move |_, parameter| {
        let command = parameter.and_then(glib::Variant::str).and_then(|text| text.parse::<Command>().ok());
        if let (Some(commander), Some(command)) = (weak.upgrade(), command) {
            commander.execute(command);
            commander.active().focus();
        }
    });
    window.add_action(&run);

    let not_yet = gio::SimpleAction::new(NOT_YET_ACTION, None);
    not_yet.set_enabled(false);
    window.add_action(&not_yet);
}
