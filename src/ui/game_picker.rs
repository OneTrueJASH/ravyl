//! The game picker: saved games, most recently played first, with their
//! score, the tiles left in the bag and when they were last played. Rows
//! are clickable (a double-click opens one); games open in another window
//! are dimmed.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::{App, MessageKind};
use crate::games::{self, GameEntry};
use crate::ui::hit::{register_hit, HitTarget};
use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;

/// Widths of the score, bag and last-played columns.
const SCORE_W: usize = 9;
const BAG_W: usize = 7;
const WHEN_W: usize = 9;

/// Rows inside the border that are not games: the header, a blank row,
/// the message row and the key hints.
const CHROME_ROWS: u16 = 4;

/// `text` cut to `width` characters (with an ellipsis when cut) and padded
/// to exactly `width`.
fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        format!("{:<width$}", text)
    } else if width == 0 {
        String::new()
    } else {
        let mut cut: String = text.chars().take(width - 1).collect();
        cut.push('\u{2026}');
        cut
    }
}

/// The picker popup.
pub fn render_game_picker(frame: &mut ratatui::Frame, size: Rect, app: &App, selected: usize) {
    let t = theme::current();
    let rows = app.picker_games.len().max(1) as u16;
    let area = centered_rect(size, 78, rows + CHROME_ROWS + 2);
    register_hit(area, HitTarget::DialogArea);
    let inner = render_popup_frame(area, frame.buffer_mut(), "Saved games", t.popup_border);
    if inner.height < CHROME_ROWS || inner.width < 20 {
        return;
    }
    let buf = frame.buffer_mut();
    let width = inner.width as usize;
    // The name column takes what the fixed columns leave (2 for the
    // selection marker, 3 gaps).
    let name_w = width.saturating_sub(2 + SCORE_W + BAG_W + WHEN_W + 3);

    let header = format!(
        "  {} {:>SCORE_W$} {:>BAG_W$} {:>WHEN_W$}",
        fit("Game", name_w),
        "Score",
        "Bag",
        "Played"
    );
    buf.set_line(
        inner.x,
        inner.y,
        &Line::from(Span::styled(
            header,
            Style::default()
                .fg(t.popup_header)
                .add_modifier(Modifier::BOLD),
        )),
        inner.width,
    );

    let list_h = (inner.height - CHROME_ROWS + 1) as usize;
    let list_top = inner.y + 1;
    if app.picker_games.is_empty() {
        buf.set_line(
            inner.x,
            list_top,
            &Line::from(Span::styled(
                "  No saved games yet \u{2014} n starts one",
                Style::default().fg(t.text_dim),
            )),
            inner.width,
        );
    }
    let offset = selected.saturating_sub(list_h.saturating_sub(1));
    let now = games::now_secs();
    for (row, (i, g)) in app
        .picker_games
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_h)
        .enumerate()
    {
        let y = list_top + row as u16;
        let line = game_line(app, g, i == selected, name_w, now);
        buf.set_line(inner.x, y, &line, inner.width);
        register_hit(
            Rect::new(inner.x, y, inner.width, 1),
            HitTarget::PickerRow(i),
        );
    }

    // Errors (a locked game, a failed open) show inside the picker, where
    // the user is looking.
    let message_y = inner.y + inner.height - 2;
    if let (Some(msg), MessageKind::Error) = (app.message.as_deref(), app.message_kind) {
        buf.set_line(
            inner.x,
            message_y,
            &Line::from(Span::styled(
                fit(&format!("  {}", msg), width),
                Style::default()
                    .fg(t.msg_error)
                    .add_modifier(Modifier::BOLD),
            )),
            inner.width,
        );
    }
    let hint = "Enter open \u{00b7} n new \u{00b7} r rename \u{00b7} d delete \u{00b7} Esc close";
    let hint_line = Line::from(Span::styled(hint, Style::default().fg(t.text_dim))).centered();
    buf.set_line(
        inner.x + (inner.width.saturating_sub(hint_line.width() as u16)) / 2,
        inner.y + inner.height - 1,
        &hint_line,
        inner.width,
    );
}

/// One game's row.
fn game_line(app: &App, g: &GameEntry, selected: bool, name_w: usize, now: u64) -> Line<'static> {
    let t = theme::current();
    let note = if g.locked {
        " (open in another window)"
    } else if app.is_current_game(g) {
        " (this window)"
    } else {
        ""
    };
    let bag = g.bag.map_or("-".to_string(), |b| b.to_string());
    let text = format!(
        "{} {} {:>SCORE_W$} {:>BAG_W$} {:>WHEN_W$}",
        if selected { ">" } else { " " },
        fit(&format!("{}{}", g.name, note), name_w),
        g.score_text(),
        bag,
        games::relative_time(g.updated, now)
    );
    let style = if selected {
        theme::highlight_style()
    } else if g.locked {
        Style::default().fg(t.text_dim)
    } else {
        Style::default().fg(t.text_primary)
    };
    Line::from(Span::styled(text, style))
}
