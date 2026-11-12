//! Centralized theme: every color, style and decorative glyph the TUI uses.
//!
//! [`Theme`] has four variants: `dark` (truecolor, the original look),
//! `light`, `ansi16` (only the 16 basic colors, for terminals without
//! truecolor) and `no_color` (honors `NO_COLOR`: no colors at all; the
//! cursor and selection rely on REVERSED and BOLD). [`init`] picks one at
//! startup ([`ThemeChoice::Auto`] looks at `NO_COLOR` and `COLORTERM`);
//! widgets read it with [`current`].

use std::sync::OnceLock;

use ratatui::style::palette::tailwind;
use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

/// Which theme to use (the `display.theme` setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// `NO_COLOR` set: no colors; truecolor terminal: dark; else ansi16.
    #[default]
    Auto,
    Dark,
    Light,
    Ansi16,
    NoColor,
}

/// Decorative characters, with ASCII stand-ins for terminals that are not
/// in a UTF-8 locale.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    /// Marks a bingo in the suggestions list.
    pub bingo: &'static str,
    /// The centre square.
    pub center: &'static str,
    pub arrow_right: char,
    pub arrow_down: char,
}

const UNICODE_GLYPHS: Glyphs = Glyphs {
    bingo: "\u{2605}",
    center: "\u{2666}",
    arrow_right: '\u{2192}',
    arrow_down: '\u{2193}',
};

const ASCII_GLYPHS: Glyphs = Glyphs {
    bingo: "*",
    center: "+",
    arrow_right: '>',
    arrow_down: 'v',
};

/// Every color the widgets use.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    /// True for the no-color theme: styles add REVERSED where color would
    /// otherwise carry the meaning.
    pub monochrome: bool,
    pub glyphs: Glyphs,

    pub text_primary: Color,
    pub text_dim: Color,
    pub border_normal: Color,

    pub tile_bg: Color,
    pub tile_fg: Color,
    pub tile_blank_fg: Color,
    pub tile_empty_bg: Color,
    pub tile_preview_bg: Color,
    pub tile_preview_fg: Color,
    pub tile_preview_blank_fg: Color,

    pub cursor_bg: Color,
    pub cursor_fg: Color,
    /// Cursor square while the board is not focused.
    pub cursor_dim_bg: Color,
    pub premium_3w: Color,
    pub premium_2w: Color,
    pub premium_3l: Color,
    pub premium_2l: Color,
    pub premium_center: Color,
    pub premium_normal: Color,
    pub axis_fg: Color,

    pub score_fg: Color,
    pub equity_fg: Color,
    pub sim_equity_fg: Color,

    pub win_good: Color,
    pub win_neutral: Color,
    pub win_bad: Color,

    pub leave_great: Color,
    pub leave_good: Color,
    pub leave_meh: Color,
    pub leave_bad: Color,

    pub phase_early: Color,
    pub phase_mid: Color,
    pub phase_late: Color,
    pub phase_endgame: Color,

    pub msg_ok: Color,
    pub msg_info: Color,
    pub msg_error: Color,
    pub msg_progress: Color,
    pub title_fg: Color,

    pub popup_border: Color,
    pub popup_warn_border: Color,
    pub popup_key: Color,
    pub popup_header: Color,
    pub popup_desc: Color,

    pub highlight_bg: Color,
    pub highlight_fg: Color,

    pub hint_key: Color,
    pub hint_desc: Color,

    pub pane_rack: Color,
    pub pane_suggestions: Color,
    pub tab_board_active: Color,

    pub btn_yes_fg: Color,
    pub btn_no_fg: Color,
    pub btn_selected_fg: Color,

    /// Tab labels (board/logs and sort tabs).
    pub tab_fg: Color,
    pub tab_board_bg: Color,
    pub tab_board_hl: Color,
    pub tab_logs_bg: Color,
    pub tab_logs_hl: Color,
    pub sort_tab_bg: Color,
    pub sort_tab_hl: Color,
    pub sort_tab_hl_focused: Color,
}

