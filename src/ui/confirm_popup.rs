//! Confirmation dialog (Yes/No) with mouse-clickable buttons.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;

/// Yes/No confirmation dialog widget.
pub struct ConfirmPopup<'a> {
    title: &'a str,
    message: &'a str,
    selected: bool, // true = Yes, false = No
}

impl<'a> ConfirmPopup<'a> {
    pub fn new(title: &'a str, message: &'a str, selected: bool) -> Self {
        Self {
            title,
            message,
            selected,
        }
    }

    pub fn area(terminal_area: Rect) -> Rect {
        centered_rect(terminal_area, 40, 7)
    }
}

impl Widget for ConfirmPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, self.title, theme::POPUP_WARN_BORDER);

        // Register hit targets — not wired to specific actions yet, but the
        // main loop can use FocusPane or StatusBar as needed.
        let _ = inner; // suppress unused if needed

        let message_line = Line::from(Span::styled(
            self.message,
            Style::default().fg(theme::TEXT_PRIMARY),
        ));

        let yes_style = if self.selected {
            Style::default()
                .fg(theme::BTN_SELECTED_FG)
                .bg(theme::BTN_YES_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::BTN_YES_FG)
        };

        let no_style = if !self.selected {
            Style::default()
                .fg(theme::BTN_SELECTED_FG)
                .bg(theme::BTN_NO_FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::BTN_NO_FG)
        };

        let buttons_line = Line::from(vec![
            Span::raw("        "),
            Span::styled(" Yes ", yes_style),
            Span::raw("     "),
            Span::styled(" No ", no_style),
        ]);

        let hint_line = Line::from(Span::styled(
            "←/→ to select, Enter to confirm",
            Style::default().fg(theme::TEXT_DIM),
        ));

        let content = vec![
            Line::from(""),
            message_line,
            Line::from(""),
            buttons_line,
            hint_line,
        ];

        // Register clickable button regions for mouse support.
        // "Yes" is at ~col 8..12 within the inner area, row 3.
        let btn_y = inner.y + 3;
        if btn_y < inner.y + inner.height {
            register_hit(
                Rect::new(inner.x + 8, btn_y, 5, 1),
                HitTarget::StatusBar, // Repurposed as "confirm yes"
            );
            register_hit(
                Rect::new(inner.x + 18, btn_y, 4, 1),
                HitTarget::StatusBar, // Repurposed as "confirm no"
            );
        }

        let paragraph = Paragraph::new(content).centered();
        paragraph.render(inner, buf);
    }
}
