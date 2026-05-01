//! Suggestions table with simulation equity lookup.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use ratatui::{
    layout::{Alignment, Constraint, Rect},
    style::{palette::tailwind, Color, Style, Stylize},
    symbols,
    text::Line,
    widgets::{Block, Cell, Padding, Paragraph, Row, Table, TableState, Tabs},
};

use crate::app::SortMode;
use crate::board::Direction;
use crate::logging::log;
use crate::refengine_mcts::RefSimResult;
use crate::mcts::MctsResult;
use crate::solver::Move;
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::theme;

fn log_suggestions(moves: &[Move], lookup: &SimEquityLookup, results_len: usize) {
    static LAST_LOGGED: AtomicU64 = AtomicU64::new(0);
    let hash = (results_len as u64) << 32 | (moves.len() as u64);
    if LAST_LOGGED.swap(hash, Ordering::SeqCst) == hash {
        return;
    }

    log(&format!("\n=== SOLVER SUGGESTIONS ({}) ===", moves.len()));
    for mv in moves {
        let sim = lookup.get_equity(&mv.word, mv.row, mv.col, mv.direction);
        log(&format!(
            "  {} at ({},{}) {:?} -> {:?}",
            mv.word, mv.row, mv.col, mv.direction, sim
        ));
    }
}

/// Simulation results for a single move from the RefEngine engine.
#[derive(Clone)]
pub struct SimData {
    pub sim_equity: f32,
    pub win_pct: f32,
    pub ply1_bingo_pct: f32,
    pub ply2_bingo_pct: f32,
    pub is_endgame: bool,
}

/// Maps moves to their simulation equity, combining MCTS and RefEngine results.
pub struct SimEquityLookup {
    mcts_map: HashMap<(String, usize, usize, Direction), f32>,
    refengine_map: HashMap<(String, usize, usize, Direction), SimData>,
}

