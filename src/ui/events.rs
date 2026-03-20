//! Unified event types for the application.

use crate::refengine_mcts::SimulationOutput;
use crossterm::event::{KeyEvent, MouseEvent};

/// All events the application can handle.
pub enum AppEvent {
    /// Keyboard input.
    Key(KeyEvent),
    /// Mouse input.
    Mouse(MouseEvent),
    /// Periodic tick (for spinner animation, polling).
    Tick,
    /// RefEngine/MCTS simulation completed.
    MctsComplete(Result<SimulationOutput, String>),
}

/// Action returned by event handlers, telling the main loop what to do.
pub enum AppAction {
    /// Nothing happened; continue.
    Nothing,
    /// Redraw the screen (state changed).
    Redraw,
    /// Quit the application.
    Quit,
}
