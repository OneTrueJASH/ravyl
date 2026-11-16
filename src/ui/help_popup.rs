//! Help popup: keyboard and mouse reference, then what the table and the
//! stats pane show. Two columns on wide terminals, one on narrow ones;
//! either scrolls (↑/↓, PgUp/PgDn) when it does not fit.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;

type Section = (&'static str, &'static [(&'static str, &'static str)]);

const SECTIONS: &[Section] = &[
    (
        "EVERYWHERE",
        &[
            ("F1", "Toggle this help"),
            ("Tab / S-Tab", "Next / previous pane"),
            ("Esc", "Stop simulation, close, back, menu"),
            ("^S", "Find moves (solve) and simulate"),
            ("^Z / ^Y", "Undo / redo"),
            ("^R", "Reset board (undoable)"),
            ("^E", "Toggle who has the final turn"),
            (":", "Enter a play: 8D QUIXOTIC"),
            ("= / +", "Set opponent / your score"),
            ("`", "Board / log tab"),
            ("\\", "Toggle sidebar"),
            ("^O", "Saved games"),
            ("^C", "Quit (asks first)"),
        ],
    ),
    (
        "READING THE TABLE",
        &[
            ("Equity", "Score + value of the tiles kept"),
            ("Spread", "Mean final spread, \u{00b1} error"),
            ("Win%", "Chance to win, \u{00b1} error"),
            ("\u{0394}", "Spread behind the best play"),
            ("n", "Playouts run for this play"),
            ("Status", "racing \u{00b7} out @n (dropped) \u{00b7} exact"),
            ("Bingo", "Our next bingo % / their reply's"),
            ("\u{2605} / abc", "Bingo (+40) / letter from a blank"),
        ],
    ),
    (
        "BOARD",
        &[
            ("Arrows", "Move cursor"),
            ("a-z", "Place letter, skip filled squares"),
            ("Shift+A-Z", "Place a blank as that letter"),
            ("?", "Toggle blank on tile (or place one)"),
            ("Space", "Typing direction across/down"),
            ("Backspace", "Delete here or step back"),
            ("Delete", "Remove tile at cursor"),
            ("Enter", "Solve"),
        ],
    ),
    (
        "RACK",
        &[
            ("A-Z / ?", "Add letter or blank"),
            ("Backspace", "Remove last tile"),
            ("Delete", "Clear rack (undoable)"),
            ("Enter", "Solve"),
        ],
    ),
    (
        "SUGGESTIONS",
        &[
            ("Up/Down j/k", "Select"),
            ("PgUp/PgDn", "Page"),
            ("Home/End", "First / last"),
            ("1-9", "Select a visible row"),
            ("/", "Filter by word or square"),
            ("n / N", "Next / previous row"),
            ("Enter", "Apply move"),
            ("M (or m)", "Simulate every candidate"),
            ("s / S", "Cycle sort"),
            ("y / Y", "Copy move / position (CGP)"),
        ],
    ),
    (
        "GAMES",
        &[
            ("Esc menu", "Open, Save As, New Game"),
            ("Autosave", "Every change, one file per game"),
            ("Enter", "Picker: open the game"),
            ("n / r / d", "Picker: new / rename / delete"),
            ("Dimmed", "Open in another window"),
        ],
    ),
    (
        "STATS PANE",
        &[
            ("Bag ~77", "Tiles left to draw (estimate)"),
            ("unseen ?1 S4", "Blanks and S's still to come"),
            ("Power tiles", "Where Q Z J X are"),
            ("Opp kept", "Letters they likely kept"),
            ("Opp error", "Their average loss per move"),
            ("Final move", "Who plays last once bag is empty"),
        ],
    ),
    (
        "MOUSE",
        &[
            ("Click", "Focus pane, square or row"),
            ("Double-click", "Apply a suggestion, open a game"),
            ("Header", "Sort by that column"),
            ("Wheel", "Scroll the pane under it"),
            ("Paste", "Load a CGP position"),
        ],
    ),
];

/// Popup widget displaying keyboard shortcuts and about information.
pub struct HelpPopup<'a> {
    scroll: u16,
    /// Set to how far this layout can scroll, so input stops there.
    max_scroll: &'a std::cell::Cell<u16>,
}

impl<'a> HelpPopup<'a> {
    pub fn new(scroll: u16, max_scroll: &'a std::cell::Cell<u16>) -> Self {
        Self { scroll, max_scroll }
    }

    /// How far `total` lines scroll in `height` rows: when they overflow,
    /// one row goes to the scroll hint.
    fn max_scroll_for(total: u16, height: u16) -> u16 {
        if total > height {
            total.saturating_sub(height.saturating_sub(1))
        } else {
            0
        }
    }