impl Theme {
    /// The original look, for truecolor terminals.
    pub fn dark() -> Self {
        Theme {
            name: "dark",
            monochrome: false,
            glyphs: UNICODE_GLYPHS,
            text_primary: Color::White,
            text_dim: Color::DarkGray,
            border_normal: Color::DarkGray,
            tile_bg: Color::Rgb(31, 95, 191), // Crossplay blue
            tile_fg: Color::White,
            tile_blank_fg: Color::Rgb(180, 180, 180),
            tile_empty_bg: Color::Rgb(60, 60, 60),
            tile_preview_bg: Color::LightYellow,
            tile_preview_fg: Color::Black,
            tile_preview_blank_fg: Color::DarkGray,
            cursor_bg: Color::Red,
            cursor_fg: Color::White,
            cursor_dim_bg: Color::Rgb(90, 50, 50),
            premium_3w: Color::Rgb(140, 80, 180),
            premium_2w: Color::Rgb(100, 140, 200),
            premium_3l: Color::Rgb(80, 160, 80),
            premium_2l: Color::Rgb(180, 150, 80),
            premium_center: Color::Yellow,
            premium_normal: Color::DarkGray,
            axis_fg: Color::DarkGray,
            score_fg: Color::Yellow,
            equity_fg: Color::Cyan,
            sim_equity_fg: Color::Magenta,
            win_good: Color::Green,
            win_neutral: Color::Yellow,
            win_bad: Color::Red,
            leave_great: Color::Green,
            leave_good: Color::Cyan,
            leave_meh: Color::Yellow,
            leave_bad: Color::Red,
            phase_early: Color::Green,
            phase_mid: Color::Yellow,
            phase_late: Color::Red,
            phase_endgame: Color::Magenta,
            msg_ok: Color::Green,
            msg_info: Color::Gray,
            msg_error: Color::Red,
            msg_progress: Color::Magenta,
            title_fg: Color::LightBlue,
            popup_border: Color::Cyan,
            popup_warn_border: Color::Yellow,
            popup_key: Color::Yellow,
            popup_header: Color::Cyan,
            popup_desc: Color::Gray,
            highlight_bg: Color::LightYellow,
            highlight_fg: Color::Black,
            hint_key: Color::Cyan,
            hint_desc: Color::DarkGray,
            pane_rack: Color::Rgb(180, 83, 9),          // amber-700
            pane_suggestions: Color::Rgb(109, 40, 217), // violet-700
            tab_board_active: Color::Rgb(4, 120, 87),   // emerald-700
            btn_yes_fg: Color::Green,
            btn_no_fg: Color::Red,
            btn_selected_fg: Color::Black,
            tab_fg: tailwind::SLATE.c200,
            tab_board_bg: tailwind::EMERALD.c900,
            tab_board_hl: tailwind::EMERALD.c700,
            tab_logs_bg: tailwind::CYAN.c900,
            tab_logs_hl: tailwind::CYAN.c700,
            sort_tab_bg: tailwind::VIOLET.c900,
            sort_tab_hl: tailwind::VIOLET.c800,
            sort_tab_hl_focused: tailwind::VIOLET.c700,
        }
    }

    /// Dark text on a light terminal background.
    pub fn light() -> Self {
        Theme {
            name: "light",
            text_primary: Color::Black,
            text_dim: Color::Gray,
            border_normal: Color::Gray,
            tile_blank_fg: Color::Rgb(220, 220, 220),
            tile_empty_bg: Color::Rgb(225, 225, 225),
            tile_preview_bg: Color::Rgb(255, 220, 120),
            cursor_dim_bg: Color::Rgb(230, 170, 170),
            premium_center: Color::Rgb(180, 140, 0),
            premium_normal: Color::Gray,
            axis_fg: Color::Gray,
            score_fg: Color::Rgb(160, 110, 0),
            equity_fg: Color::Rgb(0, 120, 140),
            win_neutral: Color::Rgb(160, 110, 0),
            leave_meh: Color::Rgb(160, 110, 0),
            phase_mid: Color::Rgb(160, 110, 0),
            msg_ok: Color::Rgb(0, 130, 0),
            msg_info: Color::DarkGray,
            title_fg: Color::Blue,
            popup_border: Color::Blue,
            popup_warn_border: Color::Rgb(180, 120, 0),
            popup_key: Color::Rgb(160, 90, 0),
            popup_header: Color::Blue,
            popup_desc: Color::DarkGray,
            highlight_bg: Color::Rgb(255, 230, 140),
            hint_key: Color::Blue,
            hint_desc: Color::DarkGray,
            tab_fg: Color::White,
            ..Self::dark()
        }
    }

