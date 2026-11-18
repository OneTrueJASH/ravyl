use super::engine::spawn_worker;
use super::*;
use crate::board::{Direction, BOARD_SIZE};
use crate::endgame::FinalTurn;
use crate::game::SolverState;
use crate::inference::RackInference as InferenceResult;
use crate::settings::Settings;
use crate::sim::{ArmStatus, ArmSummary, SimSnapshot};
use crate::solver::Move;
use crate::tile::Tile;
use crate::ui::events::AppAction;
use crate::ui::hit::HitTarget;
use crate::ui::menu_popup::EscapeMenuOption;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Build an App without touching the user's settings file.
fn test_app() -> App {
    let mut settings = Settings::default();
    // Keep simulations short in tests.
    settings.simulation.budget_secs = 2;
    App::with_settings(settings)
}

/// A simulation job the test controls: the sender for the final
/// snapshot and the live slot.
fn fake_sim_job(
    app: &App,
    candidates: Vec<Move>,
) -> (
    SimJob,
    mpsc::Sender<SimSnapshot>,
    Arc<Mutex<Option<SimSnapshot>>>,
) {
    let (tx, rx) = mpsc::channel();
    let live = Arc::new(Mutex::new(None));
    let job = SimJob {
        stamp: app.position_stamp,
        cancel: Arc::new(AtomicBool::new(false)),
        live: Arc::clone(&live),
        rx,
        candidates: Arc::new(candidates),
        first: true,
    };
    (job, tx, live)
}

fn fake_snapshot(spreads: &[(f32, usize)], done: bool) -> SimSnapshot {
    SimSnapshot {
        arms: spreads
            .iter()
            .map(|&(spread, n)| ArmSummary {
                n,
                spread,
                spread_se: 1.0,
                win: 0.5,
                win_se: 0.05,
                delta: 0.0,
                delta_se: 0.0,
                our_bingo: 0.0,
                opp_bingo: 0.0,
                status: if n > 0 {
                    ArmStatus::Racing
                } else {
                    ArmStatus::Pending
                },
            })
            .collect(),
        playouts: spreads.iter().map(|s| s.1 as u64).sum(),
        elapsed: Duration::from_millis(500),
        simulated: spreads.iter().filter(|s| s.1 > 0).count(),
        floored: spreads.iter().filter(|s| s.1 > 0).count(),
        total: spreads.len(),
        done,
        budget: Some(Duration::from_secs(2)),
        run_elapsed: Duration::from_millis(500),
    }
}

/// The full Crossplay tile set, in distribution order.
const TILE_SET: &str = "AAAAAAAAABBCCDDDDEEEEEEEEEEEEFFGGGHHHIIIIIIIIJKLLLLMMNNNNNOOOOOOOOPPQRRRRRRSSSSSTTTTTTUUUVVWWXYYZ???";

/// Put the first `on_board` tiles of the set on the board (row-major)
/// and the next `in_rack` tiles in our rack, so exactly
/// `100 - on_board - in_rack` tiles are unseen.
fn state_with(on_board: usize, in_rack: usize) -> SolverState {
    let mut state = SolverState::new();
    let tiles: Vec<char> = TILE_SET.chars().collect();
    for (i, &ch) in tiles.iter().take(on_board).enumerate() {
        state.place_tile(i / BOARD_SIZE, i % BOARD_SIZE, Tile::new(ch));
    }
    for &ch in tiles.iter().skip(on_board).take(in_rack) {
        assert!(state.add_to_rack(ch));
    }
    state
}

fn fake_move(tiles: usize) -> Move {
    Move {
        kind: crate::solver::MoveKind::Play,
        word: "X".repeat(tiles),
        row: 7,
        col: 0,
        direction: Direction::Horizontal,
        score: 5,
        tiles_used: (0..tiles).map(|i| (7, i, Tile::new('X'))).collect(),
        is_bingo: false,
        leave: vec![],
        equity: 5.0,
    }
}

#[test]
fn final_turn_inferred_from_applied_move() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(dict) = crate::datafiles::test_data::dictionary() else {
        return;
    };
    let mut app = test_app();
    app.dictionary = Some(Arc::new(dict));

    // 80 tiles on the board + 7 in our rack = 13 unseen = bag of 6.
    app.state = state_with(80, 7);
    assert_eq!(app.state.unseen_tile_count(), 13);
    assert_eq!(app.final_turn, FinalTurn::Opponent);

    // Playing 3 tiles leaves 3 in the bag: the opponent could empty it.
    app.suggestions = vec![fake_move(3)];
    app.apply_suggestion();
    assert_eq!(app.final_turn, FinalTurn::Opponent);
    app.undo();

    // Playing 6 tiles draws the last one: we get the final move.
    app.suggestions = vec![fake_move(6)];
    app.apply_suggestion();
    assert_eq!(app.final_turn, FinalTurn::Us);

    // Undo restores the previous inference.
    app.undo();
    assert_eq!(app.final_turn, FinalTurn::Opponent);

    // Manual toggle and reset.
    app.toggle_final_turn();
    assert_eq!(app.final_turn, FinalTurn::Us);
    app.reset();
    assert_eq!(app.final_turn, FinalTurn::Opponent);
}

/// An auto-scored opponent move is measured for equity loss; undoing
/// the auto-score rolls the measurement back too.
#[test]
fn opponent_moves_are_measured_and_undone() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(dict) = crate::datafiles::test_data::dictionary() else {
        return;
    };
    let mut app = test_app();
    app.dictionary = Some(Arc::new(dict));

    // Play our own move first so the board snapshot exists.
    for ch in ['A', 'E', 'I', 'O', 'U', 'S', 'T'] {
        app.state.add_to_rack(ch);
    }
    app.find_moves();
    assert!(!app.suggestions.is_empty());
    app.apply_suggestion();
    assert_eq!(app.opp_moves_measured, 0);

    // The opponent's tiles appear on the board; the next Ctrl+S
    // auto-scores and measures them. A 3-tile word for a handful of
    // points is far below the best completions can do, so the measured
    // loss is reliably positive.
    app.state.place_tile(0, 0, Tile::new('C'));
    app.state.place_tile(0, 1, Tile::new('A'));
    app.state.place_tile(0, 2, Tile::new('T'));
    app.clear_rack();
    for ch in ['R', 'E', 'T', 'I', 'N', 'A', 'S'] {
        app.state.add_to_rack(ch);
    }
    let score_before = app.opp_score;
    let stamp_before = app.opp_move_stamp;
    app.find_moves();
    assert!(app.opp_score > score_before, "opponent move was scored");
    assert_eq!(app.opp_moves_measured, 1);
    assert!(app.opp_loss_total > 0.0, "a weak move measures a real loss");
    assert_eq!(
        app.opp_move_stamp,
        stamp_before + 1,
        "a scored opponent move bumps the inference stamp"
    );
    assert!(
        app.inferred_opponent_rack.is_none(),
        "a scored opponent move invalidates the stored inference"
    );

    // Undo the auto-score: the measurement goes with it.
    app.undo();
    assert_eq!(app.opp_moves_measured, 0);
    assert_eq!(app.opp_loss_total, 0.0);
    assert_eq!(app.opp_temperature(), 0.0);
}

