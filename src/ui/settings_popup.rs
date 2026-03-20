//! Settings popup dialog for configuring application settings.

use crate::app::SettingsDialogState;
use crate::settings::Settings;
use crate::ui::popup_frame::{centered_rect, render_popup_frame};
use crate::ui::theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// Settings tab categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    RefEngine,
    Mcts,
    Display,
    Game,
    Performance,
}

impl SettingsTab {
    pub fn label(&self) -> &'static str {
        match self {
            SettingsTab::RefEngine => "RefEngine",
            SettingsTab::Mcts => "MCTS",
            SettingsTab::Display => "Display",
            SettingsTab::Game => "Game",
            SettingsTab::Performance => "Perf",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            SettingsTab::RefEngine => SettingsTab::Mcts,
            SettingsTab::Mcts => SettingsTab::Display,
            SettingsTab::Display => SettingsTab::Game,
            SettingsTab::Game => SettingsTab::Performance,
            SettingsTab::Performance => SettingsTab::RefEngine,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            SettingsTab::RefEngine => SettingsTab::Performance,
            SettingsTab::Mcts => SettingsTab::RefEngine,
            SettingsTab::Display => SettingsTab::Mcts,
            SettingsTab::Game => SettingsTab::Display,
            SettingsTab::Performance => SettingsTab::Game,
        }
    }

    pub fn row_count(&self) -> usize {
        match self {
            SettingsTab::RefEngine => 5,
            SettingsTab::Mcts => 3,
            SettingsTab::Display => 2,
            SettingsTab::Game => 1,
            SettingsTab::Performance => 1,
        }
    }
}

fn setting_description(tab: SettingsTab, row: usize) -> &'static str {
    match tab {
        SettingsTab::RefEngine => match row {
            0 => "Look-ahead depth for simulation. Higher = more accurate but slower. (1-6)",
            1 => "Monte Carlo iterations per move. More = better estimates. (100-10000)",
            2 => "Number of top moves to simulate. More = broader analysis. (5-150)",
            3 => "Auto-adjust parameters based on game phase and tile count.",
            4 => "Search depth for endgame solver when few tiles remain. (4-12)",
            _ => "",
        },
        SettingsTab::Mcts => match row {
            0 => "Number of top moves to evaluate with RefEngine. (3-20)",
            1 => "Simulation iterations per candidate move. (10-200)",
            2 => "Use multiple CPU cores for faster evaluation.",
            _ => "",
        },
        SettingsTab::Display => match row {
            0 => "Default sorting method for the suggestions list.",
            1 => "Show rack and suggestions panels on startup.",
            _ => "",
        },
        SettingsTab::Game => match row {
            0 => "Word list: NWL23 (North America), CSW21 (International).",
            _ => "",
        },
        SettingsTab::Performance => match row {
            0 => "CPU threads for parallel work. 0 = auto-detect. (0-32)",
            _ => "",
        },
    }
}

pub struct SettingsPopup<'a> {
    state: &'a SettingsDialogState,
}

impl<'a> SettingsPopup<'a> {
    pub fn new(state: &'a SettingsDialogState) -> Self {
        Self { state }
    }

    pub fn area(terminal_area: Rect) -> Rect {
        centered_rect(terminal_area, 80, 24)
    }

    fn render_tabs(&self, width: u16) -> Line<'static> {
        let tabs = [
            SettingsTab::RefEngine,
            SettingsTab::Mcts,
            SettingsTab::Display,
            SettingsTab::Game,
            SettingsTab::Performance,
        ];

