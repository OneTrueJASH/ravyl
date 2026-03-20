//! Help popup showing keyboard shortcuts and about info.

use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// Popup widget displaying keyboard shortcuts and about information.
pub struct HelpPopup;

impl HelpPopup {
    pub fn new() -> Self {
        Self
    }

    pub fn area(terminal_area: Rect) -> Rect {
        centered_rect(terminal_area, 50, 34)
    }

    fn build_content() -> Vec<Line<'static>> {
        let header_style = Style::default()
            .fg(theme::POPUP_HEADER)
            .add_modifier(Modifier::BOLD);
        let key_style = Style::default().fg(theme::POPUP_KEY);
        let desc_style = Style::default().fg(theme::POPUP_DESC);

        let lines = vec![
            Line::from(Span::styled("KEYBOARD SHORTCUTS", header_style)),
            Line::from(vec![
                Span::styled("Tab       ", key_style),
                Span::styled("Cycle focus (Board→Rack→Sugg)", desc_style),
            ]),
            Line::from(vec![
                Span::styled("A-Z       ", key_style),
                Span::styled("Type letters", desc_style),
            ]),
            Line::from(vec![
                Span::styled("?         ", key_style),
                Span::styled("Place/toggle blank tile", desc_style),
            ]),
            Line::from(vec![
                Span::styled("Space     ", key_style),
                Span::styled("Toggle typing direction (→/↓)", desc_style),
            ]),
            Line::from(vec![
                Span::styled("Backspace ", key_style),
                Span::styled("Delete tile / smart backspace", desc_style),
            ]),
            Line::from(vec![
                Span::styled("Enter     ", key_style),
                Span::styled("Apply selected move", desc_style),
            ]),
            Line::from(vec![
                Span::styled("^S        ", key_style),
                Span::styled("Find moves (solve)", desc_style),
            ]),
            Line::from(vec![
                Span::styled("^Z        ", key_style),
                Span::styled("Undo last move", desc_style),
            ]),
            Line::from(vec![
                Span::styled("^R        ", key_style),
                Span::styled("Reset board", desc_style),
            ]),
            Line::from(vec![
                Span::styled("`         ", key_style),
                Span::styled("Toggle Board/Logs tab", desc_style),
            ]),
            Line::from(vec![
                Span::styled("F2        ", key_style),
                Span::styled("Settings", desc_style),
            ]),
            Line::from(vec![
                Span::styled("^C        ", key_style),
                Span::styled("Quit", desc_style),
            ]),
            Line::from(""),
            Line::from(Span::styled("SUGGESTIONS PANEL", header_style)),
            Line::from(vec![
                Span::styled("↑/↓       ", key_style),
                Span::styled("Navigate moves", desc_style),
            ]),
            Line::from(vec![
                Span::styled("s         ", key_style),
                Span::styled("Cycle sort mode", desc_style),
            ]),
            Line::from(vec![
                Span::styled("M         ", key_style),
                Span::styled("Run RefEngine simulation", desc_style),
            ]),
            Line::from(""),
            Line::from(Span::styled("MOUSE", header_style)),
            Line::from(vec![
                Span::styled("Click     ", key_style),
                Span::styled("Focus pane / select cell / row", desc_style),
            ]),
            Line::from(vec![
                Span::styled("Scroll    ", key_style),
                Span::styled("Navigate suggestions / logs", desc_style),
            ]),
            Line::from(""),
            Line::from(Span::styled("ABOUT", header_style)),
            Line::from(Span::styled(
                "RefEngine: Professional Scrabble engine by",
                desc_style,
            )),
            Line::from(Span::styled(
                "        RefEngine",
                desc_style,
            )),
            Line::from(""),
            Line::from(Span::styled(
                "github.com/OneTrueJASH/crossplay-solver",
                Style::default().fg(theme::TITLE_FG),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press F1 or Esc to close",
                Style::default()
                    .fg(theme::TEXT_DIM)
                    .add_modifier(Modifier::ITALIC),
            )),
        ];

        lines
    }
}

impl Default for HelpPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for HelpPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, "Help", theme::POPUP_BORDER);
        let content = Self::build_content();
        let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);
    }
}
