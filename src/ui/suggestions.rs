//! Suggestions table: every candidate with its static equity and, once
//! evaluated, its simulation results.

use std::collections::HashMap;

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols;
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, LineGauge, Padding, Paragraph, Row, Table, TableState, Tabs};

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

/// The move's word with letters played from blanks in lowercase, the
/// usual notation.
pub fn display_word(mv: &Move) -> String {
    if mv.kind != crate::solver::MoveKind::Play || mv.tiles_used.iter().all(|(_, _, t)| !t.is_blank)
    {
        return mv.word.clone();
    }
    let (dr, dc) = mv.direction.delta();
    mv.word
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            let r = (mv.row as i32 + dr * i as i32) as usize;
            let c = (mv.col as i32 + dc * i as i32) as usize;
            let blank = mv
                .tiles_used
                .iter()
                .any(|(pr, pc, t)| *pr == r && *pc == c && t.is_blank);
            if blank {
                ch.to_ascii_lowercase()
            } else {
                ch
            }
        })
        .collect()
}

/// A table column: header, width, and the sort a header click selects.
struct Column {
    title: &'static str,
    width: Constraint,
    sort: Option<SortMode>,
    align: Alignment,
}

/// Which columns fit in `width` (the table's inner width).
fn columns(has_sim: bool, width: u16) -> Vec<Column> {
    let col = |title, width, sort, align| Column {
        title,
        width,
        sort,
        align,
    };
    let mut cols = vec![
        col("Pos", Constraint::Length(4), None, Alignment::Left),
        col(
            "Word",
            Constraint::Min(10),
            Some(SortMode::Alpha),
            Alignment::Left,
        ),
        col(
            "Score",
            Constraint::Length(5),
            Some(SortMode::Score),
            Alignment::Right,
        ),
    ];
    if !has_sim {
        cols.push(col(
            "Equity",
            Constraint::Length(6),
            Some(SortMode::Equity),
            Alignment::Right,
        ));
        cols.push(col(
            "Leave",
            Constraint::Fill(1),
            Some(SortMode::Leave),
            Alignment::Left,
        ));
        return cols;
    }
    let wide = width >= 96;
    cols.push(col(
        "Spread",
        Constraint::Length(if wide { 11 } else { 6 }),
        Some(SortMode::MctsEquity),
        Alignment::Right,
    ));
    cols.push(col(
        "Win%",
        Constraint::Length(if wide { 8 } else { 5 }),
        Some(SortMode::WinPct),
        Alignment::Right,
    ));
    if width >= 52 {
        cols.push(col(
            "\u{0394}",
            Constraint::Length(if wide { 10 } else { 6 }),
            Some(SortMode::MctsEquity),
            Alignment::Right,
        ));
    }
    if width >= 64 {
        cols.push(col(
            "n",
            Constraint::Length(6),
            Some(SortMode::Playouts),
            Alignment::Right,
        ));
    }
    if width >= 76 {
        cols.push(col("Status", Constraint::Length(11), None, Alignment::Left));
    }
    if width >= 84 {
        cols.push(col(
            "Bingo",
            Constraint::Length(5),
            Some(SortMode::Bingo),
            Alignment::Right,
        ));
    }
    if width >= 92 {
        cols.push(col(
            "Leave",
            Constraint::Fill(1),
            Some(SortMode::Leave),
            Alignment::Left,
        ));
    }
    cols
}

/// `value`, signed, with its standard error when there is a meaningful one.
fn with_se(value: f32, se: f32, wide: bool) -> String {
    let v = format!("{value:+.1}");
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
        Paragraph::new(text).style(Style::new().fg(theme::current().text_dim)),
        label,
    );
    if gauge_w > 0 {
        let ratio = progress.budget_fraction.unwrap_or(0.0).clamp(0.0, 1.0) as f64;
        frame.render_widget(
            LineGauge::default()
                .filled_style(Style::new().fg(theme::current().sim_equity_fg))
                .unfilled_style(Style::new().fg(theme::current().text_dim))
                .ratio(ratio),
            gauge,
        );
    }
}

