//! Look of the application: the base style, and a palette from the desktop
//! theme that is followed live.
//!
//! - Light/dark mode and icons come from GTK itself (`color-scheme`,
//!   `gtk-theme`, `icon-theme`), which switches them in running apps.
//! - Widgets are libadwaita's, colored through its CSS variables
//!   (`--accent-bg-color`, `--window-bg-color`, ...). Our own colors are
//!   variables too (`--dc-*`) and default to libadwaita's, never to fixed
//!   values.
//! - A palette stylesheet sets these variables on `:root`. On Omarchy
//!   it is generated from the theme's colors by the template
//!   `extras/omarchy/doubycmd.css.tpl` on every theme change; its
//!   directory is watched and the stylesheet reloaded right away.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Base style. Colors only through CSS variables: those of libadwaita
/// (`--accent-bg-color`, `--view-bg-color`, ...) and ours (`--dc-*`), which
/// default to libadwaita's. A palette redefines them on `:root`.
const BASE_CSS: &str = "
:root {
    --dc-marked-color: var(--red-3);
    --dc-cursor-bg-color: color-mix(in srgb, var(--accent-bg-color) 45%, var(--view-bg-color));
    --dc-cursor-fg-color: var(--view-fg-color);
    --dc-active-path-bg-color: var(--accent-bg-color);
    --dc-active-path-fg-color: var(--accent-fg-color);
    --dc-inactive-cursor-color: var(--accent-bg-color);
    --dc-current-drive-color: color-mix(in srgb, var(--accent-bg-color) 35%, transparent);
    --dc-menu-border-color: var(--accent-bg-color);
    --dc-menu-item-bg-color: color-mix(in srgb, var(--accent-bg-color) 30%, var(--popover-bg-color));
    --dc-wave-bg-color: var(--accent-bg-color);
    --dc-wave-fg-color: var(--accent-fg-color);
}

/* Startup wave over the function keys (function_bar.rs): a button with
   .dc-wave is shown inverted, the transition fades it in and out. */
.dc-function-bar button {
    transition: background-color 250ms ease-in-out, color 250ms ease-in-out;
}
.dc-function-bar button.dc-wave,
.dc-function-bar button.dc-wave:disabled {
    background-image: none;
    background-color: var(--dc-wave-bg-color);
    color: var(--dc-wave-fg-color);
    /* Disabled buttons are dimmed; the wave lights all alike. */
    filter: none;
    opacity: 1;
}

.dc-path { padding: 2px 6px; }
.dc-active .dc-path { background: var(--dc-active-path-bg-color); color: var(--dc-active-path-fg-color); }
.dc-panel columnview > listview > row:selected {
    background-color: var(--dc-cursor-bg-color);
    color: var(--dc-cursor-fg-color);
}
.dc-panel:not(.dc-active) columnview > listview > row:selected {
    background-color: transparent;
    color: inherit;
    box-shadow: inset 0 0 0 1px var(--dc-inactive-cursor-color);
}
/* The cursor is the highlighted row, as in the original. The focus ring of
   the GTK theme (accent colored and animated, so it trails behind the
   cursor) is not drawn on rows and cells. */
.dc-panel columnview > listview > row,
.dc-panel columnview > listview > row > cell {
    outline: none;
    transition: none;
}
/* List view: rows tight (padding as LIST_CELL_PADDING and
   LIST_CELL_SIDE_PADDING in panel_view.rs, which also sets the 20rem
   width), the cursor as in details. */