/// Inference runs after the ranking finishes; the main loop must keep
/// polling until it has landed, or the stats pane never shows it and
/// the next evaluation would pick up a stale result.
#[test]
fn inference_is_polled_after_ranking() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(dict) = crate::datafiles::test_data::dictionary() else {
        return;
    };
    let mut app = test_app();
    app.dictionary = Some(Arc::new(dict));
    // Our opening, then the opponent hooked an S: the diff yields tiles.
    app.state =
        SolverState::from_cgp("15/15/15/15/15/15/15/3ANESTRI5/15/15/15/15/15/15/15 DEGILOU")
            .unwrap();
    app.last_applied_board = Some(app.state.board.clone());
    app.state.place_tile(7, 10, Tile::new('S'));
    app.find_moves();
    app.run_mcts_evaluation();
    let started = std::time::Instant::now();
    while app.has_background_work() && started.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_mcts();
    }
    assert!(!app.has_background_work());
    assert!(app.sim_results.is_some());
    assert!(
        app.inferred_opponent_rack.is_some(),
        "inference result never arrived"
    );
}

#[test]
fn our_move_ends_game_depends_on_final_turn() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    app.state = state_with(90, 3);
    assert_eq!(app.state.unseen_tile_count(), 7);
    assert!(app.state.is_endgame());

    app.final_turn = FinalTurn::Opponent;
    assert!(!app.our_move_ends_game(), "opponent still replies");
    app.final_turn = FinalTurn::Us;
    assert!(app.our_move_ends_game(), "we emptied the bag: no reply");

    // Opponent out of tiles: last move regardless of who emptied the bag.
    app.state = state_with(95, 5);
    assert_eq!(app.state.unseen_tile_count(), 0);
    app.final_turn = FinalTurn::Opponent;
    assert!(app.our_move_ends_game());
}

// ── Keystroke-script and regression tests ──────────────────────────

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_keys(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

/// An app with the dictionary and a midgame position, solved.
fn solved_app() -> Option<App> {
    let dict = crate::datafiles::test_data::dictionary()?;
    let mut app = test_app();
    app.set_dictionary(dict);
    app.state =
        SolverState::from_cgp("15/15/15/15/15/15/15/3BEECH7/7I7/7K7/7E7/7r7/15/15/15 AEINRST")
            .unwrap();
    app.find_moves();
    assert!(!app.suggestions.is_empty());
    Some(app)
}

/// Wait for the running evaluation (if any) to finish and be polled.
fn finish_evaluation(app: &mut App) {
    let started = Instant::now();
    while app.is_mcts_running() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(10));
        app.poll_mcts();
    }
}

/// Editing the position while an evaluation runs cancels it: its flag
/// is set and the job is gone, so no result can land on the new
/// position.
#[test]
fn edits_cancel_a_running_evaluation() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.run_mcts_evaluation();
    let flag = Arc::clone(&app.sim_job.as_ref().expect("job started").cancel);
    let stamp = app.position_stamp;

    app.focus = Focus::Rack;
    app.handle_key(key(KeyCode::Backspace));

    assert!(
        flag.load(AtomicOrdering::Relaxed),
        "the worker was told to stop"
    );
    assert!(!app.is_mcts_running());
    assert!(app.mcts_progress.is_none());
    assert!(app.position_stamp > stamp);
}

/// A result stamped for an older position is dropped on receipt.
#[test]
fn stale_results_are_dropped() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    app.suggestions = vec![fake_move(2)];
    let before = app.suggestions.clone();
    let (job, tx, _live) = fake_sim_job(&app, vec![fake_move(3)]);
    app.sim_job = Some(job);
    // The position moves on without going through the usual paths
    // (which would also cancel the job), then the old result arrives.
    app.position_stamp += 1;
    tx.send(fake_snapshot(&[(99.0, 10)], /* done= */ true))
        .unwrap();
    app.poll_mcts();
    assert!(app.sim_job.is_none());
    assert!(app.sim_results.is_none(), "stale result was applied");
    assert_eq!(app.suggestions.len(), before.len());
    assert_eq!(app.suggestions[0].word, before[0].word);
}

/// The bug behind stale results: an evaluation that finished after the
/// opponent's tiles were typed used to snapshot the live board, which
/// swallowed the opponent's move. Now the snapshot is the board when
/// the evaluation started and the late result is dropped, so the next
/// solve still scores their move.
#[test]
fn opponent_tiles_typed_during_evaluation_are_scored() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    let board_at_start = app.state.board.clone();
    app.run_mcts_evaluation();
    assert_eq!(
        app.last_applied_board
            .as_ref()
            .map(|b| b.get_tile(7, 3).is_some()),
        Some(true),
        "snapshot taken when the evaluation starts"
    );

    // The opponent plays AT through the B of BEECH while we evaluate.
    app.focus = Focus::Board;
    app.cursor = (6, 3);
    app.typing_direction = Direction::Vertical;
    type_keys(&mut app, "A");
    app.cursor = (8, 3);
    type_keys(&mut app, "T");
    finish_evaluation(&mut app);
    assert!(app.sim_results.is_none());
    for r in 0..BOARD_SIZE {
        for c in 0..BOARD_SIZE {
            assert_eq!(
                app.last_applied_board
                    .as_ref()
                    .unwrap()
                    .get_tile(r, c)
                    .is_some(),
                board_at_start.get_tile(r, c).is_some(),
                "snapshot changed at ({r},{c})"
            );
        }
    }

    let opp_before = app.opp_score;
    app.handle_key(ctrl('s'));
    assert!(app.opp_score > opp_before, "the opponent's move was scored");
    app.cancel_evaluation();
}

/// M while an evaluation runs restarts it instead of being ignored.
#[test]
fn rerunning_restarts_the_evaluation() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.run_mcts_evaluation();
    let first = Arc::clone(&app.sim_job.as_ref().unwrap().cancel);
    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Char('m')));
    assert!(first.load(AtomicOrdering::Relaxed), "first run cancelled");
    let second = &app.sim_job.as_ref().expect("a new run started").cancel;
    assert!(!second.load(AtomicOrdering::Relaxed));
    app.cancel_evaluation();
}

/// Candidates that never got a playout show no numbers and sort after
/// every simulated one, whatever their placeholder values say.
#[test]
fn unsimulated_candidates_sort_last() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    let mut moves: Vec<Move> = (1..=3).map(fake_move).collect();
    for (i, m) in moves.iter_mut().enumerate() {
        m.row = i;
    }
    app.suggestions = moves.clone();
    let sim = |m: &Move, eq: f32, n: usize, status: SimStatus| SimResult {
        word: m.word.clone(),
        row: m.row,
        col: m.col,
        direction: m.direction,
        win_pct: 50.0,
        win_se: 0.0,
        sim_equity: eq,
        spread_se: 0.0,
        delta: 0.0,
        delta_se: 0.0,
        ply1_bingo_pct: 0.0,
        ply2_bingo_pct: 0.0,
        is_endgame: false,
        iterations: n,
        status,
    };
    // Floored, never simulated, and still below the floor.
    app.sim_results = Some(vec![
        sim(&moves[0], 5.0, 100, SimStatus::Racing),
        sim(&moves[1], 50.0, 0, SimStatus::Pending),
        sim(&moves[2], 10.0, 8, SimStatus::Pending),
    ]);
    app.set_sort_mode(SortMode::MctsEquity);
    let order: Vec<usize> = app.suggestions.iter().map(|m| m.row).collect();
    assert_eq!(order, vec![0, 2, 1]);
}

