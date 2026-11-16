//! Confirmation dialog (Yes/No) with mouse-clickable buttons.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::popup_frame::{centered_rect, centered_x, render_popup_frame};
use crate::ui::theme;

const YES_LABEL: &str = " Yes ";
const NO_LABEL: &str = " No ";
/// Columns between the two buttons.
const BUTTON_GAP: u16 = 5;

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

    /// Wide enough for `message` (a game's name can make it long), at
    /// least 40 columns and at most the terminal's width.
    pub fn area(terminal_area: Rect, message: &str) -> Rect {
        let want = (message.chars().count() as u16 + 6).max(40);
        let width = want.min(terminal_area.width.saturating_sub(2).max(40));
        centered_rect(terminal_area, width, 7)
    }

    /// Screen rectangles of the Yes and No buttons for a dialog whose
    /// inner (bordered-off) area is `inner`.
    pub fn button_rects(inner: Rect) -> (Rect, Rect) {
        let yes_w = YES_LABEL.len() as u16;
        let no_w = NO_LABEL.len() as u16;
        let x0 = centered_x(inner, yes_w + BUTTON_GAP + no_w);
        let y = inner.y + 3;
        (
            Rect::new(x0, y, yes_w, 1),
            Rect::new(x0 + yes_w + BUTTON_GAP, y, no_w, 1),
        )
    }
}

impl Widget for ConfirmPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, self.title, theme::current().popup_warn_border);
        register_hit(area, HitTarget::DialogArea);

        let message_line = Line::from(Span::styled(
            self.message,
            Style::default().fg(theme::current().text_primary),
        ));

        let yes_style = if self.selected {
            Style::default()
                .fg(theme::current().btn_selected_fg)
                .bg(theme::current().btn_yes_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::current().btn_yes_fg)
        };

        let no_style = if !self.selected {
            Style::default()
                .fg(theme::current().btn_selected_fg)
                .bg(theme::current().btn_no_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::current().btn_no_fg)
        };

        let hint_line = Line::from(Span::styled(
            "←/→ to select, Enter to confirm",
            Style::default().fg(theme::current().text_dim),
        ));

        let content = vec![
            Line::from(""),
            message_line,
            Line::from(""),
            Line::from(""),
            hint_line,
        ];
        let paragraph = Paragraph::new(content).centered();
        paragraph.render(inner, buf);

        // The buttons are drawn at the same rectangles that are registered
        // for clicks.
        let (yes, no) = Self::button_rects(inner);
        if yes.y < inner.y + inner.height {
            buf.set_span(yes.x, yes.y, &Span::styled(YES_LABEL, yes_style), yes.width);
            buf.set_span(no.x, no.y, &Span::styled(NO_LABEL, no_style), no.width);
            register_hit(yes, HitTarget::DialogYes);
            register_hit(no, HitTarget::DialogNo);
        }
    }
}