impl SimEquityLookup {
    pub fn new(
        mcts_results: Option<&[MctsResult]>,
        refengine_results: Option<&[RefSimResult]>,
    ) -> Self {
        let mcts_map = mcts_results
            .map(|results| {
                results
                    .iter()
                    .map(|r| {
                        (
                            (r.mv.word.clone(), r.mv.row, r.mv.col, r.mv.direction),
                            r.mcts_equity,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let refengine_map: HashMap<(String, usize, usize, Direction), SimData> = refengine_results
            .map(|results| {
                results
                    .iter()
                    .map(|r| {
                        let word_upper = r.word.to_uppercase();
                        (
                            (word_upper, r.row, r.col, r.direction),
                            SimData {
                                sim_equity: r.sim_equity,
                                win_pct: r.win_pct,
                                ply1_bingo_pct: r.ply1_bingo_pct,
                                ply2_bingo_pct: r.ply2_bingo_pct,
                                is_endgame: r.is_endgame,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        SimEquityLookup {
            mcts_map,
            refengine_map,
        }
    }

    pub fn get_equity(
        &self,
        word: &str,
        row: usize,
        col: usize,
        direction: Direction,
    ) -> Option<f32> {
        let key = (word.to_string(), row, col, direction);
        self.refengine_map
            .get(&key)
            .map(|d| d.sim_equity)
            .or_else(|| self.mcts_map.get(&key).copied())
    }

    pub fn get_full(
        &self,
        word: &str,
        row: usize,
        col: usize,
        direction: Direction,
    ) -> Option<&SimData> {
        let key = (word.to_string(), row, col, direction);
        self.refengine_map.get(&key)
    }

    pub fn has_data(&self) -> bool {
        !self.mcts_map.is_empty() || !self.refengine_map.is_empty()
    }

    pub fn is_refengine(&self) -> bool {
        !self.refengine_map.is_empty()
    }
}

/// Render the suggestions panel.
pub fn render_suggestions(
    frame: &mut ratatui::Frame,
    area: Rect,
    moves: &[Move],
    selected: usize,
    is_focused: bool,
    mcts_results: Option<&[MctsResult]>,
    refengine_results: Option<&[RefSimResult]>,
) {
    let border_color = if is_focused {
        theme::PANE_SUGGESTIONS
    } else {
        theme::BORDER_NORMAL
    };

    let sim_lookup = SimEquityLookup::new(mcts_results, refengine_results);

    if let Some(results) = refengine_results {
        if !moves.is_empty() {
            log_suggestions(moves, &sim_lookup, results.len());
        }
    }
    let has_sim = sim_lookup.has_data();

    let title = if moves.is_empty() {
        " Suggestions ".to_string()
    } else if has_sim {
        format!(" Suggestions ({})* ", moves.len())
    } else {
        format!(" Suggestions ({}) ", moves.len())
    };

    let block = Block::bordered()
        .border_set(symbols::border::PROPORTIONAL_TALL)
        .title(title)
        .border_style(border_color)
        .padding(Padding::horizontal(1));

    // Register focus pane hit.
    register_hit(area, HitTarget::FocusPane(crate::app::Focus::Suggestions));

    if moves.is_empty() {
        let paragraph = Paragraph::new("Press S to find moves")
            .style(Style::new().fg(theme::TEXT_DIM))
            .block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let right = |text: &str, style: Style| -> Cell {
        Cell::from(Line::from(text.to_string()).alignment(Alignment::Right)).style(style)
    };
    let hdr_style = Style::new().fg(theme::TEXT_DIM);

    let header = if sim_lookup.is_refengine() {
        Row::new(vec![
            Cell::from("Word").style(hdr_style),
            right("Score", hdr_style),
            right("Equity", hdr_style),
            right("Win%", hdr_style),
            right("Bingo", hdr_style),
            Cell::from("Leave").style(hdr_style),
        ])
    } else if has_sim {
        Row::new(vec![
            Cell::from("Word").style(hdr_style),
            right("Score", hdr_style),
            right("Equity", hdr_style),
            right("Sim", hdr_style),
            Cell::from("Leave").style(hdr_style),
        ])
    } else {
        Row::new(vec![
            Cell::from("Word").style(hdr_style),
            right("Score", hdr_style),
            right("Equity", hdr_style),
            Cell::from("Leave").style(hdr_style),
        ])
    }
    .style(Style::new().fg(theme::TEXT_DIM))
    .bottom_margin(0);

    // Register per-row hits.
    // We need to estimate row positions. The block border is 1, header is 1, padding 0.
    let inner_start_y = area.y + 2; // border(1) + header(1)
    for (i, _) in moves.iter().enumerate() {
        let row_y = inner_start_y + i as u16;
        if row_y < area.y + area.height.saturating_sub(1) {
            register_hit(
                Rect::new(area.x, row_y, area.width, 1),
                HitTarget::SuggestionRow(i),
            );
        }
    }

    let rows: Vec<Row> = moves
        .iter()
        .map(|mv| {
            let word_text = if mv.is_bingo {
                format!("{} \u{2605}", mv.word)
            } else {
                mv.word.clone()
            };
            let word_cell = Cell::from(word_text).style(Style::new().bold());
            let pts_cell = right(&format!("{}", mv.score), Style::new().fg(theme::SCORE_FG));
            let equity_cell = right(
                &format!("{:.0}", mv.equity),
                Style::new().fg(theme::EQUITY_FG),
            );

            let leave_tiles: String = mv
                .leave
                .iter()
                .map(|t| if t.is_blank { '?' } else { t.letter })
                .collect();
            let leave_value = mv.equity - mv.score as f32;
            let leave_color = theme::leave_color(leave_value);
            let leave_text = if leave_tiles.is_empty() {
                format!("({:+.0})", leave_value)
            } else {
                format!("{} ({:+.0})", leave_tiles, leave_value)
            };
            let tiles_cell = Cell::from(leave_text).style(Style::new().fg(leave_color));

            if sim_lookup.is_refengine() {
                if let Some(sim) = sim_lookup.get_full(&mv.word, mv.row, mv.col, mv.direction) {
                    let sim_cell = right(
                        &format!("{:.0}", sim.sim_equity),
                        Style::new().fg(theme::SIM_EQUITY_FG),
                    );

                    let wc = theme::win_color(sim.win_pct);
                    let win_cell = if sim.is_endgame {
                        right(&format!("{:+.0}", sim.sim_equity), Style::new().fg(wc))
                    } else {
                        right(&format!("{:.0}%", sim.win_pct), Style::new().fg(wc))
                    };

                    let bingo_text = if sim.is_endgame {
                        "end".to_string()
                    } else {
                        format!("{:.0}/{:.0}", sim.ply2_bingo_pct, sim.ply1_bingo_pct)
                    };
                    let bingo_color = if sim.ply2_bingo_pct >= 20.0 {
                        theme::WIN_GOOD
                    } else if sim.ply2_bingo_pct >= 10.0 {
                        theme::EQUITY_FG
                    } else {
                        theme::TEXT_DIM
                    };
                    let bingo_cell = right(&bingo_text, Style::new().fg(bingo_color));

                    Row::new(vec![
                        word_cell, pts_cell, sim_cell, win_cell, bingo_cell, tiles_cell,
                    ])
                } else {
                    Row::new(vec![
                        word_cell,
                        pts_cell,
                        Cell::from("-").style(Style::new().fg(theme::TEXT_DIM)),
                        Cell::from("-").style(Style::new().fg(theme::TEXT_DIM)),
                        Cell::from("-").style(Style::new().fg(theme::TEXT_DIM)),
                        tiles_cell,
                    ])
                }
            } else if has_sim {
                let sim_cell = if let Some(equity) =
                    sim_lookup.get_equity(&mv.word, mv.row, mv.col, mv.direction)
                {
                    right(
                        &format!("{:.0}", equity),
                        Style::new().fg(theme::SIM_EQUITY_FG),
                    )
                } else {
                    right("-", Style::new().fg(theme::TEXT_DIM))
                };
                Row::new(vec![word_cell, pts_cell, equity_cell, sim_cell, tiles_cell])
            } else {
                Row::new(vec![word_cell, pts_cell, equity_cell, tiles_cell])
            }
        })
        .collect();

    let widths = if sim_lookup.is_refengine() {
        vec![
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Fill(1),
        ]
    } else if has_sim {
        vec![
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Fill(1),
        ]
    } else {
        vec![
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Fill(1),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme::highlight_style())
        .highlight_symbol("> ");

    let mut state = TableState::default();
    state.select(Some(selected));

    frame.render_stateful_widget(table, area, &mut state);
}

/// Render sort-mode tabs above the suggestions panel.
pub fn render_sort_tabs(
    frame: &mut ratatui::Frame,
    area: Rect,
    selected: SortMode,
    has_sim: bool,
    is_focused: bool,
) {
    let palette = tailwind::VIOLET;

    let modes: Vec<SortMode> = if has_sim {
        vec![
            SortMode::Equity,
            SortMode::MctsEquity,
            SortMode::WinPct,
            SortMode::Score,
            SortMode::Leave,
            SortMode::WordLength,
            SortMode::Alpha,
        ]
    } else {
        vec![
            SortMode::Equity,
            SortMode::Score,
            SortMode::Leave,
            SortMode::WordLength,
            SortMode::Alpha,
        ]
    };

    let selected_index = modes.iter().position(|m| *m == selected).unwrap_or(0);

    let titles: Vec<Line> = modes
        .iter()
        .map(|m| {
            format!(" {} ", m.label())
                .fg(tailwind::SLATE.c200)
                .bg(palette.c900)
                .into()
        })
        .collect();

    let highlight_color: Color = if is_focused {
        palette.c700
    } else {
        palette.c800
    };

    let tabs = Tabs::new(titles)
        .select(selected_index)
        .highlight_style((Color::default(), highlight_color))
        .padding("", "")
        .divider(" ");

    // Register hit targets for each tab.
    let mut x = area.x;
    for mode in &modes {
        let label_w = mode.label().len() as u16 + 2; // padding around label
        register_hit(Rect::new(x, area.y, label_w, 1), HitTarget::SortTab(*mode));
        x += label_w + 1; // +1 for divider space
    }

    frame.render_widget(tabs, area);
}
