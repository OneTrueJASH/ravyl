//! Help popup: keyboard and mouse reference. Two columns on wide
//! terminals; one scrollable column (↑/↓, PgUp/PgDn) on narrow ones.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

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
            ("^C", "Quit (asks first; names an untitled game)"),
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
pub struct HelpPopup {
    scroll: u16,
}

impl HelpPopup {
    pub fn new(scroll: u16) -> Self {
        Self { scroll }
    }

    /// Width at which the help switches to two columns.
    const TWO_COLUMN_WIDTH: u16 = 104;

    pub fn area(terminal_area: Rect) -> Rect {
        if terminal_area.width >= Self::TWO_COLUMN_WIDTH {
            let rows = Self::column_lines(&SECTIONS[..2])
                .len()
                .max(Self::column_lines(&SECTIONS[2..]).len()) as u16;
            centered_rect(terminal_area, 100, rows + Self::footer().len() as u16 + 2)
        } else {
            let rows = Self::column_lines(SECTIONS).len() as u16 + Self::footer().len() as u16;
            centered_rect(terminal_area, 52, rows + 2)
        }
    }

    /// Total lines of the one-column layout (for clamping the scroll).
    pub fn line_count() -> u16 {
        (Self::column_lines(SECTIONS).len() + Self::footer().len()) as u16
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

impl Widget for HelpPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, "Help", theme::current().popup_border);
        crate::ui::hit::register_hit(area, crate::ui::hit::HitTarget::DialogArea);
        if inner.width >= 96 {
            let footer = Self::footer();
            let [cols, foot] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(footer.len() as u16)])
                    .areas(inner);
            let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
                .spacing(2)
                .areas(cols);
            Paragraph::new(Self::column_lines(&SECTIONS[..2])).render(left, buf);
            Paragraph::new(Self::column_lines(&SECTIONS[2..])).render(right, buf);
            Paragraph::new(footer).render(foot, buf);
        } else {
            let mut lines = Self::column_lines(SECTIONS);
            lines.extend(Self::footer());
            let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
            Paragraph::new(lines)
                .scroll((self.scroll.min(max_scroll), 0))
                .render(inner, buf);
        }
    }
}
