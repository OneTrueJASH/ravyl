//! Unified event types for the application.

/// Action returned by event handlers, telling the main loop what to do.
pub enum AppAction {
    /// Nothing happened; continue.
    Nothing,
    /// Redraw the screen (state changed).
    Redraw,
    /// Quit the application.
    Quit,
}
