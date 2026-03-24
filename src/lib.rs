//! Library crate for the Crossplay solver engine.
//!
//! Re-exports core modules for use by integration tests and external consumers.
//! The TUI application modules (app, ui, settings) are only available in the binary.

pub mod board;
pub mod dictionary;
pub mod endgame;
pub mod game;
pub mod logging;
pub mod solver;
pub mod tile;