/// One click on the selected row only selects; a second click on the
/// same row within the double-click window applies it.
#[test]
fn applying_from_the_list_needs_a_real_double_click() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    app.suggestions = vec![fake_move(2), fake_move(3)];
    app.focus = Focus::Suggestions;
    app.selected_suggestion = 0;

    // A single click on the already-selected row does nothing more.
    app.handle_click(HitTarget::SuggestionRow(0));
    assert!(app.undo_stack.is_empty());
    // A slow second click is still a single click.
    app.last_row_click = Some((0, Instant::now() - Duration::from_secs(1)));
    app.handle_click(HitTarget::SuggestionRow(0));
    assert!(app.undo_stack.is_empty());
    // A click on another row selects it.
    app.handle_click(HitTarget::SuggestionRow(1));
    assert_eq!(app.selected_suggestion, 1);
    assert!(app.undo_stack.is_empty());
    // A quick second click on it applies.
    app.handle_click(HitTarget::SuggestionRow(1));
    assert_eq!(app.undo_stack.len(), 1, "double-click applied the move");
}

/// Tab skips panes the layout does not show, and hiding the sidebar
/// takes focus back to the board.
#[test]
fn focus_skips_hidden_panes() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    app.terminal_size = Some((200, 50));
    assert!(app.sidebar_visible);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Rack);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Suggestions);

    app.handle_key(key(KeyCode::Char('\\')));
    assert!(!app.sidebar_visible);
    assert_eq!(app.focus, Focus::Board, "focus left the hidden pane");
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Board, "nothing else is visible");

    // 80x24 keeps a compact sidebar beside a mini board: Tab reaches the
    // rack and the list.
    app.sidebar_visible = true;
    app.terminal_size = Some((80, 24));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Rack);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Suggestions);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Board);

    // Too narrow for a sidebar at all.
    app.terminal_size = Some((70, 24));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Board);
}

/// When the opponent's turn adds no tiles (a pass or an exchange), the
/// stored rack inference no longer applies and is cleared.
#[test]
fn opponent_pass_or_exchange_clears_inference() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.apply_suggestion();
    app.inferred_opponent_rack = Some(InferenceResult {
        played_tiles: "QI".to_string(),
        top_leaves: vec![],
        letter_marginals: vec![],
        total_racks: None,
    });
    let stamp = app.opp_move_stamp;
    // Our new rack, and no new tiles on the board.
    app.focus = Focus::Rack;
    type_keys(&mut app, "DEGILOU");
    app.find_moves();
    assert!(app.inferred_opponent_rack.is_none());
    assert_eq!(app.opp_move_stamp, stamp + 1);

    // A second solve with nothing new in between is not another turn.
    app.inferred_opponent_rack = Some(InferenceResult {
        played_tiles: "QI".to_string(),
        top_leaves: vec![],
        letter_marginals: vec![],
        total_racks: None,
    });
    app.find_moves();
    assert!(app.inferred_opponent_rack.is_some());
    app.cancel_evaluation();
}

/// A worker that panics is reported as a failed evaluation instead of
/// taking the app down.
#[test]
fn panicking_worker_reports_failure() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    let (job, tx, _live) = fake_sim_job(&app, vec![fake_move(2)]);
    app.sim_job = Some(job);
    spawn_worker(move || {
        let _keep = tx;
        panic!("simulated worker failure");
    });
    let started = Instant::now();
    while app.is_mcts_running() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
        app.poll_mcts();
    }
    assert!(!app.is_mcts_running());
    assert!(app.message.as_deref().unwrap_or("").contains("failed"));
}

/// Live snapshots update the table while the simulation runs: results
/// appear, the list re-sorts by simulated spread, and the selection
/// stays on the same move.
#[test]
fn live_snapshots_update_and_resort_the_table() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    let mut moves: Vec<Move> = (1..=3).map(fake_move).collect();
    for (i, m) in moves.iter_mut().enumerate() {
        m.row = i;
    }
    app.suggestions = moves.clone();
    app.selected_suggestion = 2; // the third move
    let (job, tx, live) = fake_sim_job(&app, moves.clone());
    app.sim_job = Some(job);

    *live.lock().unwrap() = Some(fake_snapshot(
        &[(1.0, 16), (9.0, 16), (5.0, 16)],
        /* done= */ false,
    ));
    assert!(app.poll_mcts());
    assert!(
        app.is_mcts_running(),
        "a live snapshot does not end the run"
    );
    assert_eq!(app.sort_mode, SortMode::MctsEquity);
    let order: Vec<usize> = app.suggestions.iter().map(|m| m.row).collect();
    assert_eq!(order, vec![1, 2, 0]);
    assert_eq!(app.selected_suggestion, 0, "first results start at the top");
    assert_eq!(app.sim_progress.map(|p| p.playouts), Some(48));

    // The user picks the second row; a later snapshot changes the order
    // again (the final one ends the run) and the selection follows the
    // move they picked.
    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.suggestions[app.selected_suggestion].row, 2);
    tx.send(fake_snapshot(
        &[(20.0, 32), (9.0, 32), (5.0, 32)],
        /* done= */ true,
    ))
    .unwrap();
    app.poll_mcts();
    assert!(!app.is_mcts_running());
    assert_eq!(app.suggestions[0].row, 0);
    assert_eq!(
        app.suggestions[app.selected_suggestion].row, 2,
        "selection kept"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap()
        .starts_with("Simulated 3/3 plays"));
}

/// Until the user moves the selection, a live simulation keeps it on the
/// leader as the order changes, so the top of the table stays in view
/// instead of following an early leader down the list.
#[test]
fn live_snapshots_keep_the_selection_on_the_leader_until_the_user_moves_it() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    let mut moves: Vec<Move> = (1..=3).map(fake_move).collect();
    for (i, m) in moves.iter_mut().enumerate() {
        m.row = i;
    }
    app.suggestions = moves.clone();
    let (job, tx, live) = fake_sim_job(&app, moves.clone());
    app.sim_job = Some(job);

    *live.lock().unwrap() = Some(fake_snapshot(
        &[(1.0, 16), (9.0, 16), (5.0, 16)],
        /* done= */ false,
    ));
    app.poll_mcts();
    assert_eq!(
        app.suggestions[app.selected_suggestion].row, 1,
        "the leader"
    );

    // The leader changes; nobody touched the list, so row 1 stays selected.
    *live.lock().unwrap() = Some(fake_snapshot(
        &[(20.0, 32), (9.0, 32), (5.0, 32)],
        /* done= */ false,
    ));
    app.poll_mcts();
    assert_eq!(app.selected_suggestion, 0);
    assert_eq!(app.suggestions[0].row, 0, "the new leader");

    // Once the user moves, their move stays selected as the order changes.
    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Down));
    let picked = app.suggestions[app.selected_suggestion].row;
    tx.send(fake_snapshot(
        &[(1.0, 48), (2.0, 48), (30.0, 48)],
        /* done= */ true,
    ))
    .unwrap();
    app.poll_mcts();
    assert_eq!(app.suggestions[app.selected_suggestion].row, picked);
}

