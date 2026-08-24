//! Binary entry point for the Crossplay solver TUI.
//!
//! Sets up the terminal, loads the dictionary, and runs the main event loop.

mod app;
mod board;
mod dictionary;
mod endgame;
mod game;
mod leaves;
mod logging;
mod refengine_ffi;
mod refengine_mcts;
mod mcts;
mod settings;
mod solver;
mod stats;
mod tile;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
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
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
}

fn main() -> Result<()> {
    // A panic anywhere on the main thread would otherwise leave the user's
    // terminal in raw mode on the alternate screen with mouse capture on,
    // and the panic message would be unreadable. Restore first, then let
    // the default hook print.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    match dictionary::load_default_dictionary() {
        Ok(dict) => app.set_dictionary(dict),
        Err(e) => {
            app.message = Some(format!("Failed to load dictionary: {}", e));
        }
    }

    let res = run_app(&mut terminal, &mut app);

    restore_terminal();
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    loop {
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

    let layout = AppLayout::compute(size);

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
        let opp_skill = (app.opp_moves_measured > 0).then(|| {
            (
                app.opp_loss_total / app.opp_moves_measured as f32,
                app.opp_moves_measured,
            )
        });
        ui::stats_view::render_rack_stats(
            frame,
            rack_area,
            &app.state,
            app.focus == Focus::Rack,
            app.inferred_opponent_rack.as_ref(),
            (app.our_score, app.opp_score),
            (app.final_turn, app.final_turn_known),
            opp_skill,
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

    /// Callers hold `REF_ENGINE_TEST_LOCK` while the App lives (it may start
    /// RefEngine inference in the background).
    fn sample_app() -> App {
        let mut app = App::with_settings(settings::Settings::default());
        let dict = dictionary::load_default_dictionary().unwrap();
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
        app
    }

    /// Every board row must be visible at common terminal sizes: the board
    /// picks a geometry that fits instead of clipping its bottom rows.
    #[test]
    fn board_fits_common_terminal_sizes() {
        let _lock = crate::refengine_ffi::tests::REF_ENGINE_TEST_LOCK.lock().unwrap();
        let app = sample_app();
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

    /// Dump renders for eyeballing. Run with
    /// `cargo test --release ui_snapshots -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ui_snapshots() {
        let _lock = crate::refengine_ffi::tests::REF_ENGINE_TEST_LOCK.lock().unwrap();
        let mut app = sample_app();
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
        let _lock = crate::refengine_ffi::tests::REF_ENGINE_TEST_LOCK.lock().unwrap();
        let mut app = App::with_settings(settings::Settings::default());
        app.set_dictionary(dictionary::load_default_dictionary().unwrap());
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
