//! Localization through gettext, the standard of GTK applications.
//!
//! Rules for every crate:
//!
//! - Every text the user sees goes through [`gettext`]; counts through
//!   [`ngettext`] (Czech has three plural forms); words that could be
//!   translated in more ways get a context with [`pgettext`].
//! - Never build sentences from pieces. Variable parts are named
//!   placeholders `{name}` filled by [`fill`] after translation, so a
//!   translation may put them in any order.
//! - Strings in static tables are marked with [`N_`] (or [`NC_`] with a
//!   context) for extraction and translated with [`gettext`] ([`pgettext`])
//!   when shown.
//! - Command names (`cm_*`), shortcut texts and file names are not translated.
//!
//! `po/update-pot.sh` extracts the strings into `po/doubycmd.pot`.
//! English is the source language. The source texts follow the original
//! where it has the same text, so its translations (`language/*.po`) can
//! be reused.

use std::fmt::Display;
use std::path::Path;

pub use gettextrs::{gettext, ngettext, npgettext, pgettext};

/// Text domain: the name of the `.mo` files.
pub const DOMAIN: &str = "doubycmd";

/// Marks a string for extraction without translating it, for static tables
/// that are translated with [`gettext`] when shown.
#[expect(non_snake_case, reason = "N_ is the name gettext tools and translators know")]
#[must_use]
pub const fn N_(text: &'static str) -> &'static str {
    text
}

/// [`N_`] with a context, translated with [`pgettext`] when shown.
#[expect(non_snake_case, reason = "NC_ is the name gettext tools and translators know")]
#[must_use]
pub const fn NC_(context: &'static str, text: &'static str) -> (&'static str, &'static str) {
    (context, text)
}

/// Registers the catalogs in `locale_dir`
/// (`<dir>/<lang>/LC_MESSAGES/doubycmd.mo`). Call once at start.
///
/// The language itself comes from the locale, which GTK sets from the
/// environment when it initialises (`setlocale(LC_ALL, "")`); texts are
/// therefore translated only after GTK is up. (`setlocale` of `gettext-rs`
/// is `unsafe`, which this project does not allow.)
pub fn init(locale_dir: &Path) {
    // Failures only mean that the texts stay in English.
    let _ = gettextrs::bindtextdomain(DOMAIN, locale_dir);
    let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = gettextrs::textdomain(DOMAIN);
}

/// Replaces the named placeholders `{name}` of a translated `template`.
/// Unknown placeholders stay as they are, so a broken translation shows
/// what is wrong instead of losing text.
#[must_use]
pub fn fill(template: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut text = template.to_owned();
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

/// Count for [`ngettext`], which takes `u32`; larger counts use the plural
/// of the largest number, which is the right form in every language.
#[must_use]
pub fn plural_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_named_placeholders_in_any_order() {
        let text = fill("{count} files in {dir}", &[("dir", &"/tmp"), ("count", &3)]);
        assert_eq!(text, "3 files in /tmp");
        // A translation may reorder them.
        assert_eq!(fill("V {dir} je {count}", &[("count", &3), ("dir", &"/tmp")]), "V /tmp je 3");
        assert_eq!(fill("{unknown} stays", &[("count", &1)]), "{unknown} stays");
    }

    #[test]
    fn untranslated_text_is_the_source() {
        // Tests run without catalogs: the English source comes back.
        // (Variables, so that xgettext does not take these into the template.)
        let (one, many) = ("{count} test item", "{count} test items");
        assert_eq!(gettext(one), one);
        assert_eq!(ngettext(one, many, plural_count(2)), many);
        assert_eq!(plural_count(usize::MAX), u32::MAX);
    }
}