/// Esc stops a running simulation and M continues it from the saved
/// state instead of starting over.
#[test]
fn esc_stops_and_m_resumes_the_simulation() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.focus = Focus::Suggestions;
    assert!(
        app.suggestions
            .iter()
            .any(|m| m.kind == crate::solver::MoveKind::Pass),
        "pass is a candidate"
    );
    app.handle_key(key(KeyCode::Char('m')));
    // Let it make some progress, then stop it.
    let started = Instant::now();
    while app.sim_progress.map_or(0, |p| p.playouts) == 0
        && started.elapsed() < Duration::from_secs(20)
    {
        std::thread::sleep(Duration::from_millis(20));
        app.poll_mcts();
    }
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.is_mcts_running());
    assert_eq!(
        app.dialog,
        DialogState::None,
        "Esc stopped the run, no menu"
    );
    // The worker saves its state when it notices the cancel.
    let started = Instant::now();
    let saved = loop {
        let saved = app.sim_cache.lock().unwrap().state().map(|s| s.playouts());
        if saved.is_some() || started.elapsed() > Duration::from_secs(20) {
            break saved.unwrap_or(0);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(saved > 0, "the stopped run saved its playouts");

    app.handle_key(key(KeyCode::Char('m')));
    finish_evaluation(&mut app);
    let total = app.sim_progress.map_or(0, |p| p.playouts);
    assert!(
        total > saved,
        "the resumed run continued from {saved} playouts (now {total})"
    );
}

#[test]
fn thousands_are_grouped() {
    assert_eq!(group_thousands(0), "0");
    assert_eq!(group_thousands(999), "999");
    assert_eq!(group_thousands(1000), "1,000");
    assert_eq!(group_thousands(48200), "48,200");
    assert_eq!(group_thousands(1234567), "1,234,567");
}

// ── Phase 5: navigation and input ───────────────────────────────

fn shift(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
}

/// Typing skips squares that already hold tiles, a matching letter
/// keeps the tile (and its blank flag), Shift+letter places a blank,
/// and the cursor stops at the edge.
#[test]
fn board_typing_skips_tiles_and_places_blanks() {
    let _lock = crate::datafiles::test_data::app_lock();
    let mut app = test_app();
    app.state = SolverState::from_cgp("15/15/15/15/15/15/15/3BeECH7/15/15/15/15/15/15/15").unwrap();
    app.focus = Focus::Board;

    // Typing the letter a tile already shows keeps the tile, blank
    // flag included, and moves on.
    app.cursor = (7, 4);
    type_keys(&mut app, "e");
    assert!(
        app.state.get_tile(7, 4).unwrap().is_blank,
        "blank flag kept"
    );
    assert_eq!(app.cursor, (7, 8), "jumped past the rest of the word");

    // Typing only the new tiles of a word fills the gaps: the cursor
    // jumps over the tiles already there.
    app.cursor = (7, 1);
    type_keys(&mut app, "a");
    assert_eq!(app.state.get_tile(7, 1).unwrap().letter, 'A');
    assert_eq!(app.cursor, (7, 2));
    type_keys(&mut app, "s");
    assert_eq!(app.cursor, (7, 8), "skipped the five tiles of BEECH");

    // Shift+letter places a blank.
    app.cursor = (0, 0);
    app.handle_key(shift('Q'));
    let q = app.state.get_tile(0, 0).unwrap();
    assert!(q.is_blank && q.letter == 'Q');

    // At the edge the cursor stays put.
    app.cursor = (0, 14);
    type_keys(&mut app, "z");
    assert_eq!(app.cursor, (0, 14));
    app.typing_direction = Direction::Vertical;
    app.cursor = (14, 0);
    type_keys(&mut app, "z");
    assert_eq!(app.cursor, (14, 0));
}

/// Shift+Tab goes backward, Enter on the board solves, releases are
/// ignored, and score entry opens from any pane.
#[test]
fn keymap_consistency() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.terminal_size = Some((200, 50));
    app.focus = Focus::Board;
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(app.focus, Focus::Suggestions);
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(app.focus, Focus::Rack);

    let mut release = key(KeyCode::Tab);
    release.kind = KeyEventKind::Release;
    assert!(matches!(app.handle_key(release), AppAction::Nothing));
    assert_eq!(app.focus, Focus::Rack);

    for focus in [Focus::Board, Focus::Rack, Focus::Suggestions] {
        app.focus = focus;
        app.handle_key(key(KeyCode::Char('=')));
        assert!(
            matches!(app.dialog, DialogState::ScoreInput { .. }),
            "{focus:?}"
        );
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.dialog, DialogState::None);
    }

    app.focus = Focus::Board;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Suggestions, "Enter on the board solves");
    assert!(!app.suggestions.is_empty());
    assert!(app.undo_stack.is_empty(), "nothing was applied");
    app.cancel_evaluation();

    // S cycles the sort like s.
    let before = app.sort_mode;
    app.handle_key(key(KeyCode::Char('S')));
    assert_ne!(app.sort_mode, before);
}

/// Esc first stops a simulation, then leaves a pane, then opens the
/// menu; Ctrl+C asks before quitting and a second Ctrl+C confirms.
#[test]
fn escape_order_and_quit_confirmation() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.focus = Focus::Suggestions;
    app.run_mcts_evaluation();
    assert!(app.is_mcts_running());
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.is_mcts_running());
    assert_eq!(app.focus, Focus::Suggestions, "first Esc only stopped it");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Board);
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.dialog, DialogState::EscapeMenu(_)));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.dialog, DialogState::None);

    assert!(matches!(app.handle_key(ctrl('c')), AppAction::Redraw));
    assert!(matches!(app.dialog, DialogState::ConfirmQuit { .. }));
    assert!(matches!(app.handle_key(ctrl('c')), AppAction::Quit));
}

