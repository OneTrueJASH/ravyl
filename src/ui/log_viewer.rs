//! Log viewer and tab bar rendering.

use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    symbols,
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Tabs, Wrap},
};

use crate::app::BoardTab;
use crate::logging::{log_path_str, with_section_lines};
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::theme;

/// Background and highlight colors of a tab.
fn tab_colors(tab: BoardTab) -> (Color, Color) {
    let t = theme::current();
    match tab {
        BoardTab::Board => (t.tab_board_bg, t.tab_board_hl),
        BoardTab::Logs => (t.tab_logs_bg, t.tab_logs_hl),
    }
}

pub fn tab_border_color(tab: BoardTab) -> Color {
    tab_colors(tab).1
}

pub fn render_tabs(
    frame: &mut ratatui::Frame,
    area: Rect,
    selected_tab: BoardTab,
    _is_focused: bool,
) {
    let titles: Vec<Line> = vec![tab_title(BoardTab::Board), tab_title(BoardTab::Logs)];

    let selected_index = match selected_tab {
        BoardTab::Board => 0,
        BoardTab::Logs => 1,
    };

    let highlight_style = (Color::default(), tab_colors(selected_tab).1);

    let tabs = Tabs::new(titles)
        .select(selected_index)
        .highlight_style(highlight_style)
        .padding("", "")
        .divider(" ");

    // Register tab header hits at the titles' real widths (no padding,
    // one-column divider between them).
    let mut x = area.x;
    for tab in [BoardTab::Board, BoardTab::Logs] {
        let w = tab_title(tab).width() as u16;
        register_hit(Rect::new(x, area.y, w, 1), HitTarget::TabHeader(tab));
        x += w + 1;
    }

    frame.render_widget(tabs, area);
}

fn tab_title(tab: BoardTab) -> Line<'static> {
    let name = match tab {
        BoardTab::Board => "Board",
        BoardTab::Logs => "Logs",
    };
    format!("  {name}  ")
        .fg(theme::current().tab_fg)
        .bg(tab_colors(tab).0)
        .into()
}

pub fn render_log_viewer(
    frame: &mut ratatui::Frame,
    area: Rect,
    scroll_offset: usize,
    is_focused: bool,
) {
    let border_color = if is_focused {
        tab_border_color(BoardTab::Logs)
    } else {
        theme::current().border_normal
    };

    let log_path = log_path_str();
    let inner_height = area.height.saturating_sub(2) as usize;

    // Only the visible window is copied out of the in-memory log.
    let (visible, total_lines, effective_scroll) = with_section_lines(|lines| {
        let total_lines = lines.len();
        let max_scroll = total_lines.saturating_sub(inner_height);
        let effective_scroll = scroll_offset.min(max_scroll);
        let end_idx = total_lines.saturating_sub(effective_scroll);
        let start_idx = end_idx.saturating_sub(inner_height);
        (
            lines[start_idx..end_idx].to_vec(),
            total_lines,
            effective_scroll,
        )
    });

    let visible_lines: Vec<Line> = visible
        .into_iter()
        .map(|line| {
            if line.starts_with('[') {
                if let Some(bracket_end) = line.find(']') {
                    let timestamp = line[..=bracket_end].to_string();
                    let rest = line[bracket_end + 1..].to_string();

                    let content_color = if rest.contains("ERROR") || rest.contains("error") {
                        theme::current().msg_error
                    } else if rest.contains("===") {
                        theme::current().popup_header
                    } else if rest.contains("Parsed") || rest.contains("results") {
                        theme::current().msg_ok
                    } else {
                        theme::current().text_primary
                    };

                    Line::from(vec![
                        Span::styled(timestamp, Style::default().fg(theme::current().text_dim)),
                        Span::styled(rest, Style::default().fg(content_color)),
                    ])
                } else {
                    Line::from(Span::styled(
                        line,
                        Style::default().fg(theme::current().text_primary),
                    ))
                }
            } else {
                Line::from(Span::styled(
                    line,
                    Style::default().fg(theme::current().text_primary),
                ))
            }
        })
        .collect();

    let scroll_info = if effective_scroll > 0 {
        format!("\u{2191}{} below | ", effective_scroll)
    } else {
        String::new()
    };
    let footer = format!(" {}{} ({} lines) ", scroll_info, log_path, total_lines);

    let block = Block::bordered()
        .border_set(symbols::border::PROPORTIONAL_TALL)
        .padding(Padding::horizontal(1))
        .title_bottom(Line::from(footer).style(Style::default().fg(theme::current().text_dim)))
        .border_style(border_color);

    let paragraph = Paragraph::new(visible_lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}
