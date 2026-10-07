//! Suggestions table: every candidate with its static equity and, once
//! evaluated, its simulation results.

use std::collections::HashMap;

use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{palette::tailwind, Color, Style, Stylize},
    symbols,
    text::Line,
    widgets::{Block, Cell, LineGauge, Padding, Paragraph, Row, Table, TableState, Tabs},
};

use crate::app::{group_thousands, SimProgress, SimResult, SimStatus, SortMode};
use crate::board::Direction;
use crate::solver::Move;
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::theme;

/// Width of the selection marker the table reserves on the left.
const HIGHLIGHT_SYMBOL: &str = "> ";

/// Maps moves to their evaluation results (playouts or endgame search).
pub struct SimEquityLookup<'a> {
    map: HashMap<(String, usize, usize, Direction), &'a SimResult>,
}

impl<'a> SimEquityLookup<'a> {
    pub fn new(results: Option<&'a [SimResult]>) -> Self {
        let map = results
            .map(|results| {
                results
                    .iter()
                    .map(|r| ((r.word.to_uppercase(), r.row, r.col, r.direction), r))
                    .collect()
            })
            .unwrap_or_default();
        SimEquityLookup { map }
    }

    pub fn get(&self, mv: &Move) -> Option<&'a SimResult> {
        self.map
            .get(&(mv.word.to_uppercase(), mv.row, mv.col, mv.direction))
            .copied()
    }

    pub fn has_data(&self) -> bool {
        !self.map.is_empty()
    }
}

/// A table column: header, width, and the sort a header click selects.
struct Column {
    title: &'static str,
    width: Constraint,
    sort: Option<SortMode>,
    right: bool,
}

/// Which columns fit in `width` (the table's inner width).
fn columns(has_sim: bool, width: u16) -> Vec<Column> {
    let col = |title, width, sort, right| Column {
        title,
        width,
        sort,
        right,
    };
    let mut cols = vec![
        col("Pos", Constraint::Length(4), None, false),
        col("Word", Constraint::Min(10), Some(SortMode::Alpha), false),
        col("Score", Constraint::Length(5), Some(SortMode::Score), true),
    ];
    if !has_sim {
        cols.push(col(
            "Equity",
            Constraint::Length(6),
            Some(SortMode::Equity),
            true,
        ));
        cols.push(col(
            "Leave",
            Constraint::Fill(1),
            Some(SortMode::Leave),
            false,
        ));
        return cols;
    }
    let wide = width >= 96;
    cols.push(col(
        "Spread",
        Constraint::Length(if wide { 11 } else { 6 }),
        Some(SortMode::MctsEquity),
        true,
    ));
    cols.push(col(
        "Win%",
        Constraint::Length(if wide { 8 } else { 5 }),
        Some(SortMode::WinPct),
        true,
    ));
    if width >= 52 {
        cols.push(col(
            "\u{0394}",
            Constraint::Length(if wide { 10 } else { 6 }),
            Some(SortMode::MctsEquity),
            true,
        ));
    }
    if width >= 64 {
        cols.push(col(
            "n",
            Constraint::Length(6),
            Some(SortMode::Playouts),
            true,
        ));
    }
    if width >= 76 {
        cols.push(col("Status", Constraint::Length(11), None, false));
    }
    if width >= 84 {
        cols.push(col(
            "Bingo",
            Constraint::Length(5),
            Some(SortMode::Bingo),
            true,
        ));
    }
    if width >= 92 {
        cols.push(col(
            "Leave",
            Constraint::Fill(1),
            Some(SortMode::Leave),
            false,
        ));
    }
    cols
}

/// `value` with its standard error when there is a meaningful one.
fn with_se(value: f32, se: f32, wide: bool, signed: bool) -> String {
    let v = if signed {
        format!("{:+.1}", value)
    } else {
        format!("{:.1}", value)
    };
    if wide && se.is_finite() && se > 0.0 {
        format!("{}\u{00b1}{:.1}", v, se)
    } else {
        v
    }
}

fn status_text(r: &SimResult) -> String {
    match r.status {
        SimStatus::Exact => "exact".to_string(),
        SimStatus::Racing => "racing".to_string(),
        SimStatus::Stopped => format!("stopped @{}", r.iterations),
        SimStatus::Capped => "capped".to_string(),
        SimStatus::Pending if r.iterations > 0 => "floor".to_string(),
        SimStatus::Pending => "\u{2013}".to_string(),
    }
}