/// List navigation: paging, Home/End, j/k, digits, and the filter.
#[test]
fn suggestion_list_navigation_and_filter() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.focus = Focus::Suggestions;
    app.list_height.set(10);
    let n = app.suggestions.len();
    assert!(n > 50);

    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(app.selected_suggestion, 9);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected_suggestion, 10);
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.selected_suggestion, 0);
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.selected_suggestion, 0, "paging stops at the top");
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.selected_suggestion, n - 1);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.selected_suggestion, 0);
    app.list_offset.set(4);
    app.handle_key(key(KeyCode::Char('3')));
    assert_eq!(app.selected_suggestion, 6, "row 3 of the window");

    // Filter by word: only matches are shown, n/N walk them.
    let word = app.suggestions[20].word.clone();
    app.list_offset.set(0);
    app.handle_key(key(KeyCode::Char('/')));
    assert!(app.filter_editing);
    type_keys(&mut app, &word.to_lowercase());
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.filter_editing);
    let shown = app.visible_suggestions();
    assert!(!shown.is_empty() && shown.len() < n);
    assert!(shown
        .iter()
        .all(|&i| app.suggestions[i].word.contains(&word)));
    assert_eq!(app.selected_suggestion, shown[0]);
    if shown.len() > 1 {
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.selected_suggestion, shown[1]);
        app.handle_key(key(KeyCode::Char('N')));
        assert_eq!(app.selected_suggestion, shown[0]);
    }
    // Filter by square.
    app.filter = Some("12".to_string());
    assert!(app
        .visible_suggestions()
        .iter()
        .all(|&i| app.suggestions[i].coords().starts_with("12")));
    // Esc drops the filter (and stays in the list).
    app.handle_key(key(KeyCode::Esc));
    assert!(app.filter.is_none());
    assert_eq!(app.focus, Focus::Suggestions);
    assert_eq!(app.visible_suggestions().len(), n);
}

/// Undo and redo walk the history both ways; reset and clearing the
/// rack can be undone; a new change drops the redo history.
#[test]
fn undo_redo_reset_and_rack_clear() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    let board_before = app.state.board_cgp();
    app.apply_suggestion();
    let board_after = app.state.board_cgp();
    assert_ne!(board_before, board_after);

    app.handle_key(ctrl('z'));
    assert_eq!(app.state.board_cgp(), board_before);
    app.handle_key(ctrl('y'));
    assert_eq!(app.state.board_cgp(), board_after);

    app.reset();
    assert!(app
        .state
        .board_cgp()
        .starts_with("15/15/15/15/15/15/15/15/"));
    app.undo();
    assert_eq!(app.state.board_cgp(), board_after, "reset undone");

    let rack = app.state.rack.tiles().len();
    if rack > 0 {
        app.clear_rack();
        assert!(app.state.rack.is_empty());
        app.undo();
        assert_eq!(app.state.rack.tiles().len(), rack, "rack clear undone");
    }

    app.undo();
    app.reset();
    app.handle_key(ctrl('y'));
    assert_eq!(
        app.message.as_deref(),
        Some("Nothing to redo"),
        "a new change dropped the redo history"
    );
}

/// After applying an exchange the rack has the focus, to type the
/// replacement tiles.
#[test]
fn exchange_focuses_the_rack() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    let i = app
        .suggestions
        .iter()
        .position(|m| matches!(m.kind, crate::solver::MoveKind::Exchange(_)))
        .expect("exchanges offered with a full bag");
    app.selected_suggestion = i;
    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Rack);
}

/// `:` then a square and a word previews the play and places it.
#[test]
fn quick_entry_validates_previews_and_places() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(dict) = crate::datafiles::test_data::dictionary() else {
        return;
    };
    let mut app = test_app();
    app.set_dictionary(dict);

    let play = app.parse_quick_entry("8D QUIXOTIC").unwrap();
    assert!(play.problem.is_none());
    assert_eq!(play.tiles.len(), 8);
    assert!(play.score > 0);
    assert!(app
        .parse_quick_entry("8D QUIXOTICX")
        .unwrap()
        .problem
        .is_some());
    assert!(app
        .parse_quick_entry("1A QUIXOTIC")
        .unwrap()
        .problem
        .unwrap()
        .contains("centre"));
    assert!(
        app.parse_quick_entry("8M QUIXOTIC").is_err(),
        "off the board"
    );
    assert!(app.parse_quick_entry("Z9 QI").is_err(), "not a square");

    app.handle_key(key(KeyCode::Char(':')));
    type_keys(&mut app, "8D QUIXOTIc");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.dialog, DialogState::None);
    assert_eq!(app.state.get_tile(7, 3).unwrap().letter, 'Q');
    assert!(
        app.state.get_tile(7, 10).unwrap().is_blank,
        "lowercase = blank"
    );

    // Through existing tiles, down: G7 AX uses the X of QUIXOTIC.
    let down = app.parse_quick_entry("G7 AX").unwrap();
    assert_eq!(down.tiles.len(), 1);
    assert!(down.problem.is_none(), "{:?}", down.problem);
    assert!(app.parse_quick_entry("G7 AZ").is_err(), "G8 holds X");
}

/// Copying puts the move or the position on the clipboard; pasting a
/// CGP loads it (undoably); the session text round-trips.
#[test]
fn clipboard_paste_and_session_round_trip() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Char('y')));
    let copied = app.pending_clipboard.take().unwrap();
    let mv = &app.suggestions[app.selected_suggestion];
    assert!(copied.starts_with(&mv.coords()), "{copied}");

    app.our_score = 120;
    app.opp_score = 95;
    app.final_turn = FinalTurn::Us;
    app.final_turn_known = true;
    app.handle_key(key(KeyCode::Char('Y')));
    let cgp = app.pending_clipboard.take().unwrap();
    assert!(cgp.contains(" 120/95 0"), "{cgp}");

    let mut other = test_app();
    other.load_position(&cgp, /* undoable= */ false).unwrap();
    assert_eq!(other.state.board_cgp(), app.state.board_cgp());
    assert_eq!((other.our_score, other.opp_score), (120, 95));
    assert_eq!(other.final_turn, FinalTurn::Us);
    assert!(other.final_turn_known);

    // A pasted plain CGP replaces the position and can be undone.
    let before = app.state.board_cgp();
    app.handle_paste("15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 AEINRST/ 10/20 0");
    assert!(app
        .state
        .board_cgp()
        .starts_with("15/15/15/15/15/15/15/4CAT8"));
    assert_eq!((app.our_score, app.opp_score), (10, 20));
    app.undo();
    assert_eq!(app.state.board_cgp(), before);
    app.handle_paste("not a position");
    assert_eq!(app.message_kind, MessageKind::Error);
}

#[test]
fn base64_matches_known_vectors() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64(b"8D QUIXOTIC"), "OEQgUVVJWE9USUM=");
}

// ── Saved games ────────────────────────────────────────────────────

use crate::games::{self, OppLosses, SavedGame};

/// A test App that saves its games in `dir` (a temporary directory).
fn games_app(dir: &std::path::Path) -> App {
    let mut app = test_app();
    app.games_dir = Some(dir.to_path_buf());
    app
}

/// Store a game in `dir` directly, as another window would have.
fn store_game(dir: &std::path::Path, name: &str, updated: u64) {
    let game = SavedGame {
        name: name.to_string(),
        cgp: "15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 EINRST/ 12/8 0".to_string(),
        updated,
        opp_losses: OppLosses::default(),
        record: None,
    };
    games::save(dir, &game).unwrap();
}

