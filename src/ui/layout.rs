//! Fluid responsive layout — no hard breakpoints.
//!
//! Board always gets priority (sized from terminal height).
//! Sidebar fills remaining width. Suggestions hide if sidebar < 30 cols.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::ui::board_view::BoardGeometry;

/// Narrowest board pane worth keeping next to a sidebar: the compact grid
/// (5 + 14*4) plus its border.
pub const BOARD_MIN_WIDTH: u16 = 63;
/// Narrowest board pane that still shows every square (mini grid + border).
const BOARD_ABS_MIN_WIDTH: u16 = 47;
const MIN_LAYOUT_HEIGHT: u16 = 20;

/// Sidebar width needed for the rack tiles (7 x 5 + gaps + border).
const MIN_SIDEBAR_WIDTH: u16 = 45;
/// Gap between the board pane and the sidebar.
const GAP: u16 = 1;

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
    /// Board pane width (grid + border) for the geometry that the terminal
    /// height allows, given unlimited width.
    fn ideal_board_width(terminal_height: u16) -> u16 {
        // Vertical overhead: title(1) + tab(1) + message(1) + status(1) + board border(2)
        let board_inner_h = terminal_height.saturating_sub(6);
        let geometry = BoardGeometry::fit(u16::MAX, board_inner_h);
        geometry.grid_size().0 + 2
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
        if area.width < BOARD_ABS_MIN_WIDTH || area.height < MIN_LAYOUT_HEIGHT {
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

        // The board comes first: the sidebar only appears once both fit.
        let has_sidebar = area.width >= BOARD_MIN_WIDTH + GAP + MIN_SIDEBAR_WIDTH;
        let (board_section, sidebar) = if has_sidebar {
            // Board width: ideal from height, clamped to leave room for sidebar.
            let ideal_bw = Self::ideal_board_width(area.height)
                .max(BOARD_MIN_WIDTH)
                .min(area.width - (MIN_SIDEBAR_WIDTH + GAP));

            // Horizontal: board | gap | sidebar
            let horiz = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(ideal_bw),
                    Constraint::Length(GAP),
                    Constraint::Fill(1),
                ])
                .split(content);
            (horiz[0], horiz[2])
        } else {
            (content, Rect::default())
        };

        // Board section: tab(1) | board | message(1)
        let board_v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
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
                    Constraint::Length(1), // aligns with tab bar
                    Constraint::Length(19),
                    Constraint::Length(1),
                    Constraint::Min(8),
                    Constraint::Length(1), // aligns with message bar
                ])
                .split(sidebar);
            (Some(sidebar_v[1]), Some(sidebar_v[2]), Some(sidebar_v[3]))
        } else if sidebar_width > 10 {
            let sidebar_v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // aligns with tab bar
                    Constraint::Min(1),
                    Constraint::Length(1), // aligns with message bar
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