    /// Width at which the help switches to two columns.
    const TWO_COLUMN_WIDTH: u16 = 104;

    /// Sections in the left column of the two-column layout.
    const LEFT: usize = 3;

    pub fn area(terminal_area: Rect) -> Rect {
        if terminal_area.width >= Self::TWO_COLUMN_WIDTH {
            let rows = Self::column_lines(&SECTIONS[..Self::LEFT])
                .len()
                .max(Self::column_lines(&SECTIONS[Self::LEFT..]).len())
                as u16;
            centered_rect(terminal_area, 100, rows + Self::footer().len() as u16 + 2)
        } else {
            let rows = Self::column_lines(SECTIONS).len() as u16 + Self::footer().len() as u16;
            centered_rect(terminal_area, 52, rows + 2)
        }
    }

    /// The last line of `area` (a popup's inner area) says how to scroll
    /// when there is more than fits; returns the area left for content.
    fn scroll_hint(area: Rect, buf: &mut Buffer, scroll: u16, max_scroll: u16) -> Rect {
        if max_scroll == 0 || area.height < 2 {
            return area;
        }
        let [content, hint] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let more = if scroll >= max_scroll {
            "\u{2191} scroll up \u{00b7} F1 or Esc closes"
        } else {
            "\u{2193} more below (\u{2191}\u{2193} PgUp PgDn) \u{00b7} F1 or Esc closes"
        };
        Paragraph::new(Line::from(Span::styled(
            more,
            Style::default()
                .fg(theme::current().hint_key)
                .add_modifier(Modifier::ITALIC),
        )))
        .render(hint, buf);
        content
    }

    fn column_lines(sections: &[Section]) -> Vec<Line<'static>> {
        let t = theme::current();
        let header = Style::default()
            .fg(t.popup_header)
            .add_modifier(Modifier::BOLD);
        let key = Style::default().fg(t.popup_key);
        let desc = Style::default().fg(t.popup_desc);
        let mut lines = Vec::new();
        for (i, (title, entries)) in sections.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(*title, header)));
            for (k, d) in entries.iter() {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<13}", k), key),
                    Span::styled(*d, desc),
                ]));
            }
        }
        lines
    }

    fn footer() -> Vec<Line<'static>> {
        let t = theme::current();
        let desc = Style::default().fg(t.popup_desc);
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "Opponent inference: RefEngine (RefEngine, GPL-3.0)",
                desc,
            )),
            Line::from(Span::styled(
                "cpsolver2 by OneTrueJASH (github.com/OneTrueJASH/cpsolver2)",
                desc,
            )),
            Line::from(Span::styled(
                format!("Theme: {} (display.theme in settings.toml)", t.name),
                desc,
            )),
            Line::from(Span::styled(
                "F1 or Esc closes \u{00b7} \u{2191}\u{2193} scroll",
                Style::default()
                    .fg(t.text_dim)
                    .add_modifier(Modifier::ITALIC),
            )),
        ]
    }
}

impl Widget for HelpPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, "Help", theme::current().popup_border);
        crate::ui::hit::register_hit(area, crate::ui::hit::HitTarget::DialogArea);
        if inner.width >= 96 {
            // Both columns, then the footer, scroll together as one page.
            let mut left = Self::column_lines(&SECTIONS[..Self::LEFT]);
            let mut right = Self::column_lines(&SECTIONS[Self::LEFT..]);
            let rows = left.len().max(right.len());
            left.resize(rows, Line::from(""));
            right.resize(rows, Line::from(""));
            let mut left_page = left;
            left_page.extend(Self::footer());
            let max_scroll = Self::max_scroll_for(left_page.len() as u16, inner.height);
            self.max_scroll.set(max_scroll);
            let scroll = self.scroll.min(max_scroll);
            let content = Self::scroll_hint(inner, buf, scroll, max_scroll);
            let [left_area, right_area] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
                    .spacing(2)
                    .areas(content);
            Paragraph::new(left_page)
                .scroll((scroll, 0))
                .render(left_area, buf);
            Paragraph::new(right)
                .scroll((scroll, 0))
                .render(right_area, buf);
        } else {
            let mut lines = Self::column_lines(SECTIONS);
            lines.extend(Self::footer());
            let max_scroll = Self::max_scroll_for(lines.len() as u16, inner.height);
            self.max_scroll.set(max_scroll);
            let scroll = self.scroll.min(max_scroll);
            let content = Self::scroll_hint(inner, buf, scroll, max_scroll);
            Paragraph::new(lines)
                .scroll((scroll, 0))
                .render(content, buf);
        }
    }
}
