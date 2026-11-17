//! Crossplay solver: the TUI over the RAVYN engine (move generation,
//! playouts, simulation, inference, endgame and pre-endgame).
//! The `cpsolver2` binary is a thin event loop over this library.

pub mod app;
pub mod datafiles;
pub mod dictionary;
pub mod games;
pub mod logging;
pub mod settings;
pub mod ui;

// The engine's modules, at the paths the app has always used.
pub use ravyn::{
    board, endgame, game, inference, leaves, mcts, preendgame, rules, sim, solver, stats, tile,
};
