//! Binary entry point for the Crossplay solver TUI.
//!
//! Sets up the terminal, loads the dictionary, and runs the main event loop.

mod app;
mod board;
mod datafiles;
mod dictionary;
mod endgame;
mod game;
mod leaves;
mod logging;
#[cfg(feature = "refengine")]
mod refengine_ffi;
mod refengine_mcts;
mod mcts;
mod settings;
mod solver;
mod stats;
mod tile;
mod ui;

use std::io;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::{
    cursor::Show,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;

use app::{App, BoardTab, DialogState, Focus};
use ui::events::AppAction;
use ui::hit::clear_hits;
use ui::layout::AppLayout;

/// Put the terminal back the way we found it. Safe to call more than once.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        Show
    );
}

/// Restores the terminal when dropped, so every exit path from the TUI
/// (including an error from `Terminal::new` or the event loop) leaves the
/// user's terminal usable.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        // From here on the guard owns the cleanup, even if the next call
        // fails half-way.
        let guard = TerminalGuard;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Panics on the main thread restore the terminal before the message is
/// printed, or it would be unreadable on the alternate screen. Panics on
/// worker threads go to the log instead: the TUI is still running, the
/// worker's caller reports the failure, and printing to stderr would
/// scribble over the screen.
fn install_panic_hook() {
    let main_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main_thread {
            restore_terminal();
            default_hook(info);
        } else {
            logging::log(&format!(
                "Worker thread '{}' panicked: {}",
                std::thread::current().name().unwrap_or("unnamed"),
                info
            ));
        }
    }));
}