/// Rows kept visible above and below the selection when scrolling.
const SCROLL_OFF: usize = 2;

/// What the list shows and where it is scrolled to.
pub struct ListView<'a> {
    /// Every suggestion.
    pub moves: &'a [Move],
    /// Indices into `moves` of the rows shown (all, or the filter's matches).
    pub visible: &'a [usize],
    /// Index into `moves` of the selected suggestion.
    pub selected: usize,
    /// First shown row (an index into `visible`), kept between frames.
    pub offset: &'a std::cell::Cell<usize>,
    /// Rows shown, recorded for paging.
    pub height: &'a std::cell::Cell<usize>,
    pub filter: Option<&'a str>,
    pub filter_editing: bool,
}

/// Scroll `offset` just enough that `pos` sits at least `SCROLL_OFF` rows
/// inside a window of `height` rows over `len` rows.
pub fn scroll_to(offset: usize, pos: usize, height: usize, len: usize) -> usize {
    if height == 0 {
        return 0;
    }
    let so = SCROLL_OFF.min(height.saturating_sub(1) / 2);
    let mut off = offset;
    if pos < off + so {
        off = pos.saturating_sub(so);
    } else if pos + so >= off + height {
        off = pos + so + 1 - height;
    }
    off.min(len.saturating_sub(height))
}

