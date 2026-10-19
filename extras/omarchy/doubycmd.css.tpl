/*
 * Douby Commander: colors of the current Omarchy theme.
 *
 * Built into the application, which fills it from the theme itself; no
 * setup needed. To change the look, copy it and edit the copy:
 *   cp extras/omarchy/doubycmd.css.tpl ~/.config/omarchy/themed/
 * Omarchy then renders it to ~/.local/state/omarchy/current/theme/doubycmd.css
 * on every theme change, which the running application prefers and reloads.
 * Placeholders: see docs/theming.md of Omarchy (`omarchy-theme-color --all`).
 *
 * Only colors: every widget of libadwaita takes them from these variables,
 * the looks are in crates/dc-gui/src/theme.rs.
 */

:root {
    /* libadwaita */
    --accent-bg-color: {{ accent }};
    --accent-fg-color: {{ background }};
    --window-bg-color: {{ background }};
    --window-fg-color: {{ foreground }};
    --view-bg-color: {{ background }};
    --view-fg-color: {{ foreground }};
    --headerbar-bg-color: {{ lighter_background }};
    --headerbar-fg-color: {{ foreground }};
    --headerbar-backdrop-color: {{ background }};
    --popover-bg-color: {{ lighter_background }};
    --popover-fg-color: {{ foreground }};
    --dialog-bg-color: {{ lighter_background }};
    --dialog-fg-color: {{ foreground }};
    --card-bg-color: {{ mix background foreground 5% }};
    --card-fg-color: {{ foreground }};
    --sidebar-bg-color: {{ lighter_background }};
    --sidebar-fg-color: {{ foreground }};
    --destructive-bg-color: {{ red }};
    --error-bg-color: {{ red }};
    --success-bg-color: {{ green }};
    --warning-bg-color: {{ yellow }};

    /* Douby Commander (the others follow the accent, see theme.rs) */
    --dc-marked-color: {{ red }};
    --dc-cursor-fg-color: {{ bright_foreground }};
    --dc-current-drive-color: {{ mix background accent 30% }};
}

/* The theme's own text cursor and text selection. */
text, entry {
    caret-color: {{ cursor }};
}

entry selection,
text selection,
label selection {
    background-color: {{ selection_background }};
    color: {{ selection_foreground }};
}
