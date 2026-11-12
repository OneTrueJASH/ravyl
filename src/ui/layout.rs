//! Fluid responsive layout.
//!
//! The board always gets priority (sized from terminal height). With room
//! for both, the sidebar (rack + stats, sort tabs, suggestions) sits to its
//! right, unless hidden with the `\` toggle. On terminals too narrow for
//! that, a stacked layout puts a one-line rack and a short suggestion list
//! under the board. Below the minimum size nothing is laid out and the UI
//! says how big the terminal needs to be.

use ratatui::layout::{Constraint, Layout, Rect};

use crate::ui::board_view::{BoardGeometry, AXIS_H, AXIS_W};

/// Narrowest board pane worth keeping next to a sidebar: the compact grid
/// (5 + 14*4), the axes and the border.
pub const BOARD_MIN_WIDTH: u16 = 61 + AXIS_W + 2;
/// Narrowest board pane that still shows every square (mini grid of 3
/// columns per cell, axes, border).
pub const MIN_WIDTH: u16 = 45 + AXIS_W + 2;
/// Fewest rows that show every square: title, tabs, message and hint
/// lines, board border, axis row and 15 board rows.
pub const MIN_HEIGHT: u16 = 4 + 2 + AXIS_H + 15;

/// Sidebar width needed for the rack tiles (7 x 5 + gaps + border).
const MIN_SIDEBAR_WIDTH: u16 = 45;
/// Gap between the board pane and the sidebar.
const GAP: u16 = 1;

/// Minimum sidebar width where suggestions are useful.
const SUGGESTIONS_MIN_WIDTH: u16 = 30;

/// Rows the stacked layout needs below the board: the one-line rack and a
/// suggestion list with border, header and at least three rows.
const STACKED_EXTRA_ROWS: u16 = 1 + 6;

/// Computed layout areas for a single frame.
#[derive(Debug, Clone, Default)]
pub struct AppLayout {
    /// Top status line.
    pub title_bar: Rect,
    /// Board pane (or log viewer).
    pub board_area: Rect,
    /// Tab bar above the board.
    pub tab_area: Rect,
    /// Message bar below the board.
    pub message_area: Rect,
    /// Rack + stats area (side-by-side layout).
    pub rack_area: Option<Rect>,
    /// One-line rack (stacked layout).
    pub rack_line: Option<Rect>,
    /// Sort-mode tab bar above suggestions.
    pub suggestions_tab_area: Option<Rect>,
    /// Suggestions table — `None` when there is no room for it.
    pub suggestions_area: Option<Rect>,
    /// Bottom hint line.
    pub status_bar: Rect,
    /// The terminal is below the minimum size: (needed width, height).
    pub too_small: Option<(u16, u16)>,
}

impl AppLayout {
    /// Board pane width (grid + axes + border) for the geometry that the
    /// terminal height allows, given unlimited width.
    fn ideal_board_width(terminal_height: u16) -> u16 {
        // Vertical overhead: title(1) + tab(1) + message(1) + status(1) +
        // board border(2) + axis row.
        let board_inner_h = terminal_height.saturating_sub(6 + AXIS_H);
        let geometry = BoardGeometry::fit(u16::MAX, board_inner_h);
        geometry.grid_size().0 + 2 + AXIS_W
    }

    /// Height the board pane needs for its mini (one row per square) form.
    fn mini_board_height() -> u16 {
        15 + AXIS_H + 2
    }

    /// Compute layout from terminal size. `sidebar_visible` is the `\`
    /// toggle: when false the board pane takes the whole terminal.
    pub fn compute(area: Rect, sidebar_visible: bool) -> Self {
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            return AppLayout {
                too_small: Some((MIN_WIDTH, MIN_HEIGHT)),
                ..Default::default()
            };
        }

