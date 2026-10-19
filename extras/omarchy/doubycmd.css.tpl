/*
 * Douby Commander: colors of the current Omarchy theme.
 *
 * Built into the application, which fills it from the theme itself; no
 * setup needed. To change the look, copy it and edit the copy:
 *   cp extras/omarchy/doubycmd.css.tpl ~/.config/omarchy/themed/
 * Omarchy then renders it to ~/.local/state/omarchy/current/theme/doubycmd.css
 * on every theme change, which the running application prefers and reloads.
 * Placeholders: see docs/theming.md of Omarchy (`omarchy-theme-color --all`).
 */

/* Colors of the application itself (defaults in crates/dc-gui/src/theme.rs). */
@define-color dc_marked_color {{ red }};
@define-color dc_active_path_bg_color {{ accent }};
@define-color dc_active_path_fg_color {{ background }};
@define-color dc_inactive_cursor_color {{ accent }};
@define-color dc_current_drive_color {{ mix background accent 30% }};
@define-color dc_menu_border_color {{ accent }};
@define-color dc_wave_bg_color {{ accent }};
@define-color dc_wave_fg_color {{ background }};

/* Named colors of the GTK theme that widgets and our base style refer to. */
@define-color theme_bg_color {{ background }};
@define-color theme_fg_color {{ foreground }};
@define-color theme_base_color {{ background }};
@define-color theme_text_color {{ foreground }};
@define-color theme_selected_bg_color {{ accent }};
@define-color theme_selected_fg_color {{ background }};
@define-color borders {{ mix background foreground 15% }};
/* Accent of the GTK theme (blue in Adwaita), where widgets refer to it by name. */
@define-color accent_color {{ accent }};
@define-color accent_bg_color {{ accent }};
@define-color accent_fg_color {{ background }};

window.background,
.view,
columnview,
columnview > listview,
listview {
    background-color: {{ background }};
    color: {{ foreground }};
}

columnview > header > button {
    background-color: {{ lighter_background }};
    color: {{ foreground }};
}

columnview > listview > row:hover {
    background-color: {{ mix background foreground 8% }};
}

columnview > listview > row:selected {
    background-color: {{ mix background accent 45% }};
    color: {{ bright_foreground }};
}

popover > contents,
popover > arrow,
tooltip.background {
    background-color: {{ lighter_background }};
    color: {{ foreground }};
}

popover modelbutton:hover,
button.flat:hover {
    background-color: {{ mix lighter_background accent 30% }};
}

/* Everything the GTK theme draws in its own accent (Adwaita blue).
   Menu bar: CSS nodes `menubar > item`, `.active` while its menu is open
   (docs.gtk.org/gtk4/class.PopoverMenuBar.html). */
menubar > item:hover,
menubar > item:selected,
menubar > item.active,
menubar > item:focus-visible {
    background-color: {{ mix lighter_background accent 30% }};
    color: {{ foreground }};
    box-shadow: none;
}

*:focus-visible,
entry:focus-within {
    outline-color: {{ mix background accent 60% }};
}

check:checked,
radio:checked,
check:indeterminate,
radio:indeterminate,
switch:checked {
    background-color: {{ accent }};
    color: {{ background }};
}

scale > trough > highlight,
levelbar > trough > block.filled {
    background-color: {{ accent }};
}

link,
label link,
button.link {
    color: {{ accent }};
}

notebook > header > tabs > tab:checked,
stackswitcher > button:checked {
    box-shadow: inset 0 -3px {{ accent }};
    color: {{ foreground }};
}

/* The main button of dialogs (OK) and the default button of message dialogs.
   The GTK theme paints buttons with background-image and per state. */
button.suggested-action,
button.default {
    background-image: none;
    background-color: {{ accent }};
    color: {{ background }};
}

button.suggested-action:hover,
button.default:hover {
    background-image: none;
    background-color: {{ mix accent foreground 15% }};
}

button.suggested-action:active,
button.suggested-action:checked,
button.default:active {
    background-image: none;
    background-color: {{ mix accent background 20% }};
}

button.suggested-action:disabled {
    background-image: none;
    background-color: {{ mix background accent 30% }};
    color: {{ mix background foreground 50% }};
}

columnview > header > button sort-indicator,
spinner {
    color: {{ accent }};
}

entry {
    background-color: {{ lighter_background }};
    color: {{ foreground }};
    caret-color: {{ cursor }};
}

entry selection,
label selection {
    background-color: {{ selection_background }};
    color: {{ selection_foreground }};
}

progressbar > trough > progress {
    background-color: {{ accent }};
}

scrollbar slider {
    background-color: {{ mix background foreground 35% }};
}

separator {
    background-color: {{ mix background foreground 15% }};
}

/* Startup wave over the function keys, see theme.rs. Repeated here because
   this stylesheet is above the base style and its rules would win. */
.dc-function-bar button.dc-wave,
.dc-function-bar button.dc-wave:disabled {
    background-image: none;
    background-color: {{ accent }};
    color: {{ background }};
    filter: none;
}
