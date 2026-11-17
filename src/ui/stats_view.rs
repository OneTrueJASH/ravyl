//! Combined rack tiles + game stats + opponent inference widget.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols;
use ratatui::widgets::{Block, Padding, Widget};

use crate::endgame::FinalTurn;
use crate::game::SolverState;
use crate::inference::RackInference as InferenceResult;
use crate::stats::{GamePhase, GameStats};
use crate::tile::Rack;
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::theme;

const TILE_WIDTH: u16 = 5;
const TILE_HEIGHT: u16 = 3;

/// Combined rack tile display with game statistics and opponent inference.
pub struct RackStatsWidget<'a> {
    state: &'a SolverState,
    rack: &'a Rack,
    is_focused: bool,
    inferred_rack: Option<&'a InferenceResult>,
    our_score: u32,
    opp_score: u32,
    final_turn: FinalTurn,
    final_turn_known: bool,
    /// Mean measured equity loss per opponent move and how many were
    /// measured, once any have been.
    opp_skill: Option<(f32, u32)>,
}

impl<'a> RackStatsWidget<'a> {
    pub fn new(
        state: &'a SolverState,
        is_focused: bool,
        inferred_rack: Option<&'a InferenceResult>,
        scores: (u32, u32),
        final_turn: (FinalTurn, bool),
        opp_skill: Option<(f32, u32)>,
    ) -> Self {
        Self {
            state,
            rack: &state.rack,
            is_focused,
            inferred_rack,
            our_score: scores.0,
            opp_score: scores.1,
            final_turn: final_turn.0,
            final_turn_known: final_turn.1,
            opp_skill,
        }
    }

    /// Rows the pane needs to show everything at `width` columns (its
    /// outer width), borders included. Mirrors `render`, line for line.
    pub fn rows_needed(&self, width: u16) -> u16 {
        let inner_w = width.saturating_sub(4); // border and padding
        let stats = GameStats::compute(self.state);
        let mut rows = TILE_HEIGHT; // the rack tiles
        rows += 2; // score, separator
        rows += 2; // rack line, separator
        rows += 1; // bag
        if stats.game_phase == GamePhase::Endgame {
            rows += 1; // final move
        }
        rows += 1; // power tiles
        if let Some(infer) = self.inferred_rack {
            if !infer.letter_marginals.is_empty() || !infer.top_leaves.is_empty() {
                rows += 1; // separator
            }
            if !infer.letter_marginals.is_empty() {
                if infer
                    .letter_marginals
                    .iter()
                    .any(|m| m.p_at_least_one >= 1.0)
                {
                    rows += 1; // kept letters
                }
                rows += 1; // played
            } else if !infer.top_leaves.is_empty() {
                rows += 2; // likely rack, played / kept
            }
        }
        if self.opp_skill.is_some() {
            rows += 1;
        }
        rows += 1; // unseen heading
        let unseen = self.state.unseen_tiles();
        let letters: Vec<u32> = ('A'..='Z')
            .filter_map(|c| unseen.get(&c).copied().filter(|&n| n > 0))
            .collect();
        let widest = letters
            .iter()
            .map(|n| n.to_string().len())
            .max()
            .unwrap_or(1) as u16;
        let cols = (inner_w / (3 + widest)).max(1) as usize;
        rows += letters.len().div_ceil(cols) as u16;
        rows + 2 // borders
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
                .bg(theme::current().tile_bg)
                .fg(theme::current().tile_blank_fg)
                .bold()
        } else {
            Style::new()
                .bg(theme::current().tile_bg)
                .fg(theme::current().tile_fg)
                .bold()
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
        let style = Style::new().bg(theme::current().tile_empty_bg);
        for dy in 0..TILE_HEIGHT {
            for dx in 0..TILE_WIDTH {
                if let Some(cell) = buf.cell_mut((x + dx, y + dy)) {
                    cell.set_char(' ').set_style(style);
                }
            }
        }
    }

    fn render_separator(buf: &mut Buffer, x: u16, y: u16, width: u16) {
        let style = Style::new().fg(theme::current().text_dim);
        for dx in 0..width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_char('\u{2500}').set_style(style);
            }
        }
    }
}

