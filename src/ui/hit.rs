//! Thread-local hit map for mouse support.
//!
//! Populated during render, consumed during event handling.
//! Each frame: `clear_hits()`, then widgets call `register_hit()`,
//! then the event loop calls `resolve_click()`.

use std::cell::RefCell;

use ratatui::layout::Rect;

use crate::app::{BoardTab, Focus, SortMode};
use crate::ui::menu_popup::EscapeMenuOption;

/// Clickable target identified by the hit map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    /// A cell on the 15x15 board grid.
    BoardCell(usize, usize),
    /// A slot in the 7-tile rack.
    RackSlot(usize),
    /// A row in the suggestions table.
    SuggestionRow(usize),
    /// A tab header (Board / Logs).
    TabHeader(BoardTab),
    /// A sort-mode tab above the suggestions pane.
    SortTab(SortMode),
    /// Click anywhere inside a focus pane to focus it.
    FocusPane(Focus),
    /// The open dialog's body: a click here does nothing (outside it, a
    /// click cancels the dialog).
    DialogArea,
    /// The "Yes" button of a confirmation dialog.
    DialogYes,
    /// The "No" button of a confirmation dialog.
    DialogNo,
    /// An option in the escape menu.
    MenuItem(EscapeMenuOption),
    /// A row of the game picker (index into `App::picker_games`).
    PickerRow(usize),
}

struct HitEntry {
    rect: Rect,
    target: HitTarget,
}

thread_local! {
    static HIT_MAP: RefCell<Vec<HitEntry>> = RefCell::new(Vec::with_capacity(256));
}

/// Clear all registered hit targets (call at the start of each frame).
pub fn clear_hits() {
    HIT_MAP.with(|map| map.borrow_mut().clear());
}

/// Register a clickable region during rendering.
pub fn register_hit(rect: Rect, target: HitTarget) {
    HIT_MAP.with(|map| map.borrow_mut().push(HitEntry { rect, target }));
}

/// Resolve a click at terminal coordinates `(x, y)`.
///
/// Returns the most recently registered (i.e. topmost) target that
/// contains the click point, or `None` if no target matches.
pub fn resolve_click(x: u16, y: u16) -> Option<HitTarget> {
    HIT_MAP.with(|map| {
        let map = map.borrow();
        // Iterate in reverse so the last-registered (topmost) target wins.
        for entry in map.iter().rev() {
            if x >= entry.rect.x
                && x < entry.rect.x + entry.rect.width
                && y >= entry.rect.y
                && y < entry.rect.y + entry.rect.height
            {
                return Some(entry.target.clone());
            }
        }
        None
    })
}
