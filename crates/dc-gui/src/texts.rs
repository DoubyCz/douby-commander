//! Texts used in more places of the GUI, translated in one spot.

use dc_i18n::{fill, gettext, pgettext};

/// The application name is a name, it is not translated.
pub const APP_NAME: &str = "Douby Commander";

/// Tooltip of buttons and menu items whose command does not exist yet.
pub fn not_implemented() -> String {
    gettext("Not implemented yet")
}

/// "View F3": a caption with its shortcut, as on the function key buttons.
pub fn with_shortcut(caption: &str, shortcut: &str) -> String {
    if shortcut.is_empty() {
        return caption.to_owned();
    }
    fill(&pgettext("caption with shortcut", "{caption} {shortcut}"), &[("caption", &caption), ("shortcut", &shortcut)])
}
