//! Escape menu popup with mouse-clickable options.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::popup_frame::{centered_rect, centered_x, render_popup_frame};
use crate::ui::theme;

/// Menu options in display order.
const OPTIONS: [EscapeMenuOption; 3] = [
    EscapeMenuOption::Quit,
    EscapeMenuOption::Reset,
    EscapeMenuOption::Cancel,
];

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

    /// The clickable row of each option, for a menu whose inner area is
    /// `inner` (the whole row inside the border, so the target is wide).
    pub fn option_rects(inner: Rect) -> Vec<(EscapeMenuOption, Rect)> {
        OPTIONS
            .iter()
            .enumerate()
            .map(|(i, &o)| {
                (
                    o,
                    Rect::new(inner.x, inner.y + 1 + i as u16, inner.width, 1),
                )
            })
            .filter(|(_, r)| r.y < inner.y + inner.height)
            .collect()
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
        register_hit(area, HitTarget::DialogArea);

        let content = vec![
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                "↑/↓ select, Enter confirm",
                Style::default().fg(theme::TEXT_DIM),
            )),
        ];
        let paragraph = Paragraph::new(content).centered();
        paragraph.render(inner, buf);

        // Options are drawn on the same rows that are registered for clicks.
        for (option, row) in Self::option_rects(inner) {
            let line = self.render_option(option);
            let x = centered_x(row, line.width() as u16);
            buf.set_line(x, row.y, &line, row.width);
            register_hit(row, HitTarget::MenuItem(option));
        }
    }
}
