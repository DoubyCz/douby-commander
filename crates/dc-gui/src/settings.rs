//! Options of the application.
//!
//! For now only the defaults: loading, saving and the options dialog come
//! with the configuration design.
//! Every option lives here, never as a loose constant in the code.

#[derive(Debug, Clone)]
pub struct Settings {
    /// "Use trash" of the original (`gUseTrash`): F8 deletes into the trash,
    /// Shift+F8 permanently; when off, the other way round.
    pub use_trash: bool,
    /// A wave of the theme's accent color runs over the function key buttons
    /// when the application starts, to point them out. Skipped anyway when
    /// the system turns animations off.
    pub startup_wave: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { use_trash: true, startup_wave: true }
    }
}
