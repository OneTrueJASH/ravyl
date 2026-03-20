mod app;
mod board;
mod dictionary;
mod endgame;
mod game;
mod logging;
mod refengine_ffi;
mod refengine_mcts;
mod mcts;
mod settings;
mod solver;
mod stats;
mod tile;
mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

use app::{App, BoardTab, DialogState, Focus};
use ui::events::AppAction;
use ui::hit::clear_hits;
use ui::layout::AppLayout;

fn main() -> Result<()> {
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

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
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

        // Poll for simulation results.
        if app.is_mcts_running() {
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
        ui::stats_view::render_rack_stats(
            frame,
            rack_area,
            &app.state,
            &app.state.rack,
            app.focus == Focus::Rack,
            app.inferred_opponent_rack.as_ref(),
        );
    }

    // Sort-mode tabs.
    if let Some(tab_area) = layout.suggestions_tab_area {
        let has_sim = app.mcts_results.is_some() || app.refengine_results.is_some();
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
            app.mcts_results.as_deref(),
            app.refengine_results.as_deref(),
        );
    }

    // Hint bar.
    ui::status_bar::render_hints(
        frame,
        layout.status_bar,
        app.focus,
        !app.suggestions.is_empty(),
        app.is_mcts_running(),
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

        DialogState::Settings(ref state) => {
            let popup_area = ui::settings_popup::SettingsPopup::area(size);
            frame.render_widget(ui::settings_popup::SettingsPopup::new(state), popup_area);
        }
    }
}
