//! Board widget: 15x15 grid with premium squares, cursor, and preview tiles.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    symbols::{self, merge::MergeStrategy},
    widgets::{Block, BorderType, Padding, Widget},
};

use crate::board::{Direction, Premium, BOARD_SIZE};
use crate::game::SolverState;
use crate::tile::{letter_value, Tile};
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::theme;

const MIN_CELL_WIDTH: u16 = 7;
const MIN_CELL_HEIGHT: u16 = 4;

/// How board cells are drawn for the space available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardGeometry {
    pub cell_w: u16,
    pub cell_h: u16,
    /// Cells share thick borders (cell_h >= 3); otherwise the grid is a
    /// plain coloured matrix with one row per board row.
    pub bordered: bool,
}

impl BoardGeometry {
    /// Largest geometry that fits in `inner_w` x `inner_h` cells.
    ///
    /// Full size keeps a ~7:4 cell aspect with 4+ row cells (needs 46 rows);
    /// compact uses 3-row cells with a single content row (needs 31 rows);
    /// below that the board is a borderless grid with 1-row cells.
    pub fn fit(inner_w: u16, inner_h: u16) -> Self {
        let n = BOARD_SIZE as u16;
        // Bordered grids share borders: total = cell + (n - 1) * (cell - 1).
        let bordered_fits = |cell_w: u16, cell_h: u16| {
            cell_w + (n - 1) * (cell_w - 1) <= inner_w && cell_h + (n - 1) * (cell_h - 1) <= inner_h
        };

        if bordered_fits(MIN_CELL_WIDTH, MIN_CELL_HEIGHT) {
            let max_from_width = inner_w.div_ceil(n);
            let max_from_height = inner_h.div_ceil(n);
            let cell_h_for_width = (max_from_width * 4 + 3) / 7;
            let cell_h = max_from_height.min(cell_h_for_width).max(MIN_CELL_HEIGHT);
            let cell_w = (cell_h * 7 / 4).max(MIN_CELL_WIDTH);
            return BoardGeometry {
                cell_w,
                cell_h,
                bordered: true,
            };
        }

        // Compact: 3-row cells, as wide as fits between 5 and 7.
        for cell_w in (5..=7).rev() {
            if bordered_fits(cell_w, 3) {
                return BoardGeometry {
                    cell_w,
                    cell_h: 3,
                    bordered: true,
                };
            }
        }

        // Mini: borderless, one row per board row, 3-4 columns per cell.
        let cell_w = (inner_w / n).clamp(3, 4);
        BoardGeometry {
            cell_w,
            cell_h: 1,
            bordered: false,
        }
    }

    /// Total grid size in terminal cells.
    pub fn grid_size(&self) -> (u16, u16) {
        let n = BOARD_SIZE as u16;
        if self.bordered {
            (
                self.cell_w + (n - 1) * (self.cell_w - 1),
                self.cell_h + (n - 1) * (self.cell_h - 1),
            )
        } else {
            (self.cell_w * n, self.cell_h * n)
        }
    }

    fn step(&self) -> (u16, u16) {
        if self.bordered {
            (self.cell_w - 1, self.cell_h - 1)
        } else {
            (self.cell_w, self.cell_h)
        }
    }
}

fn premium_label(premium: &Premium) -> &'static str {
    match premium {
        Premium::TripleWord => "3W",
        Premium::DoubleWord => "2W",
        Premium::TripleLetter => "3L",
        Premium::DoubleLetter => "2L",
        Premium::Center => "\u{2666}",
        Premium::Normal => "",
    }
}

fn premium_fg(premium: &Premium) -> Color {
    match premium {
        Premium::TripleWord => theme::PREMIUM_3W,
        Premium::DoubleWord => theme::PREMIUM_2W,
        Premium::TripleLetter => theme::PREMIUM_3L,
        Premium::DoubleLetter => theme::PREMIUM_2L,
        Premium::Center => theme::PREMIUM_CENTER,
        Premium::Normal => theme::PREMIUM_NORMAL,
    }
}

pub struct BoardWidget<'a> {
    state: &'a SolverState,
    cursor: (usize, usize),
    is_focused: bool,
    typing_direction: Direction,
    preview: HashMap<(usize, usize), &'a Tile>,
    geometry: BoardGeometry,
}