/// Render the suggestions panel. Only the visible rows are built, so the
/// full candidate list (tens of thousands of plays with blanks) costs no
/// more to draw than a page.
pub fn render_suggestions(
    frame: &mut ratatui::Frame,
    area: Rect,
    view: ListView,
    is_focused: bool,
    sim_results: Option<&[SimResult]>,
    progress: Option<&SimProgress>,
    running: bool,
) {
    let border_color = if is_focused {
        theme::current().pane_suggestions
    } else {
        theme::current().border_normal
    };

    let moves = view.moves;
    let lookup = SimEquityLookup::new(sim_results);
    let has_sim = lookup.has_data();
    let pos = view.visible.iter().position(|&i| i == view.selected);

    // Title: count, and the selection's place in the list ("12/340").
    let title = if moves.is_empty() {
        " Suggestions ".to_string()
    } else {
        format!(
            " Suggestions {}/{}{} ",
            pos.map_or(0, |p| p + 1),
            group_thousands(view.visible.len() as u64),
            if has_sim { "*" } else { "" }
        )
    };
    let filter_title = view.filter.map(|f| {
        let cursor = if view.filter_editing { "_" } else { "" };
        format!(" /{}{} ", f, cursor)
    });

    let mut block = Block::bordered()
        .border_set(symbols::border::PROPORTIONAL_TALL)
        .title(title)
        .border_style(border_color)
        .padding(Padding::horizontal(1));
    if let Some(f) = filter_title {
        block = block.title(Line::from(f).alignment(Alignment::Right));
    }

    // Register focus pane hit.
    register_hit(area, HitTarget::FocusPane(crate::app::Focus::Suggestions));

    if moves.is_empty() {
        let paragraph = Paragraph::new("Ctrl+S to find moves")
            .style(Style::new().fg(theme::current().text_dim))
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

    // Visible window, kept between frames: it scrolls only as far as
    // needed to keep the selection SCROLL_OFF rows inside it.
    let rows_shown = table_area.height.saturating_sub(1) as usize; // minus header
    let len = view.visible.len();
    let offset = scroll_to(view.offset.get(), pos.unwrap_or(0), rows_shown, len);
    view.offset.set(offset);
    view.height.set(rows_shown);
    let end = (offset + rows_shown).min(len);

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
    for (slot, &i) in view.visible[offset..end].iter().enumerate() {
        let row_y = table_area.y + 1 + slot as u16;
        register_hit(
            Rect::new(area.x, row_y, area.width, 1),
            HitTarget::SuggestionRow(i),
        );
    }

    let hdr_style = Style::new().fg(theme::current().text_dim);
    let header = Row::new(cols.iter().map(|c| {
        let line = Line::from(c.title).alignment(c.align);
        Cell::from(line).style(hdr_style)
    }));

    let right = |text: String, style: Style| -> Cell<'static> {
        Cell::from(Line::from(text).alignment(Alignment::Right)).style(style)
    };
    let dash = || {
        right(
            "\u{2013}".to_string(),
            Style::new().fg(theme::current().text_dim),
        )
    };

    let rows: Vec<Row> = view.visible[offset..end]
        .iter()
        .map(|&i| {
            let mv = &moves[i];
            let sim = lookup.get(mv).filter(|r| r.is_simulated());
            let cells: Vec<Cell> = cols
                .iter()
                .map(|c| match c.title {
                    "Pos" => {
                        Cell::from(mv.coords()).style(Style::new().fg(theme::current().text_dim))
                    }
                    "Word" => {
                        let word = display_word(mv);
                        let text = if mv.is_bingo {
                            format!("{} {}", word, theme::current().glyphs.bingo)
                        } else {
                            word
                        };
                        Cell::from(text).style(Style::new().bold())
                    }
                    "Score" => right(
                        mv.score.to_string(),
                        Style::new().fg(theme::current().score_fg),
                    ),
                    "Equity" => right(
                        format!("{:.0}", mv.equity),
                        Style::new().fg(theme::current().equity_fg),
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
                        Cell::from(text).style(Style::new().fg(theme::current().leave_color(value)))
                    }
                    "Spread" => match sim {
                        Some(r) => right(
                            with_se(r.sim_equity, r.spread_se, wide),
                            Style::new().fg(theme::current().sim_equity_fg),
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
                            right(text, Style::new().fg(theme::current().win_color(r.win_pct)))
                        }
                        None => dash(),
                    },
                    "\u{0394}" => match sim {
                        Some(r) if r.delta == 0.0 && r.delta_se == 0.0 && !r.is_endgame => right(
                            "best".to_string(),
                            Style::new().fg(theme::current().win_good),
                        ),
                        Some(r) if r.is_endgame && r.delta == 0.0 => right(
                            "best".to_string(),
                            Style::new().fg(theme::current().win_good),
                        ),
                        Some(r) => right(
                            with_se(r.delta, r.delta_se, wide),
                            Style::new().fg(theme::current().text_primary),
                        ),
                        None => dash(),
                    },
                    "n" => match sim {
                        Some(r) if r.is_endgame => dash(),
                        Some(r) => right(
                            group_thousands(r.iterations as u64),
                            Style::new().fg(theme::current().text_dim),
                        ),
                        None => dash(),
                    },
                    "Bingo" => match sim {
                        Some(r) if !r.is_endgame => {
                            let color = if r.ply2_bingo_pct >= 20.0 {
                                theme::current().win_good
                            } else if r.ply2_bingo_pct >= 10.0 {
                                theme::current().equity_fg
                            } else {
                                theme::current().text_dim
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
                            Some(SimStatus::Racing) => theme::current().sim_equity_fg,
                            Some(SimStatus::Exact) => theme::current().win_good,
                            _ => theme::current().text_dim,
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
    state.select(pos.and_then(|p| p.checked_sub(offset)));
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
    let t = theme::current();

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
                .fg(t.tab_fg)
                .bg(t.sort_tab_bg)
                .into()
        })
        .collect();

    let highlight_color: Color = if is_focused {
        t.sort_tab_hl_focused
    } else {
        t.sort_tab_hl
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_keeps_a_margin_and_stays_put() {
        // Moving down inside the window does not scroll.
        assert_eq!(scroll_to(0, 5, 20, 100), 0);
        // Near the bottom edge it scrolls by just enough.
        assert_eq!(scroll_to(0, 18, 20, 100), 1);
        assert_eq!(scroll_to(0, 30, 20, 100), 13);
        // Moving up keeps the margin above.
        assert_eq!(scroll_to(13, 14, 20, 100), 12);
        // Never past the end, never negative.
        assert_eq!(scroll_to(90, 99, 20, 100), 80);
        assert_eq!(scroll_to(5, 0, 20, 100), 0);
        assert_eq!(scroll_to(0, 3, 20, 5), 0);
    }
}