/// One line above the table while (or after) simulating: coverage,
/// playouts, time, and a gauge of the run's budget.
fn render_progress(frame: &mut ratatui::Frame, area: Rect, progress: &SimProgress, running: bool) {
    let text = format!(
        "simulated {}/{} plays \u{00b7} {} playouts \u{00b7} {:.1}s{}",
        group_thousands(progress.simulated as u64),
        group_thousands(progress.total as u64),
        group_thousands(progress.playouts),
        progress.elapsed.as_secs_f32(),
        if progress.done {
            " \u{00b7} M continues"
        } else {
            ""
        }
    );
    let gauge_w = if running && area.width > text.len() as u16 + 12 {
        (area.width - text.len() as u16 - 2).min(24)
    } else {
        0
    };
    let [label, gauge] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(gauge_w)]).areas(area);
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(theme::TEXT_DIM)),
        label,
    );
    if gauge_w > 0 {
        let ratio = progress.budget_fraction.unwrap_or(0.0).clamp(0.0, 1.0) as f64;
        frame.render_widget(
            LineGauge::default()
                .filled_style(Style::new().fg(theme::SIM_EQUITY_FG))
                .unfilled_style(Style::new().fg(theme::TEXT_DIM))
                .ratio(ratio),
            gauge,
        );
    }
}

