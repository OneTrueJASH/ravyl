//! Crossplay solver: engine (move generation, playouts, simulation,
//! endgame), the RefEngine bindings for opponent inference, and the TUI.
//! The `cpsolver2` binary is a thin event loop over this library.

pub mod app;
pub mod board;
pub mod datafiles;
pub mod dictionary;
pub mod endgame;
pub mod game;
pub mod leaves;
pub mod logging;
#[cfg(feature = "refengine")]
pub mod refengine_ffi;
pub mod refengine_mcts;
pub mod mcts;
pub mod settings;
pub mod sim;
pub mod solver;
pub mod stats;
pub mod tile;
pub mod ui;
