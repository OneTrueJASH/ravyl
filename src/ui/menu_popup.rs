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
const OPTIONS: [EscapeMenuOption; 6] = [
    EscapeMenuOption::OpenGame,
    EscapeMenuOption::SaveAs,
    EscapeMenuOption::NewGame,
    EscapeMenuOption::Reset,
    EscapeMenuOption::Quit,
    EscapeMenuOption::Cancel,
];

/// Menu options for the escape menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeMenuOption {
    OpenGame,
    SaveAs,
    NewGame,
    Reset,
    Quit,
    Cancel,
}

impl EscapeMenuOption {
    pub fn label(&self) -> &'static str {
        match self {
            EscapeMenuOption::OpenGame => "Open Game\u{2026}",
            EscapeMenuOption::SaveAs => "Save Game As\u{2026}",
            EscapeMenuOption::NewGame => "New Game",
            EscapeMenuOption::Reset => "Reset Board",
            EscapeMenuOption::Quit => "Quit Game",
            EscapeMenuOption::Cancel => "Cancel",
        }
    }

    fn index(&self) -> usize {
        OPTIONS.iter().position(|o| o == self).unwrap_or(0)
    }

    pub fn next(&self) -> Self {
        OPTIONS[(self.index() + 1) % OPTIONS.len()]
    }

    pub fn prev(&self) -> Self {
        OPTIONS[(self.index() + OPTIONS.len() - 1) % OPTIONS.len()]
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
        // Border, a blank row, the options, a blank row, the hint.
        centered_rect(terminal_area, 30, OPTIONS.len() as u16 + 5)
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
                .fg(theme::current().btn_selected_fg)
                .bg(theme::current().popup_border)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::current().text_primary)
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
        let inner = render_popup_frame(area, buf, "Menu", theme::current().popup_border);
        register_hit(area, HitTarget::DialogArea);

        let mut content = vec![Line::from(""); OPTIONS.len() + 2];
        content.push(Line::from(Span::styled(
            "↑/↓ select, Enter confirm",
            Style::default().fg(theme::current().text_dim),
        )));
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
