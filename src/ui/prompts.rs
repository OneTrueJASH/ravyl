//! Small input dialogs: the score prompt and the coordinate quick-entry
//! prompt (`:` then `8D QUIXOTIC`).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

use crate::app::{QuickPlay, ScoreTarget};
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;

/// The score prompt.
pub fn render_score_input(
    frame: &mut ratatui::Frame,
    size: Rect,
    target: ScoreTarget,
    buffer: &str,
) {
    let t = theme::current();
    let title = match target {
        ScoreTarget::Player => "Set Your Score",
        ScoreTarget::Opponent => "Set Opponent Score",
    };
    let popup_area = centered_rect(size, 36, 6);
    register_hit(popup_area, HitTarget::DialogArea);
    let inner = render_popup_frame(popup_area, frame.buffer_mut(), title, t.popup_border);
    let display = if buffer.is_empty() { "0" } else { buffer };
    let content = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Score: ", Style::default().fg(t.text_primary)),
            Span::styled(
                display.to_string(),
                Style::default()
                    .fg(t.score_fg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Type digits, Enter to set, Esc to cancel",
            Style::default().fg(t.text_dim),
        )),
    ];
    Paragraph::new(content).render(inner, frame.buffer_mut());
}

/// The coordinate prompt with a live preview of the play it describes.
pub fn render_quick_entry(
    frame: &mut ratatui::Frame,
    size: Rect,
    buffer: &str,
    preview: Result<QuickPlay, String>,
) {
    let t = theme::current();
    let popup_area = centered_rect(size, 56, 8);
    register_hit(popup_area, HitTarget::DialogArea);
    let inner = render_popup_frame(
        popup_area,
        frame.buffer_mut(),
        "Enter a play",
        t.popup_border,
    );
    let preview_line = match (buffer.trim().is_empty(), preview) {
        (true, _) => Line::from(Span::styled(
            "e.g. 8D QUIXOTIC across, H4 ZAX down; lowercase = blank",
            Style::default().fg(t.text_dim),
        )),
        (false, Ok(play)) => match play.problem {
            None => Line::from(Span::styled(
                format!(
                    "{} \u{00b7} {} pts \u{00b7} Enter places it",
                    play.word, play.score
                ),
                Style::default().fg(t.msg_ok).add_modifier(Modifier::BOLD),
            )),
            Some(problem) => Line::from(Span::styled(
                format!(
                    "{} \u{00b7} {} pts \u{00b7} {}",
                    play.word, play.score, problem
                ),
                Style::default().fg(t.msg_error),
            )),
        },
        (false, Err(e)) => Line::from(Span::styled(e, Style::default().fg(t.msg_error))),
    };
    let content = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  > ", Style::default().fg(t.popup_key)),
            Span::styled(
                format!("{}_", buffer),
                Style::default()
                    .fg(t.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        preview_line,
        Line::from(""),
        Line::from(Span::styled(
            "Enter to place, Esc to cancel",
            Style::default().fg(t.text_dim),
        )),
    ];
    Paragraph::new(content)
        .wrap(Wrap { trim: false })
        .centered()
        .render(inner, frame.buffer_mut());
}