/// Render the suggestions panel. Only the visible rows are built, so the
/// full candidate list (tens of thousands of plays with blanks) costs no
/// more to draw than a page.
#[allow(clippy::too_many_arguments)]
pub fn render_suggestions(
    frame: &mut ratatui::Frame,
    area: Rect,
    moves: &[Move],
    selected: usize,
    is_focused: bool,
    sim_results: Option<&[SimResult]>,
    progress: Option<&SimProgress>,
    running: bool,
) {
    let border_color = if is_focused {
        theme::PANE_SUGGESTIONS
    } else {
        theme::BORDER_NORMAL
    };

    let lookup = SimEquityLookup::new(sim_results);
    let has_sim = lookup.has_data();

    let title = if moves.is_empty() {
        " Suggestions ".to_string()
    } else {
        format!(
            " Suggestions ({}){} ",
            group_thousands(moves.len() as u64),
            if has_sim { "*" } else { "" }
        )
    };

    let block = Block::bordered()
        .border_set(symbols::border::PROPORTIONAL_TALL)
        .title(title)
        .border_style(border_color)
        .padding(Padding::horizontal(1));

    // Register focus pane hit.
    register_hit(area, HitTarget::FocusPane(crate::app::Focus::Suggestions));

    if moves.is_empty() {
        let paragraph = Paragraph::new("Ctrl+S to find moves")
            .style(Style::new().fg(theme::TEXT_DIM))
            .block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (progress_area, table_area) = match progress {
        Some(_) if inner.height > 3 => {
            let [p, t] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
            (Some(p), t)
        }
        _ => (None, inner),
    };
    if let (Some(p_area), Some(progress)) = (progress_area, progress) {
        render_progress(frame, p_area, progress, running);
    }

    let cols = columns(has_sim, table_area.width);
    let wide = table_area.width >= 96;

    // Visible window: the selection stays on screen, scrolling just enough
    // (the persistent scroll offset comes with the list navigation work).
    let visible = table_area.height.saturating_sub(1) as usize; // minus header
    let offset = selected.saturating_sub(visible.saturating_sub(1));
    let end = (offset + visible).min(moves.len());

    // Click targets: header cells sort, rows select.
    let selection_w = HIGHLIGHT_SYMBOL.len() as u16;
    let columns_area = Rect {
        x: table_area.x + selection_w,
        width: table_area.width.saturating_sub(selection_w),
        ..table_area
    };
    let col_rects = Layout::horizontal(cols.iter().map(|c| c.width))
        .flex(Flex::Start)
        .spacing(1)
        .split(Rect {
            height: 1,
            ..columns_area
        });
    for (c, r) in cols.iter().zip(col_rects.iter()) {
        if let Some(mode) = c.sort {
            register_hit(*r, HitTarget::SortTab(mode));
        }
    }
    for (slot, i) in (offset..end).enumerate() {
        let row_y = table_area.y + 1 + slot as u16;
        register_hit(
            Rect::new(area.x, row_y, area.width, 1),
            HitTarget::SuggestionRow(i),
        );
    }

    let hdr_style = Style::new().fg(theme::TEXT_DIM);
    let header = Row::new(cols.iter().map(|c| {
        let line = Line::from(c.title);
        let line = if c.right {
            line.alignment(Alignment::Right)
        } else {
            line
        };
        Cell::from(line).style(hdr_style)
    }));

    let right = |text: String, style: Style| -> Cell<'static> {
        Cell::from(Line::from(text).alignment(Alignment::Right)).style(style)
    };
    let dash = || right("\u{2013}".to_string(), Style::new().fg(theme::TEXT_DIM));

    let rows: Vec<Row> = moves[offset..end]
        .iter()
        .map(|mv| {
            let sim = lookup.get(mv).filter(|r| r.is_simulated());
            let cells: Vec<Cell> = cols
                .iter()
                .map(|c| match c.title {
                    "Pos" => Cell::from(mv.coords()).style(Style::new().fg(theme::TEXT_DIM)),
                    "Word" => {
                        let text = if mv.is_bingo {
                            format!("{} \u{2605}", mv.word)
                        } else {
                            mv.word.clone()
                        };
                        Cell::from(text).style(Style::new().bold())
                    }
                    "Score" => right(mv.score.to_string(), Style::new().fg(theme::SCORE_FG)),
                    "Equity" => right(
                        format!("{:.0}", mv.equity),
                        Style::new().fg(theme::EQUITY_FG),
                    ),
                    "Leave" => {
                        let tiles: String = mv
                            .leave
                            .iter()
                            .map(|t| if t.is_blank { '?' } else { t.letter })
                            .collect();
                        let value = mv.equity - mv.score as f32;
                        let text = if tiles.is_empty() {
                            format!("({:+.0})", value)
                        } else {
                            format!("{} ({:+.0})", tiles, value)
                        };
                        Cell::from(text).style(Style::new().fg(theme::leave_color(value)))
                    }
                    "Spread" => match sim {
                        Some(r) => right(
                            with_se(r.sim_equity, r.spread_se, wide, true),
                            Style::new().fg(theme::SIM_EQUITY_FG),
                        ),
                        None => dash(),
                    },
                    "Win%" => match sim {
                        Some(r) => {
                            let text = if wide && r.win_se.is_finite() && r.win_se > 0.0 {
                                format!("{:.0}\u{00b1}{:.0}%", r.win_pct, r.win_se)
                            } else {
                                format!("{:.0}%", r.win_pct)
                            };
                            right(text, Style::new().fg(theme::win_color(r.win_pct)))
                        }
                        None => dash(),
                    },
                    "\u{0394}" => match sim {
                        Some(r) if r.delta == 0.0 && r.delta_se == 0.0 && !r.is_endgame => {
                            right("best".to_string(), Style::new().fg(theme::WIN_GOOD))
                        }
                        Some(r) if r.is_endgame && r.delta == 0.0 => {
                            right("best".to_string(), Style::new().fg(theme::WIN_GOOD))
                        }
                        Some(r) => right(
                            with_se(r.delta, r.delta_se, wide, true),
                            Style::new().fg(theme::TEXT_PRIMARY),
                        ),
                        None => dash(),
                    },
                    "n" => match sim {
                        Some(r) if r.is_endgame => dash(),
                        Some(r) => right(
                            group_thousands(r.iterations as u64),
                            Style::new().fg(theme::TEXT_DIM),
                        ),
                        None => dash(),
                    },
                    "Bingo" => match sim {
                        Some(r) if !r.is_endgame => {
                            let color = if r.ply2_bingo_pct >= 20.0 {
                                theme::WIN_GOOD
                            } else if r.ply2_bingo_pct >= 10.0 {
                                theme::EQUITY_FG
                            } else {
                                theme::TEXT_DIM
                            };
                            right(
                                format!("{:.0}/{:.0}", r.ply2_bingo_pct, r.ply1_bingo_pct),
                                Style::new().fg(color),
                            )
                        }
                        _ => dash(),
                    },
                    "Status" => {
                        let text = match lookup.get(mv) {
                            Some(r) => status_text(r),
                            None => "\u{2013}".to_string(),
                        };
                        let color = match lookup.get(mv).map(|r| r.status) {
                            Some(SimStatus::Racing) => theme::SIM_EQUITY_FG,
                            Some(SimStatus::Exact) => theme::WIN_GOOD,
                            _ => theme::TEXT_DIM,
                        };
                        Cell::from(text).style(Style::new().fg(color))
                    }
                    _ => Cell::from(""),
                })
                .collect();
            Row::new(cells)
        })
        .collect();

    let table = Table::new(rows, cols.iter().map(|c| c.width))
        .header(header)
        .row_highlight_style(theme::highlight_style())
        .highlight_symbol(HIGHLIGHT_SYMBOL);

    let mut state = TableState::default();
    state.select(Some(selected.saturating_sub(offset)));
    frame.render_stateful_widget(table, table_area, &mut state);
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
            SortMode::Playouts,
            SortMode::Bingo,
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
        if x >= area.x + area.width {
            break;
        }
        register_hit(Rect::new(x, area.y, label_w, 1), HitTarget::SortTab(*mode));
        x += label_w + 1; // +1 for divider space
    }

    frame.render_widget(tabs, area);
}