/// The files in `dir`, sorted, for checking what was written.
fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|r| {
            r.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A new game is not saved while empty, gets an automatic name on its
/// first save, and reopens with its position, scores, final turn and
/// opponent measurement; the lock goes away on exit.
#[test]
fn saved_game_round_trip_with_automatic_name() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-roundtrip");
    let mut app = games_app(&dir);
    assert!(!app.autosave(), "nothing entered yet");
    assert!(files_in(&dir).is_empty());

    app.load_position(
        "15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 AEINRST/ 120/95 0",
        false,
    )
    .unwrap();
    app.final_turn = FinalTurn::Us;
    app.final_turn_known = true;
    app.opp_loss_total = 31.5;
    app.opp_moves_measured = 3;
    assert!(app.autosave());
    let name = app.game_name.clone().unwrap();
    assert!(games::is_automatic_name(&name), "{name}");
    let slug = games::slug(&name);
    assert_eq!(
        files_in(&dir),
        [format!("{slug}.lock"), format!("{slug}.toml")]
    );
    app.shutdown();
    assert_eq!(files_in(&dir), [format!("{slug}.toml")], "lock released");

    let mut other = games_app(&dir);
    other.state.add_to_rack('Q');
    other.push_undo("something".to_string());
    other.open_game(&slug).unwrap();
    assert_eq!(other.game_name.as_deref(), Some(name.as_str()));
    assert_eq!(other.state.board_cgp(), app.state.board_cgp());
    assert_eq!((other.our_score, other.opp_score), (120, 95));
    assert_eq!(other.final_turn, FinalTurn::Us);
    assert!(other.final_turn_known);
    assert_eq!(other.opp_moves_measured, 3);
    assert_eq!(other.opp_loss_total, 31.5);
    assert_eq!(other.opp_temperature(), app.opp_temperature());
    assert!(
        other.undo_stack.is_empty(),
        "the old game's history is gone"
    );
    assert!(!other.game_dirty);
    other.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two windows cannot open the same game; a lock left by a process that
/// is gone is taken over.
#[test]
fn a_game_opens_in_one_window_only() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-lock");
    let mut first = games_app(&dir);
    first.start_session(Some("Alice")).unwrap();
    assert_eq!(first.game_name.as_deref(), Some("Alice"));
    first.autosave();

    let mut second = games_app(&dir);
    let err = second.open_game("alice").unwrap_err();
    assert_eq!(err, "Alice is open in another window");
    assert!(second.start_session(Some("alice")).is_err());
    assert_eq!(second.game_name, None);

    // Picker: dimmed and refused.
    second.show_game_picker(/* startup= */ false);
    assert!(second.picker_games[0].locked);
    second.handle_key(key(KeyCode::Enter));
    assert!(matches!(second.dialog, DialogState::GamePicker { .. }));
    assert_eq!(second.message_kind, MessageKind::Error);
    second.close_dialog();

    // A stale lock is taken over.
    store_game(&dir, "Bob", 5);
    std::fs::write(
        games::lock_path(&dir, "bob"),
        format!("{}\n", games::dead_pid()),
    )
    .unwrap();
    second.open_game("bob").unwrap();
    assert_eq!(second.game_name.as_deref(), Some("Bob"));
    assert_eq!(games::lock_holder(&dir, "bob"), Some(std::process::id()));

    // Once the first window closes, the game can be opened elsewhere.
    first.shutdown();
    second.open_game("alice").unwrap();
    assert!(
        !games::lock_path(&dir, "bob").exists(),
        "switching games releases the old lock"
    );
    second.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save As from the menu renames the game: the file and the lock move,
/// and the first key typed replaces an automatic name.
#[test]
fn save_as_renames_the_file_and_lock() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-rename");
    let mut app = games_app(&dir);
    app.state.add_to_rack('Q');
    app.autosave();
    let old = app.game_slug().unwrap();

    // Esc opens the menu on Cancel; four rows up is Save Game As.
    app.handle_key(key(KeyCode::Esc));
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Up));
    }
    assert_eq!(
        app.dialog,
        DialogState::EscapeMenu(EscapeMenuOption::SaveAs)
    );
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        app.dialog,
        DialogState::SaveAs { pristine: true, .. }
    ));
    type_keys(&mut app, "Alice Smith");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.dialog, DialogState::None);
    assert_eq!(app.game_name.as_deref(), Some("Alice Smith"));
    assert_eq!(files_in(&dir), ["alice-smith.lock", "alice-smith.toml"]);
    assert!(!files_in(&dir).contains(&format!("{old}.toml")));
    assert_eq!(
        games::load(&dir, "alice-smith").unwrap().name,
        "Alice Smith"
    );

    // Renaming to a name with the same slug only changes the display name.
    app.show_save_as(SaveAsPurpose::Save);
    app.handle_key(ctrl('u'));
    type_keys(&mut app, "alice smith");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(files_in(&dir), ["alice-smith.lock", "alice-smith.toml"]);
    assert_eq!(
        games::load(&dir, "alice-smith").unwrap().name,
        "alice smith"
    );
    app.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Saving under another saved game's name asks first; No returns to the
/// prompt, Yes replaces that game.
#[test]
fn save_as_over_another_game_asks_first() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-overwrite");
    store_game(&dir, "Bob", 5);
    let mut app = games_app(&dir);
    app.start_session(Some("Alice")).unwrap();
    // The picker was not shown: --game was given.
    assert_eq!(app.dialog, DialogState::None);
    app.state.add_to_rack('Z');

    app.show_save_as(SaveAsPurpose::Save);
    app.handle_key(ctrl('u'));
    type_keys(&mut app, "BOB");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.dialog, DialogState::ConfirmOverwrite { .. }));
    app.handle_key(key(KeyCode::Char('n')));
    assert!(matches!(
        &app.dialog,
        DialogState::SaveAs { buffer, .. } if buffer == "BOB"
    ));
    assert_eq!(app.game_name.as_deref(), Some("Alice"));

    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(app.dialog, DialogState::None);
    assert_eq!(app.game_name.as_deref(), Some("BOB"));
    assert_eq!(files_in(&dir), ["bob.lock", "bob.toml"]);
    assert!(games::load(&dir, "bob").unwrap().cgp.contains(" Z/ "));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Quitting an untitled game asks for a name: Enter saves it and quits,
/// a second Ctrl+C quits keeping the automatic name, Esc cancels. A named
/// game quits as before.
#[test]
fn quitting_an_untitled_game_offers_a_name() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-quit");
    let mut app = games_app(&dir);
    app.state.add_to_rack('Q');

    assert!(matches!(app.handle_key(ctrl('c')), AppAction::Redraw));
    assert!(matches!(
        app.dialog,
        DialogState::SaveAs {
            purpose: SaveAsPurpose::Quit,
            ..
        }
    ));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.dialog, DialogState::None, "Esc cancels the quit");
    assert!(!app.should_quit);

    // Ctrl+C twice: quit, keeping the automatic name.
    app.handle_key(ctrl('c'));
    assert!(matches!(app.handle_key(ctrl('c')), AppAction::Quit));
    let untitled = app.game_name.clone().unwrap();
    assert!(games::is_automatic_name(&untitled));

    // Enter with the name unchanged also keeps it.
    app.request_quit();
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        AppAction::Quit
    ));
    assert_eq!(app.game_name.as_deref(), Some(untitled.as_str()));
    app.should_quit = false;

    // The menu's Quit names it too; a typed name is saved, then quit.
    app.show_escape_menu();
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.dialog, DialogState::SaveAs { .. }));
    type_keys(&mut app, "Carol");
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        AppAction::Quit
    ));
    assert_eq!(app.game_name.as_deref(), Some("Carol"));
    assert!(games::exists(&dir, "carol"));
    assert!(!games::exists(&dir, &games::slug(&untitled)));

    // Named: the usual confirmation.
    app.should_quit = false;
    app.handle_key(ctrl('c'));
    assert_eq!(app.dialog, DialogState::ConfirmQuit { yes: false });
    app.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The picker lists games newest first; it opens, starts, renames and
