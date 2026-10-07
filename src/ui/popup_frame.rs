//! Shared popup frame rendering used by all dialog widgets.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};

/// Render a popup frame: clears background, draws bordered block, returns inner area.
pub fn render_popup_frame(
    area: Rect,
    buf: &mut Buffer,
    title: &str,
    border_color: ratatui::style::Color,
) -> Rect {
    Clear.render(area, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title))
        .title_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// X coordinate that centers `width` columns inside `inner`. Widgets that
/// register click targets draw at this position so the targets match what
/// is on screen.
pub fn centered_x(inner: Rect, width: u16) -> u16 {
    inner.x + inner.width.saturating_sub(width) / 2
}

/// Calculate a centered popup rectangle.
pub fn centered_rect(terminal: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(terminal.width.saturating_sub(4));
    let h = height.min(terminal.height.saturating_sub(2));
    Rect::new(
        (terminal.width.saturating_sub(w)) / 2,
        (terminal.height.saturating_sub(h)) / 2,
        w,
        h,
    )
}