fn main() -> ExitCode {
    install_panic_hook();

    // Data is loaded before the terminal is taken over, so a missing file
    // is reported plainly on stderr with a non-zero exit code.
    let dict = match dictionary::load_default_dictionary() {
        Ok(dict) => dict,
        Err(e) => {
            eprintln!("cpsolver2: failed to load the dictionary: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let (settings, settings_warning) = settings::Settings::load_or_recover();
    let mut app = App::with_settings(settings);
    app.set_dictionary(dict);
    if let Some(warning) = datafiles::lfs_warning() {
        app.message = Some(warning);
    }
    if let Some(warning) = settings_warning {
        app.message = Some(warning);
    }

    let res = run_tui(&mut app);
    if let Err(err) = res {
        eprintln!("cpsolver2: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_tui(app: &mut App) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    run_app(&mut terminal, app)
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    loop {
        if let Ok(size) = terminal.size() {
            app.terminal_size = Some((size.width, size.height));
        }
        terminal.draw(|f| draw_ui(f, app))?;

        // Poll for evaluation and inference results.
        if app.has_background_work() {
            app.poll_mcts();
        }

        let timeout = if app.is_mcts_running() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        };

        if event::poll(timeout)? {
            let action = match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => AppAction::Nothing,
            };

            match action {
                AppAction::Quit => return Ok(()),
                AppAction::Redraw => {} // Next loop iteration redraws.
                AppAction::Nothing => {}
            }
        } else if app.is_mcts_running() {
            app.tick_spinner();
        }
    }
}

fn draw_ui(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // Clear hit map for this frame.
    clear_hits();

    let layout = AppLayout::compute(size, app.sidebar_visible);

    // Title bar.
    ui::status_bar::render_title(frame, layout.title_bar);

    // Tab bar.
    ui::log_viewer::render_tabs(
        frame,
        layout.tab_area,
        app.board_tab,
        app.focus == Focus::Board,
    );

    // Board or log viewer.
    match app.board_tab {
        BoardTab::Board => {
            let preview_tiles = if !app.suggestions.is_empty() {
                Some(
                    app.suggestions[app.selected_suggestion]
                        .tiles_used
                        .as_slice(),
                )
            } else {
                None
            };

            ui::board_view::render_board(
                frame,
                layout.board_area,
                &app.state,
                app.cursor,
                app.focus == Focus::Board,
                app.typing_direction,
                preview_tiles,
            );
        }
        BoardTab::Logs => {
            ui::log_viewer::render_log_viewer(
                frame,
                layout.board_area,
                app.log_scroll,
                app.focus == Focus::Board,
            );
        }
    }

    // Message bar.
    ui::status_bar::render_message(
        frame,
        layout.message_area,
        app.message.as_deref(),
        app.mcts_progress.as_ref(),
    );

    // Rack + stats.
    if let Some(rack_area) = layout.rack_area {
        let opp_skill = app.opp_error_estimate();
        ui::stats_view::render_rack_stats(
            frame,
            rack_area,
            &app.state,
            app.focus == Focus::Rack,
            app.inferred_opponent_rack.as_ref(),
            (app.our_score, app.opp_score),
            (app.final_turn, app.final_turn_known),
            opp_skill,
            app.inference_note(),
        );
    }

    // Sort-mode tabs.
    if let Some(tab_area) = layout.suggestions_tab_area {
        let has_sim = app.sim_results.is_some();
        ui::suggestions::render_sort_tabs(
            frame,
            tab_area,
            app.sort_mode,
            has_sim,
            app.focus == Focus::Suggestions,
        );
    }

    // Suggestions panel.
    if let Some(suggestions_area) = layout.suggestions_area {
        ui::suggestions::render_suggestions(
            frame,
            suggestions_area,
            &app.suggestions,
            app.selected_suggestion,
            app.focus == Focus::Suggestions,
            app.sim_results.as_deref(),
        );
    }

    // Hint bar.
    ui::status_bar::render_hints(
        frame,
        layout.status_bar,
        app.focus,
        !app.suggestions.is_empty(),
        app.is_mcts_running(),
        app.state.is_endgame(),
    );

    // Dialog popups on top.
    match app.dialog {
        DialogState::None => {}

        DialogState::Help => {
            let popup_area = ui::help_popup::HelpPopup::area(size);
            frame.render_widget(ui::help_popup::HelpPopup::new(), popup_area);
        }

        DialogState::EscapeMenu(selected) => {
            let popup_area = ui::menu_popup::EscapeMenuPopup::area(size);
            frame.render_widget(ui::menu_popup::EscapeMenuPopup::new(selected), popup_area);
        }

        DialogState::ConfirmQuit(yes_selected) => {
            let popup_area = ui::confirm_popup::ConfirmPopup::area(size);
            frame.render_widget(
                ui::confirm_popup::ConfirmPopup::new(
                    "Confirm Quit",
                    "Are you sure you want to quit?",
                    yes_selected,
                ),
                popup_area,
            );
        }

        DialogState::ConfirmReset(yes_selected) => {
            let popup_area = ui::confirm_popup::ConfirmPopup::area(size);
            frame.render_widget(
                ui::confirm_popup::ConfirmPopup::new(
                    "Confirm Reset",
                    "Reset the board and clear all tiles?",
                    yes_selected,
                ),
                popup_area,
            );
        }

        DialogState::ScoreInput {
            ref target,
            ref buffer,
        } => {
            use ratatui::style::{Modifier, Style};
            use ratatui::text::{Line, Span};
            use ratatui::widgets::Paragraph;

            let title = match target {
                app::ScoreTarget::Player => "Set Your Score",
                app::ScoreTarget::Opponent => "Set Opponent Score",
            };
            let popup_area = ui::popup_frame::centered_rect(size, 36, 6);
            ui::hit::register_hit(popup_area, ui::hit::HitTarget::DialogArea);
            let inner = ui::popup_frame::render_popup_frame(
                popup_area,
                frame.buffer_mut(),
                title,
                ui::theme::POPUP_BORDER,
            );

            let display = if buffer.is_empty() {
                "0".to_string()
            } else {
                buffer.clone()
            };

            let content = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  Score: ", Style::default().fg(ui::theme::TEXT_PRIMARY)),
                    Span::styled(
                        &display,
                        Style::default()
                            .fg(ui::theme::SCORE_FG)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "  Type digits, Enter to set, Esc to cancel",
                    Style::default().fg(ui::theme::TEXT_DIM),
                )),
            ];
            Paragraph::new(content).render(inner, frame.buffer_mut());
        }
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// Render the whole UI at a terminal size and return it as text lines.
    fn render_at(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_ui(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect()
    }

    /// Callers hold the RefEngine test lock while the App lives (it may start
    /// RefEngine inference in the background).
    fn sample_app() -> Option<App> {
        let mut app = App::with_settings(settings::Settings::default());
        let dict = crate::datafiles::test_data::dictionary()?;
        app.set_dictionary(dict);
        app.state = game::SolverState::from_cgp(
            "15/15/15/15/15/15/15/3BEECH7/7I7/7K7/7E7/7r7/15/15/15 AEINRS?",
        )
        .unwrap();
        app.our_score = 42;
        app.opp_score = 57;
        app.find_moves();
        app.focus = Focus::Suggestions;
        // Preview a move that stays clear of the top and bottom rows, so the
        // edge premium squares remain visible for the layout checks.
        app.selected_suggestion = app
            .suggestions
            .iter()
            .position(|m| m.tiles_used.iter().all(|(r, _, _)| *r != 0 && *r != 14))
            .unwrap_or(0);
        Some(app)
    }

    /// Every board row must be visible at common terminal sizes: the board
    /// picks a geometry that fits instead of clipping its bottom rows.
    #[test]
    fn board_fits_common_terminal_sizes() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(app) = sample_app() else {
            return;
        };
        for (w, h) in [
            (220, 60),
            (200, 50),
            (170, 45),
            (140, 40),
            (120, 34),
            (100, 30),
            (80, 24),
        ] {
            let lines = render_at(&app, w, h);
            let text = lines.join("\n");
            // Rows 1 and 15 read 3L .. 3W .. 2L .. 3W .. 3L left to right and
            // no other row does; both must be on screen.
            let edge_rows = lines
                .iter()
                .filter(|l| {
                    let b = match l.find("3W") {
                        Some(b) => b,
                        None => return false,
                    };
                    let c = match l[b..].find("2L") {
                        Some(c) => b + c,
                        None => return false,
                    };
                    let d = match l[c..].find("3W") {
                        Some(d) => c + d,
                        None => return false,
                    };
                    matches!((l.find("3L"), l.rfind("3L")), (Some(a), Some(e)) if a < b && d < e)
                })
                .count();
            assert_eq!(
                edge_rows, 2,
                "{}x{}: expected top and bottom board rows on screen\n{}",
                w, h, text
            );
            assert!(
                lines.iter().any(|l| l.contains('K') && l.contains('6')),
                "{}x{}: K tile missing\n{}",
                w,
                h,
                text
            );
        }
    }

    fn click(app: &mut App, column: u16, row: u16) -> AppAction {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// Screen position of the first occurrence of `needle`.
    fn find_text(lines: &[String], needle: &str) -> (u16, u16) {
        for (y, line) in lines.iter().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            let pat: Vec<char> = needle.chars().collect();
            if let Some(x) = chars.windows(pat.len()).position(|w| w == pat.as_slice()) {
                return (x as u16, y as u16);
            }
        }
        panic!("{needle:?} not on screen:\n{}", lines.join("\n"));
    }

    /// Clicks on a confirmation dialog's buttons land on the buttons drawn
    /// on screen; a click outside cancels; nothing reaches the panes below.
    #[test]
    fn dialog_buttons_take_clicks() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = App::with_settings(settings::Settings::default());
        let (w, h) = (120, 40);

        // "No" closes the quit dialog without quitting.
        app.dialog = DialogState::ConfirmQuit(false);
        let lines = render_at(&app, w, h);
        let (x, y) = find_text(&lines, " No ");
        assert!(matches!(click(&mut app, x + 1, y), AppAction::Redraw));
        assert_eq!(app.dialog, DialogState::None);

        // "Yes" quits.
        app.dialog = DialogState::ConfirmQuit(false);
        let lines = render_at(&app, w, h);
        let (x, y) = find_text(&lines, " Yes ");
        assert!(matches!(click(&mut app, x + 2, y), AppAction::Quit));

        // Inside the dialog but off the buttons: nothing happens.
        app.dialog = DialogState::ConfirmReset(false);
        let lines = render_at(&app, w, h);
        let (x, y) = find_text(&lines, "Reset the board");
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::ConfirmReset(false));

        // Outside: the dialog is cancelled and the board cursor below the
        // click does not move.
        let cursor = app.cursor;
        click(&mut app, 2, h - 3);
        assert_eq!(app.dialog, DialogState::None);
        assert_eq!(app.cursor, cursor);
    }

    /// Escape-menu options are clickable rows.
    #[test]
    fn menu_options_take_clicks() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = App::with_settings(settings::Settings::default());
        app.dialog = DialogState::EscapeMenu(ui::menu_popup::EscapeMenuOption::Cancel);
        let lines = render_at(&app, 100, 30);
        let (x, y) = find_text(&lines, "Reset Board");
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::ConfirmReset(false));

        app.dialog = DialogState::EscapeMenu(ui::menu_popup::EscapeMenuOption::Quit);
        let lines = render_at(&app, 100, 30);
        let (x, y) = find_text(&lines, "Quit Game");
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::ConfirmQuit(false));
    }

    /// Dump renders for eyeballing. Run with
    /// `cargo test --release ui_snapshots -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ui_snapshots() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(mut app) = sample_app() else {
            return;
        };
        // Evaluate so the results columns are populated in the renders.
        app.run_mcts_evaluation();
        while app.is_mcts_running() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            app.poll_mcts();
        }
        for (w, h) in [(200, 52), (170, 45), (120, 36), (90, 30), (80, 24)] {
            println!("===== {}x{} =====", w, h);
            for line in render_at(&app, w, h) {
                println!("{}", line.trim_end());
            }
        }
    }
}

#[cfg(test)]
mod render_bench {
    use super::*;
    use ratatui::backend::TestBackend;

    /// Frame time for a full render at a large terminal size. Run with
    /// `cargo test --release bench_render -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_render() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = App::with_settings(settings::Settings::default());
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        app.set_dictionary(dict);
        app.state = game::SolverState::from_cgp(
            "14U/5HELM3ZIN/4VIG7T/6AYES3BI/6DOWER2AL/9GOOFS1/1BRA4GOWN1I1/POETIC1QI2L1N1/OX3H1U3Y3/R3FINAlS5/T4V1K1P5/A4e1EME5/L6DON5/EtHER3AD5/D2RECOAT6 AEST",
        )
        .unwrap();
        app.find_moves();
        let backend = TestBackend::new(220, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        let t = std::time::Instant::now();
        for _ in 0..200 {
            terminal.draw(|f| draw_ui(f, &app)).unwrap();
        }
        println!("render: {:?}/frame", t.elapsed() / 200);
    }
}
