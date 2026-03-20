//! Centralized theme: every color and style constant used by the TUI.

use ratatui::style::{Color, Modifier, Style};

// ── Base palette ──────────────────────────────────────────────────────
pub const TEXT_PRIMARY: Color = Color::White;
pub const TEXT_DIM: Color = Color::DarkGray;
pub const TEXT_MUTED: Color = Color::Gray;

// ── Focus / borders ───────────────────────────────────────────────────
pub const BORDER_NORMAL: Color = Color::DarkGray;

// ── Tiles ─────────────────────────────────────────────────────────────
pub const TILE_BG: Color = Color::Rgb(31, 95, 191); // Crossplay blue
pub const TILE_FG: Color = Color::White;
pub const TILE_BLANK_FG: Color = Color::Rgb(180, 180, 180);
pub const TILE_EMPTY_BG: Color = Color::Rgb(60, 60, 60);
pub const TILE_PREVIEW_BG: Color = Color::LightYellow;
pub const TILE_PREVIEW_FG: Color = Color::Black;
pub const TILE_PREVIEW_BLANK_FG: Color = Color::DarkGray;

// ── Board ─────────────────────────────────────────────────────────────
pub const CURSOR_BG: Color = Color::Red;
pub const CURSOR_FG: Color = Color::White;
pub const PREMIUM_3W: Color = Color::Rgb(140, 80, 180);
pub const PREMIUM_2W: Color = Color::Rgb(100, 140, 200);
pub const PREMIUM_3L: Color = Color::Rgb(80, 160, 80);
pub const PREMIUM_2L: Color = Color::Rgb(180, 150, 80);
pub const PREMIUM_CENTER: Color = Color::Yellow;
pub const PREMIUM_NORMAL: Color = Color::DarkGray;

// ── Numeric columns ──────────────────────────────────────────────────
pub const SCORE_FG: Color = Color::Yellow;
pub const EQUITY_FG: Color = Color::Cyan;
pub const SIM_EQUITY_FG: Color = Color::Magenta;

// ── Win percentage / quality indicators ──────────────────────────────
pub const WIN_GOOD: Color = Color::Green;
pub const WIN_NEUTRAL: Color = Color::Yellow;
pub const WIN_BAD: Color = Color::Red;

// ── Leave quality ────────────────────────────────────────────────────
pub const LEAVE_GREAT: Color = Color::Green;
pub const LEAVE_GOOD: Color = Color::Cyan;
pub const LEAVE_MEH: Color = Color::Yellow;
pub const LEAVE_BAD: Color = Color::Red;

// ── Game phase ───────────────────────────────────────────────────────
pub const PHASE_EARLY: Color = Color::Green;
pub const PHASE_MID: Color = Color::Yellow;
pub const PHASE_LATE: Color = Color::Red;
pub const PHASE_ENDGAME: Color = Color::Magenta;

// ── Status / messages ────────────────────────────────────────────────
pub const MSG_OK: Color = Color::Green;
pub const MSG_ERROR: Color = Color::Red;
pub const MSG_PROGRESS: Color = Color::Magenta;
pub const TITLE_FG: Color = Color::LightBlue;

// ── Popups / dialogs ─────────────────────────────────────────────────
pub const POPUP_BORDER: Color = Color::Cyan;
pub const POPUP_WARN_BORDER: Color = Color::Yellow;
pub const POPUP_KEY: Color = Color::Yellow;
pub const POPUP_HEADER: Color = Color::Cyan;
pub const POPUP_DESC: Color = Color::Gray;

// ── Suggestion highlight ─────────────────────────────────────────────
pub const HIGHLIGHT_BG: Color = Color::LightYellow;
pub const HIGHLIGHT_FG: Color = Color::Black;

// ── Hint bar ─────────────────────────────────────────────────────────
pub const HINT_KEY: Color = Color::Cyan;
pub const HINT_DESC: Color = Color::DarkGray;

// ── Pane focus ───────────────────────────────────────────────────────
pub const PANE_RACK: Color = Color::Rgb(180, 83, 9); // amber-700
pub const PANE_SUGGESTIONS: Color = Color::Rgb(109, 40, 217); // violet-700

// ── Tab bar ──────────────────────────────────────────────────────────
pub const TAB_BOARD_ACTIVE: Color = Color::Rgb(4, 120, 87); // emerald-700

// ── Button styles (dialogs) ──────────────────────────────────────────
pub const BTN_YES_FG: Color = Color::Green;
pub const BTN_NO_FG: Color = Color::Red;
pub const BTN_SELECTED_FG: Color = Color::Black;

// ── Helper functions ─────────────────────────────────────────────────

pub fn highlight_style() -> Style {
    Style::new()
        .bg(HIGHLIGHT_BG)
        .fg(HIGHLIGHT_FG)
        .add_modifier(Modifier::BOLD)
}

pub fn leave_color(leave_value: f32) -> Color {
    if leave_value >= 10.0 {
        LEAVE_GREAT
    } else if leave_value >= 0.0 {
        LEAVE_GOOD
    } else if leave_value >= -10.0 {
        LEAVE_MEH
    } else {
        LEAVE_BAD
    }
}

pub fn win_color(pct: f32) -> Color {
    if pct >= 60.0 {
        WIN_GOOD
    } else if pct >= 45.0 {
        WIN_NEUTRAL
    } else {
        WIN_BAD
    }
}

pub fn phase_color(phase: &crate::stats::GamePhase) -> Color {
    match phase {
        crate::stats::GamePhase::Early => PHASE_EARLY,
        crate::stats::GamePhase::Mid => PHASE_MID,
        crate::stats::GamePhase::Late => PHASE_LATE,
        crate::stats::GamePhase::Endgame => PHASE_ENDGAME,
    }
}
