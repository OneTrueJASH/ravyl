//! Unified status bar: title + message + context-sensitive hints.
//!
//! Line 1 (top/title_bar): App title + RefEngine status.
//! Line 2 (bottom/status_bar): Context-sensitive keyboard hints.

use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{group_thousands, App, BoardTab, Focus, MctsProgress, MessageKind};
use crate::ui::theme;

/// Render the title bar (top line).
pub fn render_title(frame: &mut ratatui::Frame, area: Rect) {
    let title = Line::from(vec![" Crossplay Solver "
        .bold()
        .fg(theme::current().title_fg)]);
    frame.render_widget(Paragraph::new(title), area);
}

/// Shown instead of the UI when the terminal is below the minimum size.
pub fn render_too_small(frame: &mut ratatui::Frame, area: Rect, need: (u16, u16)) {
    let text = vec![
        Line::from("terminal too small"),
        Line::from(format!(
            "need {}\u{00d7}{}, have {}\u{00d7}{}",
            need.0, need.1, area.width, area.height
        )),
    ];
    let y = area.y + area.height.saturating_sub(2) / 2;
    let rect = Rect::new(area.x, y, area.width, area.height.min(2));
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::new().fg(theme::current().msg_error))
            .centered(),
        rect,
    );
}

/// Render the message bar (below board).
pub fn render_message(
    frame: &mut ratatui::Frame,
    area: Rect,
    message: Option<&str>,
    kind: MessageKind,
    mcts_progress: Option<&MctsProgress>,
) {
    let t = theme::current();
    if let Some(progress) = mcts_progress {
        // The spinner, the phase message from when the evaluation started,
        // and live progress once a simulation reports it.
        let spinner = progress.spinner_char();
        let mut text = format!("{} {}", spinner, message.unwrap_or("Analyzing..."));
        if let Some(sim) = progress.sim {
            text.push_str(&format!(
                " \u{00b7} {}/{} \u{00b7} {} playouts \u{00b7} {:.1}s",
                group_thousands(sim.simulated as u64),
                group_thousands(sim.total as u64),
                group_thousands(sim.playouts),
                sim.elapsed.as_secs_f32()
            ));
        }
        let paragraph = Paragraph::new(text)
            .style(Style::new().fg(t.msg_progress))
            .centered();
        frame.render_widget(paragraph, area);
        return;
    }

    if let Some(msg) = message {
        let color = match kind {
            MessageKind::Info => t.msg_info,
            MessageKind::Success => t.msg_ok,
            MessageKind::Error => t.msg_error,
            MessageKind::Progress => t.msg_progress,
        };
        let style = if kind == MessageKind::Error {
            Style::new().fg(color).bold()
        } else {
            Style::new().fg(color)
        };
        let paragraph = Paragraph::new(msg).style(style).centered();
        frame.render_widget(paragraph, area);
    }
}

/// Render the bottom hint bar: the keys that matter for the current mode
/// (simulation running, filter being typed, or the focused pane), then the
/// global ones, as many as fit.
pub fn render_hints(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let t = theme::current();
    let mut keys: Vec<(&str, &str)> = Vec::new();
    if app.is_mcts_running() {
        keys.extend([
            ("Esc", "Stop"),
            ("\u{2191}\u{2193}", "Browse"),
            ("s", "Sort"),
        ]);
    } else if app.filter_editing {
        keys.extend([("Enter", "Keep filter"), ("Esc", "Clear filter")]);
    } else {
        match app.focus {
            Focus::Board if app.board_tab == BoardTab::Logs => {
                keys.extend([("\u{2191}\u{2193}", "Scroll"), ("`", "Board")]);
            }
            Focus::Board => keys.extend([
                ("Enter", "Solve"),
                ("Space", "Dir"),
                ("Shift+A", "Blank"),
                (":", "Enter play"),
                ("Esc", "Menu"),
            ]),
            Focus::Rack => keys.extend([("Enter", "Solve"), ("Del", "Clear"), ("Esc", "Board")]),
            Focus::Suggestions if !app.suggestions.is_empty() => keys.extend([
                ("Enter", "Apply"),
                ("M", "Simulate"),
                ("s", "Sort"),
                ("/", "Filter"),
                ("y", "Copy"),
                ("Esc", "Board"),
            ]),
            Focus::Suggestions => keys.push(("^S", "Solve")),
        }
        if app.state.is_endgame() {
            keys.push(("^E", "Final turn"));
        }
    }
    keys.extend([
        ("^Z", "Undo"),
        ("Tab", "Focus"),
        ("`", "Logs"),
        ("\\", "Sidebar"),
        ("F1", "Help"),
    ]);

    let mut spans = vec![Span::raw(" ")];
    let mut width = 1usize;
    for (key, desc) in keys {
        let w = key.chars().count() + desc.chars().count() + 2;
        if width + w > area.width as usize {
            break;
        }
        width += w;
        spans.push(Span::styled(
            key.to_string(),
            Style::default().fg(t.hint_key),
        ));
        spans.push(Span::styled(
            format!(" {} ", desc),
            Style::default().fg(t.hint_desc),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