    /// Only the 16 basic colors (no truecolor support).
    pub fn ansi16() -> Self {
        Theme {
            name: "ansi16",
            tile_bg: Color::Blue,
            tile_blank_fg: Color::Gray,
            tile_empty_bg: Color::Black,
            cursor_dim_bg: Color::DarkGray,
            premium_3w: Color::Magenta,
            premium_2w: Color::LightBlue,
            premium_3l: Color::Green,
            premium_2l: Color::Yellow,
            pane_rack: Color::Yellow,
            pane_suggestions: Color::Magenta,
            tab_board_active: Color::Green,
            tab_fg: Color::White,
            tab_board_bg: Color::Black,
            tab_board_hl: Color::Green,
            tab_logs_bg: Color::Black,
            tab_logs_hl: Color::Cyan,
            sort_tab_bg: Color::Black,
            sort_tab_hl: Color::DarkGray,
            sort_tab_hl_focused: Color::Magenta,
            ..Self::dark()
        }
    }

    /// No colors at all (`NO_COLOR`): meaning carried by modifiers.
    pub fn no_color() -> Self {
        let r = Color::Reset;
        Theme {
            name: "no-color",
            monochrome: true,
            glyphs: UNICODE_GLYPHS,
            text_primary: r,
            text_dim: r,
            border_normal: r,
            tile_bg: r,
            tile_fg: r,
            tile_blank_fg: r,
            tile_empty_bg: r,
            tile_preview_bg: r,
            tile_preview_fg: r,
            tile_preview_blank_fg: r,
            cursor_bg: r,
            cursor_fg: r,
            cursor_dim_bg: r,
            premium_3w: r,
            premium_2w: r,
            premium_3l: r,
            premium_2l: r,
            premium_center: r,
            premium_normal: r,
            axis_fg: r,
            score_fg: r,
            equity_fg: r,
            sim_equity_fg: r,
            win_good: r,
            win_neutral: r,
            win_bad: r,
            leave_great: r,
            leave_good: r,
            leave_meh: r,
            leave_bad: r,
            phase_early: r,
            phase_mid: r,
            phase_late: r,
            phase_endgame: r,
            msg_ok: r,
            msg_info: r,
            msg_error: r,
            msg_progress: r,
            title_fg: r,
            popup_border: r,
            popup_warn_border: r,
            popup_key: r,
            popup_header: r,
            popup_desc: r,
            highlight_bg: r,
            highlight_fg: r,
            hint_key: r,
            hint_desc: r,
            pane_rack: r,
            pane_suggestions: r,
            tab_board_active: r,
            btn_yes_fg: r,
            btn_no_fg: r,
            btn_selected_fg: r,
            tab_fg: r,
            tab_board_bg: r,
            tab_board_hl: r,
            tab_logs_bg: r,
            tab_logs_hl: r,
            sort_tab_bg: r,
            sort_tab_hl: r,
            sort_tab_hl_focused: r,
        }
    }