        // Vertical: title(1) | content | status(1)
        let [title_bar, content, status_bar] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);

        // The board comes first: the sidebar only appears beside it once
        // both fit, and never while the user has toggled it away.
        let side_by_side =
            sidebar_visible && area.width >= BOARD_MIN_WIDTH + GAP + MIN_SIDEBAR_WIDTH;
        // Too narrow for that: stack a one-line rack and a short list
        // under a mini board when the height allows.
        let stacked = sidebar_visible
            && !side_by_side
            && content.height >= 2 + Self::mini_board_height() + STACKED_EXTRA_ROWS;

        let (board_section, sidebar, below) = if side_by_side {
            let ideal_bw = Self::ideal_board_width(area.height)
                .max(BOARD_MIN_WIDTH)
                .min(area.width - (MIN_SIDEBAR_WIDTH + GAP));
            let [board, _gap, side] = Layout::horizontal([
                Constraint::Length(ideal_bw),
                Constraint::Length(GAP),
                Constraint::Fill(1),
            ])
            .areas(content);
            (board, side, Rect::default())
        } else if stacked {
            // tab(1) + mini board + message(1), then the rest below.
            let board_rows = 2 + Self::mini_board_height();
            let [board, rest] =
                Layout::vertical([Constraint::Length(board_rows), Constraint::Fill(1)])
                    .areas(content);
            (board, Rect::default(), rest)
        } else {
            (content, Rect::default(), Rect::default())
        };

        // Board section: tab(1) | board | message(1)
        let [tab_row, board_area, message_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(board_section);
        let tab_area = Rect {
            x: tab_row.x + 1,
            width: tab_row.width.saturating_sub(1),
            ..tab_row
        };

        let mut layout = AppLayout {
            title_bar,
            board_area,
            tab_area,
            message_area,
            status_bar,
            ..Default::default()
        };

        if stacked {
            let [rack_line, list] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(below);
            layout.rack_line = Some(rack_line);
            layout.suggestions_area = Some(list);
            return layout;
        }

        // Sidebar split: top_pad(1) | rack | sort_tabs | suggestions | bottom_pad(1)
        // The padding rows align with the tab bar and message bar on the left.
        if sidebar.width >= SUGGESTIONS_MIN_WIDTH {
            let [_, rack, tabs, list, _] = Layout::vertical([
                Constraint::Length(1), // aligns with tab bar
                Constraint::Length(19),
                Constraint::Length(1),
                Constraint::Min(8),
                Constraint::Length(1), // aligns with message bar
            ])
            .areas(sidebar);
            layout.rack_area = Some(rack);
            layout.suggestions_tab_area = Some(tabs);
            layout.suggestions_area = Some(list);
        } else if sidebar.width > 10 {
            let [_, rack, _] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .areas(sidebar);
            layout.rack_area = Some(rack);
        }
        layout
    }

    /// Whether the rack is shown in any form.
    pub fn shows_rack(&self) -> bool {
        self.rack_area.is_some() || self.rack_line.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `\` toggle must actually remove the sidebar: on a terminal wide
    /// enough for both panes, hiding it hands the full width to the board.
    #[test]
    fn sidebar_toggle_changes_the_layout() {
        let area = Rect::new(0, 0, 160, 45);

        let shown = AppLayout::compute(area, true);
        assert!(shown.rack_area.is_some());
        assert!(shown.suggestions_area.is_some());

        let hidden = AppLayout::compute(area, false);
        assert!(hidden.rack_area.is_none());
        assert!(hidden.suggestions_area.is_none());
        assert!(
            hidden.board_area.width > shown.board_area.width,
            "the board should absorb the freed width"
        );
    }

    /// Narrow but tall terminals stack the rack and a short list under the
    /// board; short ones show just the board; tiny ones show nothing.
    #[test]
    fn narrow_terminals_stack_or_shrink() {
        let stacked = AppLayout::compute(Rect::new(0, 0, 80, 40), /* sidebar_visible= */ true);
        assert!(stacked.rack_area.is_none());
        assert!(stacked.rack_line.is_some());
        let list = stacked.suggestions_area.expect("short list");
        assert!(list.height >= 5);
        assert!(list.y > stacked.board_area.y + stacked.board_area.height);
        assert!(stacked.shows_rack());

        let board_only =
            AppLayout::compute(Rect::new(0, 0, 80, 24), /* sidebar_visible= */ true);
        assert!(!board_only.shows_rack());
        assert!(board_only.suggestions_area.is_none());
        assert!(board_only.too_small.is_none());

        let tiny = AppLayout::compute(Rect::new(0, 0, 40, 15), /* sidebar_visible= */ true);
        assert_eq!(tiny.too_small, Some((MIN_WIDTH, MIN_HEIGHT)));
    }
}