        let mut spans = Vec::new();
        for (i, tab) in tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ", Style::default()));
            }

            let is_selected = self.state.tab == *tab;
            let style = if is_selected {
                Style::default()
                    .fg(theme::BTN_SELECTED_FG)
                    .bg(theme::POPUP_BORDER)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT_MUTED)
            };

            spans.push(Span::styled(format!(" {} ", tab.label()), style));
        }

        let content_len: usize = spans.iter().map(|s| s.content.len()).sum();
        let padding = width.saturating_sub(content_len as u16 + 2) as usize;
        if padding > 0 {
            spans.push(Span::styled(" ".repeat(padding), Style::default()));
        }

        Line::from(spans)
    }

    fn render_setting_row(
        &self,
        label: &str,
        value: &str,
        row_idx: usize,
        is_editable: bool,
    ) -> Line<'static> {
        let is_selected = self.state.row == row_idx;
        let is_editing = is_selected && self.state.editing;

        let label_style = if is_selected {
            Style::default().fg(theme::POPUP_BORDER)
        } else {
            Style::default().fg(theme::TEXT_MUTED)
        };

        let value_style = if is_editing {
            Style::default()
                .fg(theme::BTN_SELECTED_FG)
                .bg(theme::SCORE_FG)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default()
                .fg(theme::BTN_SELECTED_FG)
                .bg(theme::POPUP_BORDER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_PRIMARY)
        };

        let prefix = if is_selected { " > " } else { "   " };
        let displayed_value = if is_editing {
            format!("{}_", &self.state.edit_buffer)
        } else {
            value.to_string()
        };

        let hint = if is_selected && !is_editable && !is_editing {
            " [Space/←→]"
        } else {
            ""
        };

        Line::from(vec![
            Span::styled(prefix, label_style),
            Span::styled(format!("{:20}", label), label_style),
            Span::styled(format!(" {:>8} ", displayed_value), value_style),
            Span::styled(hint, Style::default().fg(theme::TEXT_DIM)),
        ])
    }

    fn render_bool_row(&self, label: &str, value: bool, row_idx: usize) -> Line<'static> {
        let display = if value { "ON" } else { "OFF" };
        self.render_setting_row(label, display, row_idx, false)
    }

    fn build_content(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(self.render_tabs(width));
        lines.push(Line::from(Span::styled(
            "\u{2500}".repeat(width.saturating_sub(2) as usize),
            Style::default().fg(theme::TEXT_DIM),
        )));

        let settings = &self.state.pending;
        match self.state.tab {
            SettingsTab::RefEngine => {
                lines.push(self.render_setting_row(
                    "Plies:",
                    &settings.refengine.plies.to_string(),
                    0,
                    true,
                ));
                lines.push(self.render_setting_row(
                    "Iterations:",
                    &settings.refengine.iterations.to_string(),
                    1,
                    true,
                ));
                lines.push(self.render_setting_row(
                    "Num Plays:",
                    &settings.refengine.num_plays.to_string(),
                    2,
                    true,
                ));
                lines.push(self.render_bool_row(
                    "Adaptive Mode:",
                    settings.refengine.adaptive,
                    3,
                ));
                lines.push(self.render_setting_row(
                    "Endgame Plies:",
                    &settings.refengine.endgame_plies.to_string(),
                    4,
                    true,
                ));
            }
            SettingsTab::Mcts => {
                lines.push(self.render_setting_row(
                    "Candidates:",
                    &settings.mcts.num_candidates.to_string(),
                    0,
                    true,
                ));
                lines.push(self.render_setting_row(
                    "Iterations/Candidate:",
                    &settings.mcts.iterations_per_candidate.to_string(),
                    1,
                    true,
                ));
                lines.push(self.render_bool_row("Parallel Mode:", settings.mcts.parallel, 2));
            }
            SettingsTab::Display => {
                lines.push(self.render_setting_row(
                    "Default Sort:",
                    settings.display.default_sort.label(),
                    0,
                    false,
                ));
                lines.push(self.render_bool_row(
                    "Sidebar Visible:",
                    settings.display.sidebar_visible,
                    1,
                ));
            }
            SettingsTab::Game => {
                lines.push(self.render_setting_row(
                    "Lexicon:",
                    settings.game.lexicon.label(),
                    0,
                    false,
                ));
            }
            SettingsTab::Performance => {
                let threads_display = if settings.performance.threads == 0 {
                    "Auto".to_string()
                } else {
                    settings.performance.threads.to_string()
                };
                lines.push(self.render_setting_row("Threads:", &threads_display, 0, true));
            }
        }

        while lines.len() < 9 {
            lines.push(Line::from(""));
        }

        lines.push(Line::from(Span::styled(
            "\u{2500}".repeat(width.saturating_sub(2) as usize),
            Style::default().fg(theme::TEXT_DIM),
        )));

        let description = setting_description(self.state.tab, self.state.row);
        lines.push(Line::from(Span::styled(
            format!(" {}", description),
            Style::default()
                .fg(theme::TEXT_PRIMARY)
                .add_modifier(Modifier::ITALIC),
        )));

        lines.push(Line::from(Span::styled(
            "\u{2500}".repeat(width.saturating_sub(2) as usize),
            Style::default().fg(theme::TEXT_DIM),
        )));

        if self.state.tab == SettingsTab::Game {
            lines.push(Line::from(Span::styled(
                " Note: Lexicon change requires app restart",
                Style::default()
                    .fg(theme::SCORE_FG)
                    .add_modifier(Modifier::ITALIC),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::styled(" Tab", Style::default().fg(theme::POPUP_BORDER)),
                Span::styled(": tabs  ", Style::default().fg(theme::TEXT_DIM)),
                Span::styled("↑↓", Style::default().fg(theme::POPUP_BORDER)),
                Span::styled(": navigate  ", Style::default().fg(theme::TEXT_DIM)),
                Span::styled("Enter", Style::default().fg(theme::POPUP_BORDER)),
                Span::styled(": edit  ", Style::default().fg(theme::TEXT_DIM)),
                Span::styled("Space", Style::default().fg(theme::POPUP_BORDER)),
                Span::styled(": toggle", Style::default().fg(theme::TEXT_DIM)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled(" ^S", Style::default().fg(theme::POPUP_BORDER)),
            Span::styled(": save and close  ", Style::default().fg(theme::TEXT_DIM)),
            Span::styled("Esc", Style::default().fg(theme::POPUP_BORDER)),
            Span::styled(": cancel", Style::default().fg(theme::TEXT_DIM)),
        ]));

        lines
    }
}

impl Widget for SettingsPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = render_popup_frame(area, buf, "Settings", theme::POPUP_BORDER);
        let content = self.build_content(inner.width);
        let paragraph = Paragraph::new(content);
        paragraph.render(inner, buf);
    }
}

