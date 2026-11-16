//! Draws a whole frame of the UI from the app state.

use ratatui::prelude::*;

use crate::app::{App, BoardTab, DialogState, Focus};
use crate::ui::hit::clear_hits;
use crate::ui::layout::AppLayout;

/// Draw the UI for `app` into `frame`, registering click targets.
pub fn draw_ui(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // Clear hit map for this frame.
    clear_hits();

    let layout = AppLayout::compute(size, app.sidebar_visible);
    if let Some((w, h)) = layout.too_small {
        crate::ui::status_bar::render_too_small(frame, size, (w, h));
        return;
    }

    // Title bar.
    crate::ui::status_bar::render_title(frame, layout.title_bar, app.game_name.as_deref());

    // Tab bar.
    crate::ui::log_viewer::render_tabs(
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

            crate::ui::board_view::render_board(
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
            crate::ui::log_viewer::render_log_viewer(
                frame,
                layout.board_area,
                app.log_scroll,
                app.focus == Focus::Board,
            );
        }
    }

    // Message bar.
    crate::ui::status_bar::render_message(
        frame,
        layout.message_area,
        app.message.as_deref(),
        app.message_kind,
        app.mcts_progress.as_ref(),
    );

    // Rack + stats.
    if let Some(rack_area) = layout.rack_area {
        let opp_skill = app.opp_error_estimate();
        crate::ui::stats_view::render_rack_stats(
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
    if let Some(rack_line) = layout.rack_line {
        crate::ui::stats_view::render_rack_line(
            frame,
            rack_line,
            &app.state,
            app.focus == Focus::Rack,
            (app.our_score, app.opp_score),
        );
    }

    // Sort-mode tabs.
    if let Some(tab_area) = layout.suggestions_tab_area {
        let has_sim = app.sim_results.is_some();
        crate::ui::suggestions::render_sort_tabs(
            frame,
            tab_area,
            app.sort_mode,
            has_sim,
            app.focus == Focus::Suggestions,
        );
    }

    // Suggestions panel.
    if let Some(suggestions_area) = layout.suggestions_area {
        let visible = app.visible_suggestions();
        crate::ui::suggestions::render_suggestions(
            frame,
            suggestions_area,
            crate::ui::suggestions::ListView {
                moves: &app.suggestions,
                visible: &visible,
                selected: app.selected_suggestion,
                offset: &app.list_offset,
                height: &app.list_height,
                filter: app.filter.as_deref(),
                filter_editing: app.filter_editing,
                sort: app.sort_mode,
            },
            app.focus == Focus::Suggestions,
            app.sim_results.as_deref(),
            app.sim_progress.as_ref(),
            app.is_mcts_running(),
        );
    }

    // Hint bar.
    crate::ui::status_bar::render_hints(frame, layout.status_bar, app);

    // Dialog popups on top.
    match app.dialog {
        DialogState::None => {}

        DialogState::Help => {
            let popup_area = crate::ui::help_popup::HelpPopup::area(size);
            frame.render_widget(
                crate::ui::help_popup::HelpPopup::new(app.help_scroll, &app.help_max_scroll),
                popup_area,
            );
        }

        DialogState::EscapeMenu(selected) => {
            let popup_area = crate::ui::menu_popup::EscapeMenuPopup::area(size);
            frame.render_widget(
                crate::ui::menu_popup::EscapeMenuPopup::new(selected),
                popup_area,
            );
        }

        DialogState::ConfirmQuit { yes: yes_selected } => {
            let popup_area = crate::ui::confirm_popup::ConfirmPopup::area(size);
            frame.render_widget(
                crate::ui::confirm_popup::ConfirmPopup::new(
                    "Confirm Quit",
                    "Are you sure you want to quit?",
                    yes_selected,
                ),
                popup_area,
            );
        }

        DialogState::ConfirmReset { yes: yes_selected } => {
            let popup_area = crate::ui::confirm_popup::ConfirmPopup::area(size);
            frame.render_widget(
                crate::ui::confirm_popup::ConfirmPopup::new(
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
        } => crate::ui::prompts::render_score_input(frame, size, *target, buffer),

        DialogState::GamePicker { selected, .. } => {
            crate::ui::game_picker::render_game_picker(frame, size, app, selected)
        }

        DialogState::SaveAs {
            ref buffer,
            ref purpose,
            pristine,
        } => crate::ui::prompts::render_save_as(
            frame,
            size,
            buffer,
            purpose,
            pristine,
            (app.message_kind == crate::app::MessageKind::Error)
                .then_some(app.message.as_deref())
                .flatten(),
        ),

        DialogState::ConfirmOverwrite { ref name, yes, .. } => {
            let message = format!("Replace the saved game {}?", name);
            let popup_area = crate::ui::confirm_popup::ConfirmPopup::area(size);
            frame.render_widget(
                crate::ui::confirm_popup::ConfirmPopup::new("Game exists", &message, yes),
                popup_area,
            );
        }

        DialogState::ConfirmDelete { ref name, yes, .. } => {
            let message = format!("Delete {}? This cannot be undone.", name);
            let popup_area = crate::ui::confirm_popup::ConfirmPopup::area(size);
            frame.render_widget(
                crate::ui::confirm_popup::ConfirmPopup::new("Delete game", &message, yes),
                popup_area,
            );
        }

        DialogState::QuickEntry { ref buffer } => crate::ui::prompts::render_quick_entry(
            frame,
            size,
            buffer,
            app.parse_quick_entry(buffer),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::events::AppAction;
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
        let mut app = App::with_settings(crate::settings::Settings::default());
        let dict = crate::datafiles::test_data::dictionary()?;
        app.set_dictionary(dict);
        app.state = crate::game::SolverState::from_cgp(
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
        let mut app = App::with_settings(crate::settings::Settings::default());
        let (w, h) = (120, 40);

        // "No" closes the quit dialog without quitting.
        app.dialog = DialogState::ConfirmQuit { yes: false };
        let lines = render_at(&app, w, h);
        let (x, y) = find_text(&lines, " No ");
        assert!(matches!(click(&mut app, x + 1, y), AppAction::Redraw));
        assert_eq!(app.dialog, DialogState::None);

        // "Yes" quits.
        app.dialog = DialogState::ConfirmQuit { yes: false };
        let lines = render_at(&app, w, h);
        let (x, y) = find_text(&lines, " Yes ");
        assert!(matches!(click(&mut app, x + 2, y), AppAction::Quit));

        // Inside the dialog but off the buttons: nothing happens.
        app.dialog = DialogState::ConfirmReset { yes: false };
        let lines = render_at(&app, w, h);
        let (x, y) = find_text(&lines, "Reset the board");
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::ConfirmReset { yes: false });

        // Outside: the dialog is cancelled and the board cursor below the
        // click does not move.
        let cursor = app.cursor;
        click(&mut app, 2, h - 3);
        assert_eq!(app.dialog, DialogState::None);
        assert_eq!(app.cursor, cursor);
    }

    /// After a simulation the table shows its columns, and clicking a
    /// column header sorts by it.
    #[test]
    fn simulation_columns_and_header_sorting() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(mut app) = sample_app() else {
            return;
        };
        app.settings.simulation.budget_secs = 1;
        app.run_mcts_evaluation();
        while app.is_mcts_running() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.poll_mcts();
        }
        let lines = render_at(&app, 200, 50);
        let text = lines.join("\n");
        for header in ["Spread", "Win%", "\u{0394}", "Status", "Bingo"] {
            assert!(text.contains(header), "{header} column missing\n{text}");
        }
        assert!(
            text.contains("simulated "),
            "coverage header missing\n{text}"
        );
        assert!(
            app.suggestions
                .iter()
                .any(|m| m.kind == crate::solver::MoveKind::Pass),
            "pass is a candidate"
        );
        let total = app.suggestions.len();
        let simulated = app
            .sim_results
            .as_ref()
            .map_or(0, |r| r.iter().filter(|r| r.iterations > 0).count());
        assert!(simulated > 0 && simulated <= total);

        app.set_sort_mode(crate::app::SortMode::Score);
        let lines = render_at(&app, 200, 50);
        let (x, y) = find_text(&lines, "Spread");
        click(&mut app, x + 1, y);
        assert_eq!(app.sort_mode, crate::app::SortMode::MctsEquity);
    }

    /// Escape-menu options are clickable rows.
    #[test]
    fn menu_options_take_clicks() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = App::with_settings(crate::settings::Settings::default());
        app.dialog = DialogState::EscapeMenu(crate::ui::menu_popup::EscapeMenuOption::Cancel);
        let lines = render_at(&app, 100, 30);
        let (x, y) = find_text(&lines, "Reset Board");
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::ConfirmReset { yes: false });

        app.dialog = DialogState::EscapeMenu(crate::ui::menu_popup::EscapeMenuOption::Quit);
        let lines = render_at(&app, 100, 30);
        let (x, y) = find_text(&lines, "Quit Game");
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::ConfirmQuit { yes: false });
    }

    /// The picker draws its games (locked ones marked), a click selects a
    /// row, a second click opens it, and the title bar names the game.
    #[test]
    fn game_picker_renders_and_takes_clicks() {
        use crate::games::{self, OppLosses, SavedGame};
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let _lock = crate::datafiles::test_data::refengine_lock();
        let dir = games::test_dir("ui-picker");
        for (name, updated) in [("Alice", 30), ("Bob", 20)] {
            let game = SavedGame {
                name: name.to_string(),
                cgp: "15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 EINRST/ 337/205 0"
                    .to_string(),
                updated,
                opp_losses: OppLosses::default(),
            };
            games::save(&dir, &game).unwrap();
        }
        std::fs::write(games::lock_path(&dir, "alice"), "1\n").unwrap();
        let mut app = App::with_settings(crate::settings::Settings::default());
        app.games_dir = Some(dir.clone());
        app.show_game_picker(/* startup= */ true);
        let lines = render_at(&app, 120, 40);
        let text = lines.join("\n");
        assert!(text.contains("Saved games"), "{text}");
        assert!(text.contains("Alice (open in another window)"), "{text}");
        assert!(text.contains("337\u{2013}205"), "{text}");
        assert!(text.contains("Bag"), "{text}");

        let (x, y) = find_text(&lines, "Bob");
        click(&mut app, x, y);
        assert!(matches!(
            app.dialog,
            DialogState::GamePicker { selected: 1, .. }
        ));
        // The wheel moves the selection.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            app.dialog,
            DialogState::GamePicker { selected: 0, .. }
        ));
        click(&mut app, x, y);
        click(&mut app, x, y);
        assert_eq!(app.dialog, DialogState::None, "double-click opens");
        assert_eq!(app.game_name.as_deref(), Some("Bob"));
        let title = render_at(&app, 120, 40).remove(0);
        assert!(title.contains("Crossplay Solver \u{00b7} Bob"), "{title}");

        // The naming prompt.
        app.show_save_as(crate::app::SaveAsPurpose::Quit);
        let text = render_at(&app, 120, 40).join("\n");
        assert!(text.contains("Name this game before quitting"), "{text}");
        assert!(text.contains("Name: Bob_"), "{text}");
        app.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Snapshots of the whole UI at three terminal sizes, as text. The
    /// position and messages are fixed so the renders are deterministic;
    /// they need the real leave table (static equity is on screen), so
    /// builds without RefEngine or LFS data skip them.
    #[cfg(feature = "refengine")]
    #[test]
    fn draw_ui_snapshots() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        if crate::datafiles::test_data::refengine_data().is_none() {
            return;
        }
        let Some(mut app) = sample_app() else {
            return;
        };
        app.set_message(crate::app::MessageKind::Info, "Found 6,862 moves");
        for (w, h) in [(80, 24), (120, 40), (200, 60)] {
            let text = render_at(&app, w, h)
                .iter()
                .map(|l| l.trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            insta::assert_snapshot!(format!("draw_ui_{}x{}", w, h), text);
        }
    }

    /// A narrow but tall terminal stacks a one-line rack and a short list
    /// under the board; a tiny one says how big it needs to be.
    #[test]
    fn narrow_and_tiny_terminals() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(app) = sample_app() else {
            return;
        };
        let text = render_at(&app, 80, 44).join("\n");
        assert!(text.contains(" Rack "), "one-line rack\n{text}");
        assert!(text.contains("Suggestions"), "short list\n{text}");

        let tiny = render_at(&app, 40, 12).join("\n");
        assert!(tiny.contains("terminal too small"), "{tiny}");
        assert!(tiny.contains("need 50"), "{tiny}");
    }

    /// The help is two columns when wide and scrolls when narrow.
    #[test]
    fn help_layout() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = App::with_settings(crate::settings::Settings::default());
        app.dialog = DialogState::Help;
        let wide = render_at(&app, 140, 50);
        let row = wide
            .iter()
            .find(|l| l.contains("EVERYWHERE"))
            .expect("first section");
        assert!(row.contains("RACK"), "two columns side by side\n{row}");

        let narrow = render_at(&app, 70, 24).join("\n");
        assert!(narrow.contains("EVERYWHERE"));
        assert!(!narrow.contains("MOUSE"), "the end needs scrolling");
        app.help_scroll = 60;
        let scrolled = render_at(&app, 70, 24).join("\n");
        assert!(scrolled.contains("MOUSE"), "{scrolled}");
    }

    /// The wheel scrolls the pane under the pointer and stops at the ends.
    #[test]
    fn wheel_scrolls_the_pane_under_the_pointer() {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(mut app) = sample_app() else {
            return;
        };
        app.focus = Focus::Board;
        app.selected_suggestion = 0;
        let lines = render_at(&app, 200, 50);
        let (x, y) = find_text(&lines, "Suggestions");
        let wheel = |app: &mut App, kind, column, row| {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        wheel(&mut app, MouseEventKind::ScrollDown, x, y + 4);
        assert_eq!(
            app.selected_suggestion, 3,
            "list under the pointer scrolled"
        );
        wheel(&mut app, MouseEventKind::ScrollUp, x, y + 4);
        wheel(&mut app, MouseEventKind::ScrollUp, x, y + 4);
        assert_eq!(app.selected_suggestion, 0, "stops at the top");
        // Over the board (board tab) the wheel does nothing.
        let (bx, by) = find_text(&lines, "3W");
        assert!(matches!(
            wheel(&mut app, MouseEventKind::ScrollDown, bx, by),
            AppAction::Nothing
        ));
        assert!(matches!(
            wheel(&mut app, MouseEventKind::Moved, bx, by),
            AppAction::Nothing
        ));
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
        let mut app = App::with_settings(crate::settings::Settings::default());
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        app.set_dictionary(dict);
        app.state = crate::game::SolverState::from_cgp(
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