.dc-list-view > child {
    min-height: 0;
    padding: 1px 6px;
    margin: 0;
    border-radius: 0;
    outline: none;
    transition: none;
}
.dc-panel .dc-list-view > child:selected {
    background-color: var(--dc-cursor-bg-color);
    color: var(--dc-cursor-fg-color);
}
.dc-panel:not(.dc-active) .dc-list-view > child:selected {
    background-color: transparent;
    color: inherit;
    box-shadow: inset 0 0 0 1px var(--dc-inactive-cursor-color);
}
/* Icons view: the cursor and marks as in the other views. */
.dc-icon-view > child { padding: 6px; border-radius: 6px; outline: none; transition: none; }
.dc-panel .dc-icon-view > child:selected {
    background-color: var(--dc-cursor-bg-color);
    color: var(--dc-cursor-fg-color);
}
.dc-panel:not(.dc-active) .dc-icon-view > child:selected {
    background-color: transparent;
    color: inherit;
    box-shadow: inset 0 0 0 1px var(--dc-inactive-cursor-color);
}
/* The arrow of the sort column, in both headers, in the accent. */
.dc-panel columnview > header > button sort-indicator { color: var(--accent-bg-color); }
.dc-marked { color: var(--dc-marked-color); }
/* A menu dropped from the menu bar hangs from its item: sharp corner there,
   framed in the accent. */
menubar > item > popover > contents {
    border-top-left-radius: 0;
    border: 1px solid var(--dc-menu-border-color);
}
/* The open or hovered menu bar item: tinted, no underline. */
menubar > item:hover,
menubar > item:selected,
menubar > item.active,
menubar > item:focus-visible {
    background-color: var(--dc-menu-item-bg-color);
    color: var(--window-fg-color);
    box-shadow: none;
}
.dc-status { padding: 2px 6px; }
.dc-bar button { padding: 2px 4px; min-height: 0; }
.dc-current-drive { background: var(--dc-current-drive-color); }
.dc-tab { padding: 2px 10px; border-radius: 4px 4px 0 0; background: color-mix(in srgb, currentColor 8%, transparent); }
.dc-active .dc-tab { background: var(--dc-current-drive-color); }
";

/// Name of the palette stylesheet the Omarchy template produces.
const PALETTE_FILE: &str = "doubycmd.css";

/// The same template, built in: used when the user has not installed it in
/// Omarchy, so the application follows the theme without any setup.
const PALETTE_TEMPLATE: &str = include_str!("../../../extras/omarchy/doubycmd.css.tpl");

/// Wait after the first change before reloading: a theme switch replaces a
/// whole directory, which comes as a burst of events.
const RELOAD_DELAY: Duration = Duration::from_millis(300);

/// Keeps the palette provider and the directory monitor alive.
struct Palette {
    provider: gtk::CssProvider,
    theme_dir: PathBuf,
    _monitor: Option<gio::FileMonitor>,
    reload_pending: bool,
}

thread_local! {
    static PALETTE: RefCell<Option<Palette>> = const { RefCell::new(None) };
}

/// Installs the base style and the followed palette for `display`.
pub fn install(display: &gdk::Display) {
    let base = gtk::CssProvider::new();
    base.load_from_string(BASE_CSS);
    gtk::style_context_add_provider_for_display(display, &base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let Some(theme_dir) = omarchy_theme_dir() else { return };
    let provider = gtk::CssProvider::new();
    // Above the base style, below the user's own gtk.css.
    gtk::style_context_add_provider_for_display(display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

    // Omarchy swaps the whole theme directory, so its parent is watched.
    let monitor = theme_dir
        .parent()
        .and_then(|parent| gio::File::for_path(parent).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>).ok());
    if let Some(monitor) = &monitor {
        monitor.connect_changed(|_, _, _, _| schedule_reload());
    }

    let palette = Palette { provider, theme_dir, _monitor: monitor, reload_pending: false };
    load(&palette);
    PALETTE.with_borrow_mut(|slot| *slot = Some(palette));
}

fn schedule_reload() {
    let already = PALETTE.with_borrow_mut(|slot| match slot {
        Some(palette) => std::mem::replace(&mut palette.reload_pending, true),
        None => true,
    });
    if already {
        return;
    }
    glib::timeout_add_local_once(RELOAD_DELAY, || {
        PALETTE.with_borrow_mut(|slot| {
            if let Some(palette) = slot {
                palette.reload_pending = false;
                load(palette);
            }
        });
    });
}

/// Loads the palette: the stylesheet Omarchy rendered from the user's
/// template if there is one, else the built-in template filled with the
/// theme's colors; nothing (the GTK theme's colors) without a theme.
fn load(palette: &Palette) {
    let rendered = palette.theme_dir.join(PALETTE_FILE);
    if rendered.is_file() {
        palette.provider.load_from_path(&rendered);
        return;
    }
    match theme_colors(&palette.theme_dir.join("colors.toml")) {
        Some(colors) => palette.provider.load_from_string(&render(PALETTE_TEMPLATE, &colors)),
        None => palette.provider.load_from_string(""),
    }
}

/// Colors of the theme with everything Omarchy derives from them (aliases,
/// shades), from Omarchy's own resolver, so the result equals what its
/// template rendering would produce.
fn theme_colors(colors_file: &Path) -> Option<HashMap<String, String>> {
    if !colors_file.is_file() {
        return None;
    }
    let output =
        Command::new("omarchy-theme-color").arg("--file").arg(colors_file).arg("--all").output().ok()?;
    output.status.success().then(|| parse_color_list(&String::from_utf8_lossy(&output.stdout)))
}

/// `key<TAB>value` lines of `omarchy-theme-color --all`.
fn parse_color_list(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

/// Fills `{{ key }}` and `{{ mix first second 30% }}` placeholders as
/// Omarchy's `omarchy-theme-set-templates` does. Unknown ones stay as they
/// are; GTK then skips that declaration.
fn render(template: &str, colors: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(length) = rest[start..].find("}}") else { break };
        let token = &rest[start + 2..start + length];
        out.push_str(&rest[..start]);
        match fill_token(token.trim(), colors) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[start..start + length + 2]),
        }
        rest = &rest[start + length + 2..];
    }
    out.push_str(rest);
    out
}

