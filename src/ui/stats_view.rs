//! Combined rack tiles + game stats + opponent inference widget.

use crate::game::SolverState;
use crate::refengine_mcts::InferenceResult;
use crate::stats::GameStats;
use crate::tile::{letter_value, Rack};
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    symbols,
    widgets::{Block, Padding, Widget},
};

const TILE_WIDTH: u16 = 5;
const TILE_HEIGHT: u16 = 3;

/// Combined rack tile display with game statistics and opponent inference.
pub struct RackStatsWidget<'a> {
    state: &'a SolverState,
    rack: &'a Rack,
    focused: bool,
    inferred_rack: Option<&'a InferenceResult>,
}

impl<'a> RackStatsWidget<'a> {
    pub fn new(
        state: &'a SolverState,
        rack: &'a Rack,
        focused: bool,
        inferred_rack: Option<&'a InferenceResult>,
    ) -> Self {
        Self {
            state,
            rack,
            focused,
            inferred_rack,
        }
    }

    fn render_text(buf: &mut Buffer, x: u16, y: u16, max_x: u16, text: &str, style: Style) {
        for (i, ch) in text.chars().enumerate() {
            let px = x + i as u16;
            if px < max_x {
                if let Some(cell) = buf.cell_mut((px, y)) {
                    cell.set_char(ch).set_style(style);
                }
            }
        }
    }

    fn draw_tile(buf: &mut Buffer, x: u16, y: u16, letter: char, value: u32, is_blank: bool) {
        let style = if is_blank {
            Style::new()
                .bg(theme::TILE_BG)
                .fg(theme::TILE_BLANK_FG)
                .bold()
        } else {
            Style::new().bg(theme::TILE_BG).fg(theme::TILE_FG).bold()
        };

        for dy in 0..TILE_HEIGHT {
            for dx in 0..TILE_WIDTH {
                if let Some(cell) = buf.cell_mut((x + dx, y + dy)) {
                    cell.set_char(' ').set_style(style);
                }
            }
        }

        let letter_x = x + TILE_WIDTH / 2;
        let letter_y = y + TILE_HEIGHT / 2;
        if let Some(cell) = buf.cell_mut((letter_x, letter_y)) {
            cell.set_char(letter).set_style(style);
        }

        let value_str = value.to_string();
        let value_x = x + TILE_WIDTH - 1 - value_str.len() as u16;
        for (i, ch) in value_str.chars().enumerate() {
            if let Some(cell) = buf.cell_mut((value_x + i as u16, y)) {
                cell.set_char(ch).set_style(style);
            }
        }
    }

    fn draw_empty_slot(buf: &mut Buffer, x: u16, y: u16) {
        let style = Style::new().bg(theme::TILE_EMPTY_BG);
        for dy in 0..TILE_HEIGHT {
            for dx in 0..TILE_WIDTH {
                if let Some(cell) = buf.cell_mut((x + dx, y + dy)) {
                    cell.set_char(' ').set_style(style);
                }
            }
        }
    }

    fn render_separator(buf: &mut Buffer, x: u16, y: u16, width: u16) {
        let style = Style::new().fg(theme::TEXT_DIM);
        for dx in 0..width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_char('\u{2500}').set_style(style);
            }
        }
    }
}