/// deletes games, and refuses games open in another window.
#[test]
fn game_picker_keys() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-picker");
    store_game(&dir, "Ann", 10);
    store_game(&dir, "Bea", 30);
    store_game(&dir, "Cy", 20);
    // Pid 1 always runs: Cy is open "in another window".
    std::fs::write(games::lock_path(&dir, "cy"), "1\n").unwrap();

    let mut app = games_app(&dir);
    app.start_session(None).unwrap();
    assert_eq!(
        app.dialog,
        DialogState::GamePicker {
            selected: 0,
            startup: true
        },
        "shown at startup"
    );
    let names: Vec<&str> = app.picker_games.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Bea", "Cy", "Ann"]);
    assert_eq!(app.picker_games[0].score_text(), "12\u{2013}8");
    assert!(app.picker_games[1].locked);

    // The locked game can be neither opened nor deleted.
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        app.dialog,
        DialogState::GamePicker { selected: 1, .. }
    ));
    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.dialog, DialogState::GamePicker { .. }));
    assert_eq!(app.message_kind, MessageKind::Error);

    // Delete asks; No keeps it, Yes deletes it.
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.dialog, DialogState::ConfirmDelete { .. }));
    app.handle_key(key(KeyCode::Char('n')));
    assert!(matches!(
        app.dialog,
        DialogState::GamePicker { selected: 2, .. }
    ));
    assert!(games::exists(&dir, "ann"));
    app.handle_key(key(KeyCode::Char('d')));
    app.handle_key(key(KeyCode::Char('y')));
    assert!(!games::exists(&dir, "ann"));
    assert_eq!(app.picker_games.len(), 2);

    // Rename from the picker, then back to the picker on the new name.
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Char('r')));
    app.handle_key(ctrl('u'));
    type_keys(&mut app, "Beatrice");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        app.dialog,
        DialogState::GamePicker { selected: 0, .. }
    ));
    assert_eq!(app.picker_games[0].name, "Beatrice");
    assert!(!games::exists(&dir, "bea"));

    // Enter opens it.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.dialog, DialogState::None);
    assert_eq!(app.game_name.as_deref(), Some("Beatrice"));
    assert_eq!(app.our_score, 12);

    // The open game cannot be deleted from its own window.
    app.handle_key(ctrl('o'));
    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.dialog, DialogState::GamePicker { .. }));
    assert!(games::exists(&dir, "beatrice"));

    // n starts a new, untitled game and releases the old one.
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.dialog, DialogState::None);
    assert_eq!(app.game_name, None);
    assert_eq!(app.our_score, 0);
    assert!(!games::lock_path(&dir, "beatrice").exists());

    // Esc closes it.
    app.handle_key(ctrl('o'));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.dialog, DialogState::None);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Switching games drops what belonged to the old one: undo history,
/// inference, results and the simulation resume cache.
#[test]
fn switching_games_clears_the_old_games_state() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    let dir = games::test_dir("app-switch");
    app.games_dir = Some(dir.clone());
    store_game(&dir, "Dee", 1);
    app.push_undo("before".to_string());
    app.inferred_opponent_rack = Some(InferenceResult {
        played_tiles: "AB".to_string(),
        top_leaves: Vec::new(),
        letter_marginals: Vec::new(),
        total_racks: None,
    });
    app.opp_moves_measured = 4;
    app.opp_loss_total = 40.0;
    app.run_mcts_evaluation();
    app.open_game("dee").unwrap();
    assert!(!app.is_mcts_running());
    assert!(app.undo_stack.is_empty() && app.redo_stack.is_empty());
    assert!(app.inferred_opponent_rack.is_none());
    assert!(app.suggestions.is_empty() && app.sim_results.is_none());
    // The cancelled worker stores its run on the way out; that must not
    // land in the new game's cache. Its clone of the cache is dropped when
    // it finishes, so wait for that rather than racing it.
    let started = Instant::now();
    while Arc::strong_count(&app.sim_cache) > 1 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the cancelled simulation worker never finished"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.sim_cache.lock().unwrap().state().is_none(),
        "the old game's simulation run reached the new game's cache"
    );
    assert_eq!(app.opp_moves_measured, 0);
    // The game left behind was saved with its own measurement.
    let saved: Vec<SavedGame> = files_in(&dir)
        .iter()
        .filter(|f| f.ends_with(".toml") && *f != "dee.toml")
        .map(|f| games::load(&dir, f.trim_end_matches(".toml")).unwrap())
        .collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].opp_losses.moves, 4);
    app.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The old single session file is imported once at startup and offered
/// in the picker.
#[test]
fn old_session_is_imported_at_startup() {
    let _lock = crate::datafiles::test_data::app_lock();
    let base = games::test_dir("app-migrate");
    std::fs::write(
        base.join("session.cgp"),
        "15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 AEINRST/ 10/20 0\n",
    )
    .unwrap();
    let mut app = games_app(&base.join("games"));
    app.start_session(None).unwrap();
    assert!(matches!(app.dialog, DialogState::GamePicker { .. }));
    assert_eq!(app.picker_games[0].name, games::RESTORED_NAME);
    assert_eq!(app.picker_games[0].scores, Some((10, 20)));
    assert!(base.join("session.cgp.imported").exists());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!((app.our_score, app.opp_score), (10, 20));
    app.shutdown();
    let _ = std::fs::remove_dir_all(&base);
}