fn fill_token(token: &str, colors: &HashMap<String, String>) -> Option<String> {
    let words: Vec<&str> = token.split_whitespace().collect();
    match words.as_slice() {
        ["mix", first, second, amount] => mix(colors.get(*first)?, colors.get(*second)?, amount),
        [key] => colors.get(*key).cloned(),
        _ => None,
    }
}

/// `first` blended towards `second` by `amount` (`30%` or `0.3`), rounded
/// as Omarchy's `mix_color`.
fn mix(first: &str, second: &str, amount: &str) -> Option<String> {
    let amount = if let Some(percent) = amount.strip_suffix('%') {
        percent.parse::<f64>().ok()? / 100.0
    } else {
        let value = amount.parse::<f64>().ok()?;
        if value > 1.0 { value / 100.0 } else { value }
    }
    .clamp(0.0, 1.0);
    let (a, b) = (rgb(first)?, rgb(second)?);
    let channel = |i: usize| {
        let value = f64::from(a[i]) * (1.0 - amount) + f64::from(b[i]) * amount + 0.5;
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a blend of two bytes stays in 0..=255")]
        let value = value.floor() as u8;
        value
    };
    Some(format!("#{:02x}{:02x}{:02x}", channel(0), channel(1), channel(2)))
}

fn rgb(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() < 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// `~/.local/state/omarchy/current/theme`, where Omarchy renders its theme
/// templates; `None` when Omarchy is not there.
fn omarchy_theme_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    let current = state.join("omarchy/current");
    current.is_dir().then(|| current.join("theme"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> HashMap<String, String> {
        parse_color_list("accent\t#f38d70\nbackground\t#2c2525\nforeground\t#fff1f3\nred\t#fd6883\n")
    }

    #[test]
    fn fills_keys_and_mixes_like_omarchy() {
        let css = render("a {{ accent }}; b {{ mix background accent 30% }}; c {{ mix background foreground 0.15 }}", &colors());
        // Values from Omarchy's own mix_color for the same input.
        assert_eq!(css, "a #f38d70; b #68443c; c #4c4444");
    }

    #[test]
    fn unknown_placeholders_stay() {
        assert_eq!(render("x {{ nothing }} {{ mix accent nothing 5% }}", &colors()), "x {{ nothing }} {{ mix accent nothing 5% }}");
    }

    #[test]
    fn built_in_template_is_fully_filled_by_a_real_theme() {
        let Some(colors) = theme_colors(Path::new("/usr/share/omarchy/themes/tokyo-night/colors.toml")) else {
            return; // Not on Omarchy.
        };
        assert!(!render(PALETTE_TEMPLATE, &colors).contains("{{"));
    }
}