/// Get the current value as a string for editing.
pub fn get_setting_value(settings: &Settings, tab: SettingsTab, row: usize) -> String {
    match tab {
        SettingsTab::RefEngine => match row {
            0 => settings.refengine.plies.to_string(),
            1 => settings.refengine.iterations.to_string(),
            2 => settings.refengine.num_plays.to_string(),
            3 => return String::new(),
            4 => settings.refengine.endgame_plies.to_string(),
            _ => String::new(),
        },
        SettingsTab::Mcts => match row {
            0 => settings.mcts.num_candidates.to_string(),
            1 => settings.mcts.iterations_per_candidate.to_string(),
            2 => return String::new(),
            _ => String::new(),
        },
        SettingsTab::Display => String::new(),
        SettingsTab::Game => String::new(),
        SettingsTab::Performance => {
            if row == 0 {
                settings.performance.threads.to_string()
            } else {
                String::new()
            }
        }
    }
}

pub fn is_toggle_setting(tab: SettingsTab, row: usize) -> bool {
    match tab {
        SettingsTab::RefEngine => row == 3,
        SettingsTab::Mcts => row == 2,
        SettingsTab::Display => row == 1,
        SettingsTab::Game => false,
        SettingsTab::Performance => false,
    }
}

pub fn is_cycle_setting(tab: SettingsTab, row: usize) -> bool {
    match tab {
        SettingsTab::Display => row == 0,
        SettingsTab::Game => row == 0,
        _ => false,
    }
}

pub fn toggle_setting(settings: &mut Settings, tab: SettingsTab, row: usize) {
    match tab {
        SettingsTab::RefEngine if row == 3 => {
            settings.refengine.adaptive = !settings.refengine.adaptive;
        }
        SettingsTab::Mcts if row == 2 => {
            settings.mcts.parallel = !settings.mcts.parallel;
        }
        SettingsTab::Display if row == 1 => {
            settings.display.sidebar_visible = !settings.display.sidebar_visible;
        }
        _ => {}
    }
}

pub fn cycle_setting_next(settings: &mut Settings, tab: SettingsTab, row: usize) {
    match tab {
        SettingsTab::Display if row == 0 => {
            settings.display.default_sort = settings.display.default_sort.next();
        }
        SettingsTab::Game if row == 0 => {
            settings.game.lexicon = settings.game.lexicon.next();
        }
        _ => {}
    }
}

pub fn cycle_setting_prev(settings: &mut Settings, tab: SettingsTab, row: usize) {
    match tab {
        SettingsTab::Display if row == 0 => {
            settings.display.default_sort = settings.display.default_sort.prev();
        }
        SettingsTab::Game if row == 0 => {
            settings.game.lexicon = settings.game.lexicon.prev();
        }
        _ => {}
    }
}

pub fn apply_edit(settings: &mut Settings, tab: SettingsTab, row: usize, value: &str) -> bool {
    let Ok(num) = value.parse::<u32>() else {
        return false;
    };

    match tab {
        SettingsTab::RefEngine => match row {
            0 => {
                if (1..=6).contains(&num) {
                    settings.refengine.plies = num as u8;
                    return true;
                }
            }
            1 => {
                if (100..=10000).contains(&num) {
                    settings.refengine.iterations = num;
                    return true;
                }
            }
            2 => {
                if (5..=150).contains(&num) {
                    settings.refengine.num_plays = num;
                    return true;
                }
            }
            4 => {
                if (4..=12).contains(&num) {
                    settings.refengine.endgame_plies = num as u8;
                    return true;
                }
            }
            _ => {}
        },
        SettingsTab::Mcts => match row {
            0 => {
                if (3..=20).contains(&num) {
                    settings.mcts.num_candidates = num;
                    return true;
                }
            }
            1 => {
                if (10..=200).contains(&num) {
                    settings.mcts.iterations_per_candidate = num;
                    return true;
                }
            }
            _ => {}
        },
        SettingsTab::Performance => {
            if row == 0 && num <= 32 {
                settings.performance.threads = num;
                return true;
            }
        }
        _ => {}
    }

    false
}
