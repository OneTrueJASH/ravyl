//! Unified status bar: title + message + context-sensitive hints.
//!
//! Line 1 (top/title_bar): App title + RefEngine status.
//! Line 2 (bottom/status_bar): Context-sensitive keyboard hints.

use crate::app::{Focus, MctsProgress};
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

/// Render the title bar (top line).
pub fn render_title(frame: &mut ratatui::Frame, area: Rect) {
    let title = Line::from(vec![" Crossplay Solver ".bold().fg(theme::TITLE_FG)]);
    frame.render_widget(Paragraph::new(title), area);
}

/// Render the message bar (below board).
pub fn render_message(
    frame: &mut ratatui::Frame,
    area: Rect,
    message: Option<&str>,
    mcts_progress: Option<&MctsProgress>,
) {
    if let Some(progress) = mcts_progress {
        let spinner = progress.spinner_char();
        let text = format!(
            "{} Analyzing {} moves...",
            spinner, progress.total_candidates
        );
        let paragraph = Paragraph::new(text)
            .style(Style::new().fg(theme::MSG_PROGRESS))
            .centered();
        frame.render_widget(paragraph, area);
        return;
    }

    if let Some(msg) = message {
        let style = if msg.contains("Error")
            || msg.contains("not")
            || msg.contains("No ")
            || msg.contains("full")
        {
            Style::new().fg(theme::MSG_ERROR)
        } else {
            Style::new().fg(theme::MSG_OK)
        };

        let paragraph = Paragraph::new(msg).style(style).centered();
        frame.render_widget(paragraph, area);
    }
}

/// Render the bottom hint bar (context-sensitive).
pub fn render_hints(
    frame: &mut ratatui::Frame,
    area: Rect,
    focus: Focus,
    has_suggestions: bool,
    is_simulating: bool,
) {
    let mut spans = vec![
        Span::styled(" F1", Style::default().fg(theme::HINT_KEY)),
        Span::styled(" Help ", Style::default().fg(theme::HINT_DESC)),
        Span::styled("F2", Style::default().fg(theme::HINT_KEY)),
        Span::styled(" Settings ", Style::default().fg(theme::HINT_DESC)),
    ];

    // Context hints based on focus
    match focus {
        Focus::Suggestions if has_suggestions && !is_simulating => {
            spans.extend([
                Span::styled("M", Style::default().fg(theme::HINT_KEY)),
                Span::styled(" Simulate ", Style::default().fg(theme::HINT_DESC)),
                Span::styled("s", Style::default().fg(theme::HINT_KEY)),
                Span::styled(" Sort ", Style::default().fg(theme::HINT_DESC)),
            ]);
        }
        Focus::Board => {
            spans.extend([
                Span::styled("^S", Style::default().fg(theme::HINT_KEY)),
                Span::styled(" Solve ", Style::default().fg(theme::HINT_DESC)),
                Span::styled("Space", Style::default().fg(theme::HINT_KEY)),
                Span::styled(" Dir ", Style::default().fg(theme::HINT_DESC)),
            ]);
        }
        Focus::Rack => {
            spans.extend([
                Span::styled("Enter", Style::default().fg(theme::HINT_KEY)),
                Span::styled(" Solve ", Style::default().fg(theme::HINT_DESC)),
                Span::styled("Del", Style::default().fg(theme::HINT_KEY)),
                Span::styled(" Clear ", Style::default().fg(theme::HINT_DESC)),
            ]);
        }
        _ => {}
    }

    spans.extend([
        Span::styled("Tab", Style::default().fg(theme::HINT_KEY)),
        Span::styled(" Focus ", Style::default().fg(theme::HINT_DESC)),
    ]);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
