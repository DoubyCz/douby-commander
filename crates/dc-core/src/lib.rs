//! GUI independent core of Double Commander.
//!
//! Everything here can be tested without a display. The GUI only renders
//! the state kept in [`Panel`] and forwards user actions to it.

pub mod command;
pub mod format;
mod item;
pub mod keymap;
pub mod mask;
pub mod multi_rename;
mod panel;
pub mod preview;
pub mod quick_search;
mod sort;

pub use item::FileItem;
pub use panel::{Activation, Panel, SelectionInfo};
pub use sort::{SortColumn, SortDirection, Sorting};
