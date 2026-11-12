//! Central application state, event handling, and business logic.
//!
//! Contains the [`App`] struct which owns all game state, dialog state,
//! and coordinates between the solver, the simulation engine and the UI.
//!
//! - `state`: the `App` struct, its enums and records, construction, toggles
//! - `input`: keyboard, mouse and paste handling, focus, the list filter
//! - `engine`: solving, the simulation and endgame jobs, polling results
//! - `opponent`: detecting and scoring their move, their strength, inference
//! - `edit`: board and rack entry, applying moves, undo/redo, the `:`
//!   prompt, clipboard and the CGP position
//! - `session`: the open game: autosave, its lock, naming, switching
//!   games, the picker and the quit flow
//! - `sort`: ordering the suggestions list
//! - `format`: number formatting shared with the UI

mod edit;
mod engine;
mod format;
mod input;
mod opponent;
mod session;
mod sort;
mod state;
#[cfg(test)]
mod tests;

pub use edit::{base64, parse_coords, QuickPlay};
pub use format::group_thousands;
pub use session::{SaveAsPurpose, AUTOSAVE_INTERVAL};
pub use state::*;