impl Widget for RackStatsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_color = if self.focused {
            theme::PANE_RACK
        } else {
            theme::BORDER_NORMAL
        };

        let title = if self.focused {
            " Rack (type to add, Backspace to remove) "
        } else {
            " Rack "
        };

        let block = Block::bordered()
            .border_set(symbols::border::PROPORTIONAL_TALL)
            .title(title)
            .border_style(border_color)
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        block.render(area, buf);

        // Register focus pane hit.
        register_hit(area, HitTarget::FocusPane(crate::app::Focus::Rack));

        if inner.height < 1 || inner.width < 10 {
            return;
        }

        let max_x = inner.x + inner.width;
        let mut row = inner.y;

        // === Rack tiles ===
        if inner.height >= TILE_HEIGHT {
            let total_tile_width = 7 * TILE_WIDTH + 6;
            let start_x = inner.x + inner.width.saturating_sub(total_tile_width) / 2;

            for (i, slot) in self.rack.slots.iter().enumerate() {
                let tile_x = start_x + i as u16 * (TILE_WIDTH + 1);
                // Register rack slot hit.
                register_hit(
                    Rect::new(tile_x, row, TILE_WIDTH, TILE_HEIGHT),
                    HitTarget::RackSlot(i),
                );
                if let Some(tile) = slot {
                    let letter = if tile.is_blank { '?' } else { tile.letter };
                    let value = if tile.is_blank {
                        0
                    } else {
                        letter_value(tile.letter)
                    };
                    Self::draw_tile(buf, tile_x, row, letter, value, tile.is_blank);
                } else {
                    Self::draw_empty_slot(buf, tile_x, row);
                }
            }
            row += TILE_HEIGHT;
        }

        // === Rack analysis ===
        if row < inner.y + inner.height {
            let stats = GameStats::compute(self.state);

            let rack_text = if stats.rack_blanks > 0 {
                format!(
                    "{}V {}C {}? ({} pts)",
                    stats.rack_vowels, stats.rack_consonants, stats.rack_blanks, stats.rack_value
                )
            } else if stats.rack_value > 0 {
                format!(
                    "{}V {}C ({} pts)",
                    stats.rack_vowels, stats.rack_consonants, stats.rack_value
                )
            } else {
                return;
            };

            let mut x = inner.x;
            Self::render_text(
                buf,
                x,
                row,
                max_x,
                &rack_text,
                Style::new().fg(theme::TEXT_PRIMARY),
            );
            x += rack_text.len() as u16;

            if !stats.rack_duplicates.is_empty() {
                let dupes: String = stats.rack_duplicates.iter().collect();
                let dupe_text = format!("  dup: {}", dupes);
                Self::render_text(
                    buf,
                    x,
                    row,
                    max_x,
                    &dupe_text,
                    Style::new().fg(theme::SCORE_FG),
                );
            }
            row += 1;

            if row < inner.y + inner.height {
                Self::render_separator(buf, inner.x, row, inner.width);
                row += 1;
            }

            // === Bag count + game phase + blanks/S ===
            if row < inner.y + inner.height {
                let pc = theme::phase_color(&stats.game_phase);

                let bag_text = format!("~{}", stats.estimated_bag);
                let phase_label = format!(" [{}]", stats.game_phase.label());
                let blank_text = format!("  ?:{}", stats.blanks_unseen);
                let s_text = format!(" S:{}", stats.s_unseen);

                let mut x = inner.x;
                Self::render_text(
                    buf,
                    x,
                    row,
                    max_x,
                    &bag_text,
                    Style::new().fg(theme::TEXT_PRIMARY),
                );
                x += bag_text.len() as u16;
                Self::render_text(buf, x, row, max_x, &phase_label, Style::new().fg(pc));
                x += phase_label.len() as u16;

                let blank_style = if stats.blanks_unseen > 0 {
                    Style::new().fg(Color::Cyan).bold()
                } else {
                    Style::new().fg(theme::TEXT_DIM)
                };
                Self::render_text(buf, x, row, max_x, &blank_text, blank_style);
                x += blank_text.len() as u16;

                let s_style = if stats.s_unseen > 0 {
                    Style::new().fg(Color::Green)
                } else {
                    Style::new().fg(theme::TEXT_DIM)
                };
                Self::render_text(buf, x, row, max_x, &s_text, s_style);
                row += 1;
            }

            // === Power tiles (Q, Z, J, X) ===
            if row < inner.y + inner.height {
                let mut x = inner.x;
                for letter in ['Q', 'Z', 'J', 'X'] {
                    if let Some(loc) = stats.power_tiles.get(&letter) {
                        let (text, style) = match loc {
                            crate::stats::TileLocation::InBag(n) if *n > 0 => (
                                format!("{}:{}", letter, n),
                                Style::new().fg(theme::TEXT_PRIMARY),
                            ),
                            crate::stats::TileLocation::OnBoard => {
                                (format!("{}:B", letter), Style::new().fg(theme::TEXT_DIM))
                            }
                            crate::stats::TileLocation::InRack => (
                                format!("{}:R", letter),
                                Style::new().fg(theme::PHASE_ENDGAME).bold(),
                            ),
                            crate::stats::TileLocation::BoardAndRack => {
                                (format!("{}:BR", letter), Style::new().fg(theme::SCORE_FG))
                            }
                            crate::stats::TileLocation::Split => {
                                (format!("{}:?", letter), Style::new().fg(theme::SCORE_FG))
                            }
                            _ => (format!("{}:-", letter), Style::new().fg(theme::TEXT_DIM)),
                        };
                        Self::render_text(buf, x, row, max_x, &text, style);
                        x += text.len() as u16 + 1;
                    }
                }
                row += 1;
            }

            // === Inferred opponent rack ===
            if let Some(infer) = self.inferred_rack {
                if !infer.top_leaves.is_empty() && row < inner.y + inner.height {
                    Self::render_separator(buf, inner.x, row, inner.width);
                    row += 1;
                }
                if let Some((top_leave, pct)) = infer.top_leaves.first() {
                    if row < inner.y + inner.height {
                        let mut full_rack: Vec<char> = infer
                            .played_tiles
                            .chars()
                            .chain(top_leave.chars())
                            .collect();
                        full_rack.sort();
                        let spaced: String = full_rack
                            .iter()
                            .map(|c| c.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let text = format!("Opp rack: {}  ({:.0}%)", spaced, pct);
                        Self::render_text(
                            buf,
                            inner.x,
                            row,
                            max_x,
                            &text,
                            Style::new().fg(theme::PHASE_ENDGAME).bold(),
                        );
                        row += 1;
                    }
                    if row < inner.y + inner.height {
                        let played_spaced: String = infer
                            .played_tiles
                            .chars()
                            .map(|c| c.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let kept_spaced: String = top_leave
                            .chars()
                            .map(|c| c.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let detail = format!("  played {} / kept {}", played_spaced, kept_spaced);
                        Self::render_text(
                            buf,
                            inner.x,
                            row,
                            max_x,
                            &detail,
                            Style::new().fg(theme::TEXT_DIM),
                        );
                        row += 1;
                    }
                }
            }

            // === Separator before unseen grid ===
            if row < inner.y + inner.height {
                Self::render_separator(buf, inner.x, row, inner.width);
                row += 1;
            }

            // === Unseen letter grid ===
            let unseen = self.state.unseen_tiles();
            let letters: Vec<char> = ('A'..='Z')
                .filter(|c| unseen.get(c).copied().unwrap_or(0) > 0)
                .collect();
            let col_width = 4u16;
            let cols = (inner.width / col_width).max(1) as usize;

            let mut col = 0usize;
            for letter in &letters {
                if row >= inner.y + inner.height {
                    break;
                }

                let count = unseen.get(letter).copied().unwrap_or(0);
                let style = if count <= 2 {
                    Style::new().fg(theme::SCORE_FG)
                } else {
                    Style::new().fg(theme::TEXT_PRIMARY)
                };

                let x = inner.x + (col as u16 * col_width);
                let text = format!("{}:{}", letter, count);
                Self::render_text(buf, x, row, max_x, &text, style);

                col += 1;
                if col >= cols {
                    col = 0;
                    row += 1;
                }
            }
        }
    }
}

pub fn render_rack_stats(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &SolverState,
    rack: &Rack,
    focused: bool,
    inferred_rack: Option<&InferenceResult>,
) {
    let widget = RackStatsWidget::new(state, rack, focused, inferred_rack);
    frame.render_widget(widget, area);
}
