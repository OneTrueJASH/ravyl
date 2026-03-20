//! Escape menu popup with mouse-clickable options.

use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// Menu options for the escape menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeMenuOption {
    Quit,
    Reset,
    Cancel,
}

impl EscapeMenuOption {
    pub fn label(&self) -> &'static str {
        match self {
            EscapeMenuOption::Quit => "Quit Game",
            EscapeMenuOption::Reset => "Reset Board",
            EscapeMenuOption::Cancel => "Cancel",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            EscapeMenuOption::Quit => EscapeMenuOption::Reset,
            EscapeMenuOption::Reset => EscapeMenuOption::Cancel,
            EscapeMenuOption::Cancel => EscapeMenuOption::Quit,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            EscapeMenuOption::Quit => EscapeMenuOption::Cancel,
            EscapeMenuOption::Reset => EscapeMenuOption::Quit,
            EscapeMenuOption::Cancel => EscapeMenuOption::Reset,
        }
    }
}

/// Rendered popup widget for the escape/main menu.
pub struct EscapeMenuPopup {
    selected: EscapeMenuOption,
}

impl EscapeMenuPopup {
    pub fn new(selected: EscapeMenuOption) -> Self {
        Self { selected }
    }

    pub fn area(terminal_area: Rect) -> Rect {
        centered_rect(terminal_area, 30, 9)
    }

    fn render_option(&self, option: EscapeMenuOption) -> Line<'static> {
        let is_selected = self.selected == option;
        let style = if is_selected {
            Style::default()
                .fg(theme::BTN_SELECTED_FG)
                .bg(theme::POPUP_BORDER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_PRIMARY)
        };

        let prefix = if is_selected { " > " } else { "   " };
        let suffix = if is_selected { " < " } else { "   " };

        Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(option.label(), style),
            Span::styled(suffix, style),
        ])
    }
}

impl Widget for EscapeMenuPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, "Menu", theme::POPUP_BORDER);

        let content = vec![
            Line::from(""),
            self.render_option(EscapeMenuOption::Quit),
            self.render_option(EscapeMenuOption::Reset),
            self.render_option(EscapeMenuOption::Cancel),
            Line::from(""),
            Line::from(Span::styled(
                "↑/↓ select, Enter confirm",
                Style::default().fg(theme::TEXT_DIM),
            )),
        ];

        let paragraph = Paragraph::new(content).centered();
        paragraph.render(inner, buf);
    }
}