    /// The theme for `choice`, resolving `Auto` from the environment.
    pub fn from_choice(choice: ThemeChoice) -> Self {
        let mut theme = match choice {
            ThemeChoice::Auto => {
                if env_set("NO_COLOR") {
                    Self::no_color()
                } else if truecolor() {
                    Self::dark()
                } else {
                    Self::ansi16()
                }
            }
            ThemeChoice::Dark => Self::dark(),
            ThemeChoice::Light => Self::light(),
            ThemeChoice::Ansi16 => Self::ansi16(),
            ThemeChoice::NoColor => Self::no_color(),
        };
        if !utf8_locale() {
            theme.glyphs = ASCII_GLYPHS;
        }
        theme
    }

    /// Style of the selected row in a list.
    pub fn highlight_style(&self) -> Style {
        let style = Style::new()
            .bg(self.highlight_bg)
            .fg(self.highlight_fg)
            .add_modifier(Modifier::BOLD);
        if self.monochrome {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        }
    }

    /// Extra modifiers for the board cursor (visible without color).
    pub fn cursor_modifier(&self) -> Modifier {
        if self.monochrome {
            Modifier::REVERSED | Modifier::BOLD
        } else {
            Modifier::BOLD
        }
    }

    pub fn leave_color(&self, leave_value: f32) -> Color {
        if leave_value >= 10.0 {
            self.leave_great
        } else if leave_value >= 0.0 {
            self.leave_good
        } else if leave_value >= -10.0 {
            self.leave_meh
        } else {
            self.leave_bad
        }
    }

    pub fn win_color(&self, pct: f32) -> Color {
        if pct >= 60.0 {
            self.win_good
        } else if pct >= 45.0 {
            self.win_neutral
        } else {
            self.win_bad
        }
    }

    pub fn phase_color(&self, phase: &crate::stats::GamePhase) -> Color {
        match phase {
            crate::stats::GamePhase::Early => self.phase_early,
            crate::stats::GamePhase::Mid => self.phase_mid,
            crate::stats::GamePhase::Late => self.phase_late,
            crate::stats::GamePhase::Endgame => self.phase_endgame,
        }
    }
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// `COLORTERM=truecolor` or `24bit` (the de-facto truecolor signal).
fn truecolor() -> bool {
    std::env::var("COLORTERM")
        .map(|v| {
            let v = v.to_ascii_lowercase();
            v == "truecolor" || v == "24bit"
        })
        .unwrap_or(false)
}

/// Whether the locale says UTF-8 (`LC_ALL`, then `LC_CTYPE`, then `LANG`).
fn utf8_locale() -> bool {
    for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                let v = v.to_ascii_lowercase();
                return v.contains("utf-8") || v.contains("utf8");
            }
        }
    }
    // No locale at all: most terminals today are UTF-8.
    true
}

static THEME: OnceLock<Theme> = OnceLock::new();

/// Choose the theme for this process. Only the first call has an effect;
/// without one, [`current`] uses the dark theme.
pub fn init(choice: ThemeChoice) {
    let _ = THEME.set(Theme::from_choice(choice));
}

/// The theme in use.
pub fn current() -> &'static Theme {
    THEME.get_or_init(Theme::dark)
}

/// Style of the selected row in a list (see [`Theme::highlight_style`]).
pub fn highlight_style() -> Style {
    current().highlight_style()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_differ_where_they_should() {
        let dark = Theme::dark();
        let ansi = Theme::ansi16();
        let none = Theme::no_color();
        let light = Theme::light();
        // ansi16 uses no truecolor values.
        for c in [
            ansi.tile_bg,
            ansi.premium_3w,
            ansi.pane_rack,
            ansi.tab_board_hl,
        ] {
            assert!(!matches!(c, Color::Rgb(..)), "{c:?}");
        }
        assert!(matches!(dark.tile_bg, Color::Rgb(..)));
        assert_eq!(none.tile_bg, Color::Reset);
        assert!(none
            .highlight_style()
            .add_modifier
            .contains(Modifier::REVERSED));
        assert!(!dark
            .highlight_style()
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(light.text_primary, Color::Black);
        assert_eq!(Theme::from_choice(ThemeChoice::NoColor).name, "no-color");
    }
}