/// Edits mark the game for saving; saves happen at most once per
/// interval, and always on exit.
#[test]
fn autosave_is_throttled_and_runs_on_exit() {
    let _lock = crate::datafiles::test_data::app_lock();
    let dir = games::test_dir("app-throttle");
    let mut app = games_app(&dir);
    app.start_session(Some("Eve")).unwrap();
    let t0 = Instant::now();
    let rack = |dir: &std::path::Path| {
        let cgp = games::load(dir, "eve").unwrap().cgp;
        cgp.split_whitespace().nth(1).unwrap().to_string()
    };

    app.focus = Focus::Rack;
    type_keys(&mut app, "a");
    assert_eq!(app.autosave_due_in(t0), Some(Duration::ZERO));
    assert!(app.autosave_if_due(t0));
    assert_eq!(rack(&dir), "A/");
    assert_eq!(app.autosave_due_in(t0), None, "nothing new to save");

    type_keys(&mut app, "b");
    assert!(!app.autosave_if_due(t0 + Duration::from_millis(500)));
    assert_eq!(rack(&dir), "A/", "throttled");
    assert!(app.autosave_if_due(t0 + AUTOSAVE_INTERVAL + Duration::from_millis(10)));
    assert_eq!(rack(&dir), "AB/");

    // Score edits and undo count as changes too.
    app.handle_key(key(KeyCode::Char('+')));
    type_keys(&mut app, "42");
    app.handle_key(key(KeyCode::Enter));
    assert!(app.autosave_due_in(t0).is_some());

    // Exit saves whatever is pending, at once.
    type_keys(&mut app, "c");
    app.shutdown();
    assert_eq!(rack(&dir), "ABC/");
    assert!(games::load(&dir, "eve").unwrap().cgp.contains(" 42/0 "));
    assert!(!games::lock_path(&dir, "eve").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The filter matches what the string version did: a word containing the
/// query, or coordinates starting with it, ignoring case.
#[test]
fn filter_matches_words_and_coordinates() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(mut app) = solved_app() else {
        return;
    };
    if let Some(job) = app.sim_job.take() {
        job.cancel.store(true, AtomicOrdering::Relaxed);
    }
    for query in ["", "e", "RE", "ch", "8", "8h", "H8", "h1", "12", "-", "zz"] {
        app.filter = Some(query.to_string());
        let q = query.to_ascii_uppercase();
        let expected: Vec<usize> = if query.is_empty() {
            (0..app.suggestions.len()).collect()
        } else {
            app.suggestions
                .iter()
                .enumerate()
                .filter(|(_, m)| {
                    m.word.to_ascii_uppercase().contains(&q)
                        || m.coords().to_ascii_uppercase().starts_with(&q)
                })
                .map(|(i, _)| i)
                .collect()
        };
        assert_eq!(app.visible_suggestions(), expected, "query {query:?}");
    }
}

/// With one tile in the bag, M solves every play exactly over the
/// possible last tiles instead of simulating: results arrive marked exact,
/// and the best play and the pass are solved.
#[test]
fn one_tile_in_the_bag_is_solved_exactly() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(dict) = crate::datafiles::test_data::dictionary() else {
        return;
    };
    // Greedy self-play until the side to move faces one tile in the bag.
    let position = (0..200u64).find_map(|seed| {
        let mut rng = fastrand::Rng::with_seed(seed);
        let mut bag = crate::rules::CROSSPLAY.full_tile_set();
        rng.shuffle(&mut bag);
        let mut racks = [bag.split_off(bag.len() - 7), bag.split_off(bag.len() - 7)];
        let mut board = crate::board::Board::new();
        let mut to_move = 0;
        while bag.len() > 1 {
            let mv =
                crate::solver::Solver::new(&board, &dict).best_move_by_equity(&racks[to_move])?;
            for (r, c, t) in &mv.tiles_used {
                board.place_tile(*r, *c, *t);
            }
            for t in mv.tiles_from_rack() {
                let i = racks[to_move]
                    .iter()
                    .position(|x| x.is_blank == t.is_blank && (t.is_blank || x.letter == t.letter))
                    .unwrap();
                racks[to_move].swap_remove(i);
            }
            let draw = mv.tiles_used.len().min(bag.len());
            racks[to_move].extend(bag.split_off(bag.len() - draw));
            to_move = 1 - to_move;
        }
        (bag.len() == 1).then(|| (board, racks[to_move].clone()))
    });
    let Some((board, rack)) = position else {
        return;
    };
    let mut app = test_app();
    app.set_dictionary(dict);
    app.state = SolverState::new();
    app.state.board = board;
    for t in &rack {
        assert!(app
            .state
            .add_to_rack(if t.is_blank { '?' } else { t.letter }));
    }
    assert_eq!(app.state.bag_size(), 1);
    app.find_moves();
    if let Some(job) = app.sim_job.take() {
        job.cancel.store(true, AtomicOrdering::Relaxed);
    }
    app.run_mcts_evaluation();
    assert!(
        app.sim_job.is_none(),
        "no simulation with one tile in the bag"
    );
    finish_evaluation(&mut app);
    let results = app.sim_results.as_ref().expect("exact results");
    let top = &app.suggestions[0];
    let first = results
        .iter()
        .find(|r| r.word == top.word && r.row == top.row && r.col == top.col)
        .expect("the top play is solved");
    assert_eq!(first.status, SimStatus::Exact);
    let pass = results
        .iter()
        .find(|r| r.word == crate::solver::PASS_WORD)
        .expect("the pass is solved");
    assert_eq!(pass.status, SimStatus::Exact);
}

/// Our moves and the opponent's are recorded as we play; `G` copies the
/// record as GCG, and pasting it back imports the game with the side to
/// move as us. Undo takes the import back.
#[test]
fn moves_are_recorded_exported_and_imported() {
    let _lock = crate::datafiles::test_data::app_lock();
    let Some(dict) = crate::datafiles::test_data::dictionary() else {
        return;
    };
    let mut app = test_app();
    app.dictionary = Some(Arc::new(dict));
    for ch in "AEINRST".chars() {
        app.state.add_to_rack(ch);
    }
    app.find_moves();
    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Char('G')));
    assert!(app.pending_clipboard.is_none(), "nothing recorded yet");

    let ours = app.suggestions[0].clone();
    app.apply_suggestion();
    // The opponent plays CAT in the corner, then a pass.
    app.state.place_tile(0, 0, Tile::new('C'));
    app.state.place_tile(0, 1, Tile::new('A'));
    app.state.place_tile(0, 2, Tile::new('T'));
    for ch in "EEIOUSL".chars() {
        app.state.add_to_rack(ch);
    }
    app.find_moves();
    app.apply_suggestion();
    app.clear_rack();
    for ch in "DGRTWYZ".chars() {
        app.state.add_to_rack(ch);
    }
    app.find_moves();
    assert_eq!(app.record.game.events.len(), 4, "{:?}", app.record.game);

    app.focus = Focus::Suggestions;
    app.handle_key(key(KeyCode::Char('G')));
    let gcg = app.pending_clipboard.take().expect("the record is copied");
    let first = format!(
        ">me: AEINRST {} {} +{} {}",
        ravyn::gcg::Placement::of_move(&ours).unwrap().position(),
        ours.word,
        ours.score,
        ours.score
    );
    assert!(gcg.contains(&first), "{gcg}");
    assert!(gcg.contains(">opp: 1A CAT +"), "{gcg}");
    assert!(gcg.contains(">opp: - +0"), "{gcg}");
    let parsed = ravyn::gcg::Game::parse(&gcg).unwrap();
    assert_eq!(parsed.events.len(), 4);

    // A fresh app imports it: we are to move, with the same board and scores.
    let (board, ours_now, theirs_now) = (app.state.board_cgp(), app.our_score, app.opp_score);
    let mut other = test_app();
    other.handle_paste(&gcg);
    assert_eq!(other.state.board_cgp(), board);
    assert_eq!((other.our_score, other.opp_score), (ours_now, theirs_now));
    assert_eq!(other.record.us, 0);
    assert!(other.state.rack.is_empty());
    other.undo();
    assert_eq!((other.our_score, other.opp_score), (0, 0));
    assert!(other.record.is_empty());
}