impl Widget for RackStatsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_color = if self.is_focused {
            theme::current().pane_rack
        } else {
            theme::current().border_normal
        };

        let title = if self.is_focused {
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
                        self.state.board.rules().letter_value(tile.letter)
                    };
                    Self::draw_tile(buf, tile_x, row, letter, value, tile.is_blank);
                } else {
                    Self::draw_empty_slot(buf, tile_x, row);
                }
            }
            row += TILE_HEIGHT;
        }

        // === Score line ===
        // Always shown, so whose number is whose (and how to set them) is
        // never a guess.
        if row < inner.y + inner.height {
            let diff = self.our_score as i32 - self.opp_score as i32;
            let score_text = format!("You {} \u{2013} Opp {}", self.our_score, self.opp_score);
            let diff_text = if self.our_score == 0 && self.opp_score == 0 {
                fit(
                    (inner.width as usize).saturating_sub(score_text.chars().count()),
                    "   + / = to set",
                    "",
                )
                .to_string()
            } else if diff > 0 {
                format!("  (+{})", diff)
            } else if diff < 0 {
                format!("  ({})", diff)
            } else {
                "  (tied)".to_string()
            };

            let mut x = inner.x;
            Self::render_text(
                buf,
                x,
                row,
                max_x,
                &score_text,
                Style::new().fg(theme::current().text_primary).bold(),
            );
            x += score_text.chars().count() as u16;

            let diff_color = if diff > 0 {
                Color::Green
            } else if diff < 0 {
                Color::Red
            } else {
                theme::current().text_dim
            };
            Self::render_text(buf, x, row, max_x, &diff_text, Style::new().fg(diff_color));
            row += 1;

            if row < inner.y + inner.height {
                Self::render_separator(buf, inner.x, row, inner.width);
                row += 1;
            }
        }

        // === Rack analysis ===
        if row < inner.y + inner.height {
            let stats = GameStats::compute(self.state);
            let width = inner.width as usize;

            // With no rack yet, say how to enter one; the rest of the pane
            // (bag, unseen tiles, the opponent) still applies.
            if stats.rack_value == 0 && stats.rack_blanks == 0 {
                let hint = fit(
                    width,
                    "Rack empty: Tab, then type (? = blank)",
                    "Rack empty: type your tiles",
                );
                Self::render_text(
                    buf,
                    inner.x,
                    row,
                    max_x,
                    hint,
                    Style::new().fg(theme::current().text_dim),
                );
            } else {
                let mut parts = vec![
                    plural(stats.rack_vowels, "vowel", "vowels"),
                    plural(stats.rack_consonants, "consonant", "consonants"),
                ];
                if stats.rack_blanks > 0 {
                    parts.push(plural(stats.rack_blanks, "blank", "blanks"));
                }
                parts.push(format!("{} pts", stats.rack_value));
                let long = parts.join(" \u{00b7} ");
                let short = format!(
                    "{}V {}C{} \u{00b7} {} pts",
                    stats.rack_vowels,
                    stats.rack_consonants,
                    if stats.rack_blanks > 0 {
                        format!(" {}?", stats.rack_blanks)
                    } else {
                        String::new()
                    },
                    stats.rack_value
                );
                let rack_text = if long.chars().count() <= width {
                    long
                } else {
                    short
                };
                let mut x = inner.x;
                Self::render_text(
                    buf,
                    x,
                    row,
                    max_x,
                    &rack_text,
                    Style::new().fg(theme::current().text_primary),
                );
                x += rack_text.chars().count() as u16;

                if !stats.rack_duplicates.is_empty() {
                    let dupes: String = stats.rack_duplicates.iter().collect();
                    let dupe_text = format!(" \u{00b7} dup {}", dupes);
                    if rack_text.chars().count() + dupe_text.chars().count() <= width {
                        Self::render_text(
                            buf,
                            x,
                            row,
                            max_x,
                            &dupe_text,
                            Style::new().fg(theme::current().score_fg),
                        );
                    }
                }
            }
            row += 1;

            if row < inner.y + inner.height {
                Self::render_separator(buf, inner.x, row, inner.width);
                row += 1;
            }

            // === Bag count + game phase + blanks/S ===
            if row < inner.y + inner.height {
                let pc = theme::current().phase_color(&stats.game_phase);

                let bag_text = format!("Bag ~{}", stats.estimated_bag);
                let phase_label = format!(" \u{00b7} {}", stats.game_phase.label());
                let unseen_label = " \u{00b7} unseen";
                let blank_text = format!(" ?{}", stats.blanks_unseen);
                let s_text = format!(" S{}", stats.s_unseen);

                let mut x = inner.x;
                Self::render_text(
                    buf,
                    x,
                    row,
                    max_x,
                    &bag_text,
                    Style::new().fg(theme::current().text_primary),
                );
                x += bag_text.chars().count() as u16;
                Self::render_text(buf, x, row, max_x, &phase_label, Style::new().fg(pc));
                x += phase_label.chars().count() as u16;
                Self::render_text(
                    buf,
                    x,
                    row,
                    max_x,
                    unseen_label,
                    Style::new().fg(theme::current().text_dim),
                );
                x += unseen_label.chars().count() as u16;

                let blank_style = if stats.blanks_unseen > 0 {
                    Style::new().fg(Color::Cyan).bold()
                } else {
                    Style::new().fg(theme::current().text_dim)
                };
                Self::render_text(buf, x, row, max_x, &blank_text, blank_style);
                x += blank_text.chars().count() as u16;

                let s_style = if stats.s_unseen > 0 {
                    Style::new().fg(Color::Green)
                } else {
                    Style::new().fg(theme::current().text_dim)
                };
                Self::render_text(buf, x, row, max_x, &s_text, s_style);
                row += 1;

                // Bag empty: who plays the final move (Ctrl+E toggles); an
                // unconfirmed guess says so.
                if stats.game_phase == GamePhase::Endgame && row < inner.y + inner.height {
                    let who = match self.final_turn {
                        FinalTurn::Us => "you",
                        FinalTurn::Opponent => "opponent",
                    };
                    let long = if self.final_turn_known {
                        format!("Final move: {who}  (^E to change)")
                    } else {
                        format!("Final move: {who}? (assumed, ^E to change)")
                    };
                    let short = format!(
                        "Final: {}{}  ^E",
                        who,
                        if self.final_turn_known { "" } else { "?" }
                    );
                    let text = if long.chars().count() <= width {
                        long
                    } else {
                        short
                    };
                    Self::render_text(buf, inner.x, row, max_x, &text, Style::new().fg(pc).bold());
                    row += 1;
                }
            }

            // === Power tiles (Q, Z, J, X), grouped by where they are ===
            if row < inner.y + inner.height {
                let t = theme::current();
                let groups: [(&str, Style, Vec<char>); 4] = {
                    let mut unseen = Vec::new();
                    let mut yours = Vec::new();
                    let mut mixed = Vec::new();
                    let mut played = Vec::new();
                    for letter in ['Q', 'Z', 'J', 'X'] {
                        match stats.power_tiles.get(&letter) {
                            Some(crate::stats::TileLocation::InBag(n)) if *n > 0 => {
                                unseen.push(letter)
                            }
                            Some(crate::stats::TileLocation::InRack) => yours.push(letter),
                            Some(
                                crate::stats::TileLocation::BoardAndRack
                                | crate::stats::TileLocation::Split,
                            ) => mixed.push(letter),
                            Some(_) => played.push(letter),
                            None => {}
                        }
                    }
                    [
                        ("unseen", Style::new().fg(t.text_primary), unseen),
                        ("yours", Style::new().fg(t.phase_endgame).bold(), yours),
                        ("split", Style::new().fg(t.score_fg), mixed),
                        ("played", Style::new().fg(t.text_dim), played),
                    ]
                };
                let segments: Vec<(String, Style)> = groups
                    .iter()
                    .filter(|(_, _, letters)| !letters.is_empty())
                    .map(|(what, style, letters)| {
                        let letters: Vec<String> = letters.iter().map(|c| c.to_string()).collect();
                        (format!("{} {}", letters.join(" "), what), *style)
                    })
                    .collect();
                let sep = " \u{00b7} ";
                let body_width: usize = segments
                    .iter()
                    .map(|(text, _)| text.chars().count())
                    .sum::<usize>()
                    + sep.chars().count() * segments.len().saturating_sub(1);
                let label = "Power tiles: ";
                let mut x = inner.x;
                if label.chars().count() + body_width <= width {
                    Self::render_text(buf, x, row, max_x, label, Style::new().fg(t.text_dim));
                    x += label.chars().count() as u16;
                }
                for (i, (text, style)) in segments.iter().enumerate() {
                    if i > 0 {
                        Self::render_text(buf, x, row, max_x, sep, Style::new().fg(t.text_dim));
                        x += sep.chars().count() as u16;
                    }
                    Self::render_text(buf, x, row, max_x, text, *style);
                    x += text.chars().count() as u16;
                }
                row += 1;
            }

            // === Inferred opponent rack ===
            if let Some(infer) = self.inferred_rack {
                let has_signal = !infer.letter_marginals.is_empty() || !infer.top_leaves.is_empty();
                if has_signal && row < inner.y + inner.height {
                    Self::render_separator(buf, inner.x, row, inner.width);
                    row += 1;
                }
                if !infer.letter_marginals.is_empty() {
                    // The letters they most likely kept. The blank is the
                    // headline threat, so it earns a slot whenever it is a
                    // live possibility.
                    let mut marginals: Vec<_> = infer
                        .letter_marginals
                        .iter()
                        .filter(|m| m.p_at_least_one >= 1.0)
                        .collect();
                    marginals.sort_by(|a, b| {
                        b.p_at_least_one
                            .partial_cmp(&a.p_at_least_one)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    if let Some(i) = marginals.iter().position(|m| m.letter == '?') {
                        if i >= 4 && marginals[i].p_at_least_one >= 5.0 {
                            let blank = marginals.remove(i);
                            marginals.insert(3, blank);
                        }
                    }
                    marginals.truncate(4);
                    if !marginals.is_empty() && row < inner.y + inner.height {
                        let parts: Vec<String> = marginals
                            .iter()
                            .map(|m| format!("{} {:.0}%", m.letter, m.p_at_least_one))
                            .collect();
                        let text = format!("Opp kept: {}", parts.join("  "));
                        Self::render_text(
                            buf,
                            inner.x,
                            row,
                            max_x,
                            &text,
                            Style::new().fg(theme::current().phase_endgame).bold(),
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
                        let detail = format!("  played {}", played_spaced);
                        Self::render_text(
                            buf,
                            inner.x,
                            row,
                            max_x,
                            &detail,
                            Style::new().fg(theme::current().text_dim),
                        );
                        row += 1;
                    }
                } else if let Some((top_leave, pct)) = infer.top_leaves.first() {
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
                            Style::new().fg(theme::current().phase_endgame).bold(),
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
                            Style::new().fg(theme::current().text_dim),
                        );
                        row += 1;
                    }
                }
            }

            // === Measured opponent strength ===
            if let Some((mean_loss, moves)) = self.opp_skill {
                if row < inner.y + inner.height {
                    let text = format!("Opp error: {:.1} pts/move ({} measured)", mean_loss, moves);
                    Self::render_text(
                        buf,
                        inner.x,
                        row,
                        max_x,
                        &text,
                        Style::new().fg(theme::current().text_dim),
                    );
                    row += 1;
                }
            }

            // === Unseen letter grid, under a heading with the total ===
            let unseen = self.state.unseen_tiles();
            if row < inner.y + inner.height {
                let total: u32 = unseen.values().sum();
                Self::render_separator(buf, inner.x, row, inner.width);
                Self::render_text(
                    buf,
                    inner.x + 2,
                    row,
                    max_x,
                    &format!(" Unseen tiles: {total} "),
                    Style::new().fg(theme::current().text_dim),
                );
                row += 1;
            }
            let letters: Vec<char> = ('A'..='Z')
                .filter(|c| unseen.get(c).copied().unwrap_or(0) > 0)
                .collect();
            // Wide enough for "E:12" plus a space when any count has two digits.
            let widest = letters
                .iter()
                .map(|c| unseen.get(c).copied().unwrap_or(0).to_string().len())
                .max()
                .unwrap_or(1) as u16;
            let col_width = 3 + widest;
            let cols = (inner.width / col_width).max(1) as usize;

            let mut col = 0usize;
            for letter in &letters {
                if row >= inner.y + inner.height {
                    break;
                }

                let count = unseen.get(letter).copied().unwrap_or(0);
                let style = if count <= 2 {
                    Style::new().fg(theme::current().score_fg)
                } else {
                    Style::new().fg(theme::current().text_primary)
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

/// One-line rack for narrow (stacked) layouts: the tiles, how many, and
/// the score.
pub fn render_rack_line(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &SolverState,
    is_focused: bool,
    scores: (u32, u32),
) {
    let t = theme::current();
    let tiles: Vec<String> = state
        .rack
        .tiles()
        .iter()
        .map(|t| {
            if t.is_blank {
                "?".to_string()
            } else {
                t.letter.to_string()
            }
        })
        .collect();
    let label_style = if is_focused {
        Style::new().fg(t.pane_rack).bold()
    } else {
        Style::new().fg(t.text_dim)
    };
    let diff = scores.0 as i32 - scores.1 as i32;
    let line = ratatui::text::Line::from(vec![
        ratatui::text::Span::styled(" Rack ", label_style),
        ratatui::text::Span::styled(
            if tiles.is_empty() {
                "(empty)".to_string()
            } else {
                tiles.join(" ")
            },
            Style::new().fg(t.text_primary).bold(),
        ),
        ratatui::text::Span::styled(
            format!("   {}\u{2013}{} ({:+})", scores.0, scores.1, diff),
            Style::new().fg(t.text_dim),
        ),
    ]);
    register_hit(area, HitTarget::FocusPane(crate::app::Focus::Rack));
    frame.render_widget(ratatui::widgets::Paragraph::new(line), area);
}

#[allow(clippy::too_many_arguments)]
pub fn render_rack_stats(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &SolverState,
    is_focused: bool,
    inferred_rack: Option<&InferenceResult>,
    scores: (u32, u32),
    final_turn: (FinalTurn, bool),
    opp_skill: Option<(f32, u32)>,
) {
    let widget = RackStatsWidget::new(
        state,
        is_focused,
        inferred_rack,
        scores,
        final_turn,
        opp_skill,
    );
    frame.render_widget(widget, area);
}

/// `long` when it fits in `width` columns, otherwise `short`.
fn fit<'s>(width: usize, long: &'s str, short: &'s str) -> &'s str {
    if long.chars().count() <= width {
        long
    } else {
        short
    }
}

/// "1 vowel", "3 vowels".
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Rows the full stats pane needs at `width` columns, for the layout.
pub fn rack_stats_rows(
    width: u16,
    state: &SolverState,
    inferred_rack: Option<&InferenceResult>,
    scores: (u32, u32),
    final_turn: (FinalTurn, bool),
    opp_skill: Option<(f32, u32)>,
) -> u16 {
    RackStatsWidget::new(state, false, inferred_rack, scores, final_turn, opp_skill)
        .rows_needed(width)
}

/// Three-line rack summary for the compact sidebar beside a mini board:
/// the rack, the score, and the bag with the unseen blanks and S's.
pub fn render_rack_summary(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &SolverState,
    is_focused: bool,
    scores: (u32, u32),
) {
    let t = theme::current();
    let stats = GameStats::compute(state);
    let tiles: Vec<String> = state
        .rack
        .tiles()
        .iter()
        .map(|t| {
            if t.is_blank {
                "?".to_string()
            } else {
                t.letter.to_string()
            }
        })
        .collect();
    let label_style = if is_focused {
        Style::new().fg(t.pane_rack).bold()
    } else {
        Style::new().fg(t.text_dim)
    };
    let rack = if tiles.is_empty() {
        ratatui::text::Span::styled(
            fit(
                area.width.saturating_sub(6) as usize,
                "empty: Tab, type tiles",
                "empty",
            ),
            Style::new().fg(t.text_dim),
        )
    } else {
        ratatui::text::Span::styled(tiles.join(" "), Style::new().fg(t.text_primary).bold())
    };
    let diff = scores.0 as i32 - scores.1 as i32;
    let lines = vec![
        ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(" Rack ", label_style),
            rack,
        ]),
        ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                format!(" You {} \u{2013} Opp {}", scores.0, scores.1),
                Style::new().fg(t.text_primary),
            ),
            ratatui::text::Span::styled(
                if diff == 0 {
                    String::new()
                } else {
                    format!("  ({:+})", diff)
                },
                Style::new().fg(if diff > 0 { Color::Green } else { Color::Red }),
            ),
        ]),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            format!(
                " Bag ~{} \u{00b7} {} \u{00b7} ?{} S{}",
                stats.estimated_bag,
                stats.game_phase.label(),
                stats.blanks_unseen,
                stats.s_unseen
            ),
            Style::new().fg(t.text_dim),
        )),
    ];
    register_hit(area, HitTarget::FocusPane(crate::app::Focus::Rack));
    frame.render_widget(ratatui::widgets::Paragraph::new(lines), area);
}