impl<'a> BoardWidget<'a> {
    pub fn new(
        state: &'a SolverState,
        cursor: (usize, usize),
        is_focused: bool,
        typing_direction: Direction,
        preview_tiles: Option<&'a [(usize, usize, Tile)]>,
    ) -> Self {
        let preview = preview_tiles
            .map(|tiles| tiles.iter().map(|(r, c, t)| ((*r, *c), t)).collect())
            .unwrap_or_default();
        Self {
            state,
            cursor,
            is_focused,
            typing_direction,
            preview,
            geometry: BoardGeometry {
                cell_w: MIN_CELL_WIDTH,
                cell_h: MIN_CELL_HEIGHT,
                bordered: true,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cell(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        cell_w: u16,
        cell_h: u16,
        border_color: Color,
        bg_color: Option<Color>,
        fg_color: Color,
        label: &str,
        show_direction: bool,
        tile_value: Option<u32>,
    ) {
        let cell_area = Rect::new(x, y, cell_w, cell_h);
        let inner = if self.geometry.bordered {
            let block = Block::bordered()
                .border_type(BorderType::Thick)
                .border_style(Style::new().fg(border_color).bold())
                .merge_borders(MergeStrategy::Exact);
            block.render(cell_area, buf);
            Rect::new(
                x + 1,
                y + 1,
                cell_w.saturating_sub(2),
                cell_h.saturating_sub(2),
            )
        } else {
            // Borderless: leave the last column as a gap between cells.
            Rect::new(x, y, cell_w.saturating_sub(1), cell_h)
        };

        if let Some(bg) = bg_color {
            let fill_style = Style::new().bg(bg);
            for dy in 0..inner.height {
                for dx in 0..inner.width {
                    if let Some(cell) = buf.cell_mut((inner.x + dx, inner.y + dy)) {
                        cell.set_char(' ').set_style(fill_style);
                    }
                }
            }
        }

        if !label.is_empty() && inner.height > 0 && inner.width > 0 {
            let label_len = label.chars().count() as u16;
            // With a single content row the value shares it: keep the label
            // at the left so the two never collide.
            let label_x = if inner.height == 1 && tile_value.is_some() {
                inner.x
            } else {
                inner.x + inner.width.saturating_sub(label_len) / 2
            };
            let label_y = inner.y + inner.height / 2;
            let style = if let Some(bg) = bg_color {
                Style::new().fg(fg_color).bg(bg).bold()
            } else {
                Style::new().fg(fg_color).bold()
            };
            for (i, ch) in label.chars().enumerate() {
                if let Some(cell) = buf.cell_mut((label_x + i as u16, label_y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }

        if let Some(value) = tile_value {
            if inner.width >= 2 {
                let value_str = value.to_string();
                let value_x = inner.x + inner.width.saturating_sub(value_str.len() as u16);
                let value_y = inner.y;
                let style = if let Some(bg) = bg_color {
                    Style::new().fg(fg_color).bg(bg)
                } else {
                    Style::new().fg(fg_color)
                };
                for (i, ch) in value_str.chars().enumerate() {
                    if let Some(cell) = buf.cell_mut((value_x + i as u16, value_y)) {
                        cell.set_char(ch).set_style(style);
                    }
                }
            }
        }

        if show_direction && inner.width > 0 && inner.height > 0 {
            let arrow = match self.typing_direction {
                Direction::Horizontal => '\u{2192}',
                Direction::Vertical => '\u{2193}',
            };
            let arrow_x = inner.x + inner.width.saturating_sub(1);
            let arrow_y = inner.y + inner.height.saturating_sub(1);
            let style = if let Some(bg) = bg_color {
                Style::new().fg(fg_color).bg(bg)
            } else {
                Style::new().fg(fg_color)
            };
            if let Some(cell) = buf.cell_mut((arrow_x, arrow_y)) {
                cell.set_char(arrow).set_style(style);
            }
        }
    }
}

impl Widget for BoardWidget<'_> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        let border_color = if self.is_focused {
            theme::TAB_BOARD_ACTIVE
        } else {
            theme::BORDER_NORMAL
        };

        let block = Block::bordered()
            .border_set(symbols::border::PROPORTIONAL_TALL)
            .border_style(border_color)
            .padding(Padding::uniform(0));
        let inner = block.inner(area);
        block.render(area, buf);

        // Register focus pane hit for the whole board area.
        register_hit(area, HitTarget::FocusPane(crate::app::Focus::Board));

        self.geometry = BoardGeometry::fit(inner.width, inner.height);
        let BoardGeometry { cell_w, cell_h, .. } = self.geometry;
        let (step_x, step_y) = self.geometry.step();
        let (board_w, board_h) = self.geometry.grid_size();

        let start_x = inner.x + (inner.width.saturating_sub(board_w)) / 2;
        let start_y = inner.y + (inner.height.saturating_sub(board_h)) / 2;

        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                let cell_x = start_x + col as u16 * step_x;
                let cell_y = start_y + row as u16 * step_y;

                if cell_x + cell_w > inner.x + inner.width
                    || cell_y + cell_h > inner.y + inner.height
                {
                    continue;
                }

                // Register per-cell hit target.
                register_hit(
                    Rect::new(cell_x, cell_y, cell_w, cell_h),
                    HitTarget::BoardCell(row, col),
                );

                let is_cursor = self.is_focused && self.cursor == (row, col);
                let border = theme::BORDER_NORMAL;

                if let Some(tile) = self.state.get_tile(row, col) {
                    let bg = if is_cursor {
                        theme::CURSOR_BG
                    } else {
                        theme::TILE_BG
                    };
                    let fg = if tile.is_blank {
                        theme::TILE_BLANK_FG
                    } else {
                        theme::CURSOR_FG
                    };
                    let value = if tile.is_blank {
                        0
                    } else {
                        letter_value(tile.letter)
                    };
                    self.draw_cell(
                        buf,
                        cell_x,
                        cell_y,
                        cell_w,
                        cell_h,
                        border,
                        Some(bg),
                        fg,
                        &tile.letter.to_string(),
                        is_cursor,
                        Some(value),
                    );
                } else if let Some(preview_tile) = self.preview.get(&(row, col)) {
                    let bg = if is_cursor {
                        theme::CURSOR_BG
                    } else {
                        theme::TILE_PREVIEW_BG
                    };
                    let fg = if is_cursor {
                        theme::CURSOR_FG
                    } else if preview_tile.is_blank {
                        theme::TILE_PREVIEW_BLANK_FG
                    } else {
                        theme::TILE_PREVIEW_FG
                    };
                    let value = if preview_tile.is_blank {
                        0
                    } else {
                        letter_value(preview_tile.letter)
                    };
                    self.draw_cell(
                        buf,
                        cell_x,
                        cell_y,
                        cell_w,
                        cell_h,
                        border,
                        Some(bg),
                        fg,
                        &preview_tile.letter.to_string(),
                        is_cursor,
                        Some(value),
                    );
                } else if is_cursor {
                    let board_cell = self
                        .state
                        .board
                        .get(row, col)
                        .expect("row/col within board dimensions");
                    let label = premium_label(&board_cell.premium);
                    self.draw_cell(
                        buf,
                        cell_x,
                        cell_y,
                        cell_w,
                        cell_h,
                        border,
                        Some(theme::CURSOR_BG),
                        theme::CURSOR_FG,
                        label,
                        true,
                        None,
                    );
                } else {
                    let board_cell = self
                        .state
                        .board
                        .get(row, col)
                        .expect("row/col within board dimensions");
                    let fg = premium_fg(&board_cell.premium);
                    let label = premium_label(&board_cell.premium);
                    // Without borders an empty plain square needs some ink
                    // or the grid disappears.
                    let label = if !self.geometry.bordered && label.is_empty() {
                        "\u{00b7}"
                    } else {
                        label
                    };
                    self.draw_cell(
                        buf, cell_x, cell_y, cell_w, cell_h, border, None, fg, label, false, None,
                    );
                }
            }
        }
    }
}

pub fn render_board(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &SolverState,
    cursor: (usize, usize),
    is_focused: bool,
    typing_direction: Direction,
    preview_tiles: Option<&[(usize, usize, Tile)]>,
) {
    let widget = BoardWidget::new(state, cursor, is_focused, typing_direction, preview_tiles);
    frame.render_widget(widget, area);
}
