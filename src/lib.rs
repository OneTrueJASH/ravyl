//! Crossplay solver: the TUI over the RAVYN engine (move generation,
//! playouts, simulation, endgame), plus the RefEngine bindings for opponent
//! inference.
//! The `cpsolver2` binary is a thin event loop over this library.

pub mod app;
pub mod datafiles;
pub mod dictionary;
pub mod games;
pub mod logging;
#[cfg(feature = "refengine")]
pub mod refengine_ffi;
pub mod refengine_mcts;
pub mod settings;
pub mod ui;

// The engine's modules, at the paths the app has always used.
pub use ravyn::{board, endgame, game, leaves, mcts, sim, solver, stats, tile};
