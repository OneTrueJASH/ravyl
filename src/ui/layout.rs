//! Fluid responsive layout — no hard breakpoints.
//!
//! Board always gets priority (sized from terminal height).
//! Sidebar fills remaining width. Suggestions hide if sidebar < 30 cols.

use crate::board::BOARD_SIZE;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Minimum cell dimensions.
const MIN_CELL_WIDTH: u16 = 7;
const MIN_CELL_HEIGHT: u16 = 4;

/// Absolute minimum board grid size.
pub const BOARD_MIN_WIDTH: u16 = 93; // 7 + 14*6 + 2
const BOARD_MIN_HEIGHT: u16 = 48; // 4 + 14*3 + 2

/// Minimum sidebar width where suggestions are useful.
const SUGGESTIONS_MIN_WIDTH: u16 = 30;

/// Computed layout areas for a single frame.
pub struct AppLayout {
    /// Top status line.
    pub title_bar: Rect,
    /// Board pane (or log viewer) — always visible.
    pub board_area: Rect,
    /// Tab bar above the board.
    pub tab_area: Rect,
    /// Message bar below the board.
    pub message_area: Rect,
    /// Rack + stats area — always visible when sidebar exists.
    pub rack_area: Option<Rect>,
    /// Sort-mode tab bar above suggestions.
    pub suggestions_tab_area: Option<Rect>,
    /// Suggestions table — `None` when sidebar is too narrow.
    pub suggestions_area: Option<Rect>,
    /// Bottom hint line.
    pub status_bar: Rect,
}

impl AppLayout {
    /// Calculate ideal board pane width so cells maintain a ~7:4 aspect ratio.
    fn ideal_board_width(terminal_height: u16) -> u16 {
        // Vertical overhead: title(1) + tab(1) + message(1) + status(1) + board border(2)
        let board_inner_h = terminal_height.saturating_sub(6);
        let max_cell_h = board_inner_h.div_ceil(BOARD_SIZE as u16).max(MIN_CELL_HEIGHT);
        let cell_w = (max_cell_h * 7 / 4).max(MIN_CELL_WIDTH);
        (BOARD_SIZE as u16 * cell_w - (BOARD_SIZE as u16 - 1)) + 2 // grid width + outer border
    }

    /// Compute layout from terminal size.
    pub fn compute(area: Rect) -> Self {
        // Vertical: title(1) | content | status(1)
        let vert = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(area);

        let title_bar = vert[0];
        let content = vert[1];
        let status_bar = vert[2];

        // If too small for any meaningful layout, give everything to board.
        if area.width < BOARD_MIN_WIDTH || area.height < 20 {
            return AppLayout {
                title_bar,
                board_area: content,
                tab_area: Rect::default(),
                message_area: Rect::default(),
                rack_area: None,
                suggestions_tab_area: None,
                suggestions_area: None,
                status_bar,
            };
        }

        // Determine board width: ideal from height, clamped to leave room for sidebar.
        let min_sidebar = 45u16; // rack tiles need ~41
        let gap = 1u16; // visual gap between left and right halves
        let ideal_bw = Self::ideal_board_width(area.height)
            .max(BOARD_MIN_WIDTH)
            .min(area.width.saturating_sub(min_sidebar + gap));

        // Horizontal: board | gap | sidebar
        let horiz = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(ideal_bw),
                Constraint::Length(gap),
                Constraint::Fill(1),
            ])
            .split(content);

        let board_section = horiz[0];
        let sidebar = horiz[2];

        // Board section: tab(1) | board | message(1)
        let board_v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(BOARD_MIN_HEIGHT),
                Constraint::Length(1),
            ])
            .split(board_section);

        let tab_area = Rect {
            x: board_v[0].x + 1,
            y: board_v[0].y,
            width: board_v[0].width.saturating_sub(1),
            height: board_v[0].height,
        };

        // Sidebar split: top_pad(1) | rack | sort_tabs | suggestions | bottom_pad(1)
        // The padding rows align with the tab bar and message bar on the left.
        let sidebar_width = sidebar.width;
        let show_suggestions = sidebar_width >= SUGGESTIONS_MIN_WIDTH;

        let (rack_area, suggestions_tab_area, suggestions_area) = if show_suggestions {
            let sidebar_v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),  // aligns with tab bar
                    Constraint::Length(19),
                    Constraint::Length(1),
                    Constraint::Min(8),
                    Constraint::Length(1),  // aligns with message bar
                ])
                .split(sidebar);
            (Some(sidebar_v[1]), Some(sidebar_v[2]), Some(sidebar_v[3]))
        } else if sidebar_width > 10 {
            let sidebar_v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),  // aligns with tab bar
                    Constraint::Min(1),
                    Constraint::Length(1),  // aligns with message bar
                ])
                .split(sidebar);
            (Some(sidebar_v[1]), None, None)
        } else {
            (None, None, None)
        };

        AppLayout {
            title_bar,
            board_area: board_v[1],
            tab_area,
            message_area: board_v[2],
            rack_area,
            suggestions_tab_area,
            suggestions_area,
            status_bar,
        }
    }
}
