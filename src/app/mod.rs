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
//!
//! The submodules share this module's imports (`use super::*`).

mod edit;
mod engine;
mod input;
mod opponent;
mod session;
mod sort;
mod state;
#[cfg(test)]
mod tests;

pub use edit::{base64, parse_coords, QuickPlay};
pub use session::{SaveAsPurpose, AUTOSAVE_INTERVAL};
pub use state::*;

#[allow(unused_imports)]
use self::{edit::*, engine::*, input::*, opponent::*, session::*, sort::*};

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use std::cell::Cell;
use std::path::PathBuf;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::board::{Board, Direction, BOARD_SIZE};
use crate::dictionary::Dictionary;
use crate::endgame::{EndgameMove, EndgameSolver};
use crate::game::{observed_move_loss, SolverState};
use crate::logging::log;
use crate::refengine_mcts::{InferenceResult, RefSimulator};
use crate::mcts::{OpponentModel, TilePool};
use crate::settings::{self, Settings};
use crate::sim::{ArmStatus, ArmSummary, SimSettings, SimSnapshot, SimState, Simulation};
use crate::solver::{Move, Solver};
use crate::tile::Tile;
use crate::ui::events::AppAction;
use crate::ui::hit::{resolve_click, HitTarget};
use crate::ui::menu_popup::EscapeMenuOption;

/// `1234567` as `1,234,567`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
