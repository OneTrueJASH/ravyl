//! Central application state, event handling, and business logic.
//!
//! Contains the [`App`] struct which owns all game state, dialog state,
//! and coordinates between the solver, MCTS engine, and UI.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::board::{Board, Direction, BOARD_SIZE};
use crate::dictionary::Dictionary;
use crate::endgame::{EndgameMove, EndgameSolver};
use crate::game::SolverState;
use crate::logging::log;
use crate::refengine_mcts::{InferenceResult, RefSimResult, RefSimulator, SimulationOutput};
use crate::mcts::{MctsConfig, MctsEvaluator, MctsResult};
use crate::settings::{self, Settings};
use crate::solver::{Move, Solver};
use crate::tile::Tile;
use crate::ui::events::AppAction;
use crate::ui::hit::{resolve_click, HitTarget};
use crate::ui::menu_popup::EscapeMenuOption;

/// Which player's score is being edited
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreTarget {
    Player,
    Opponent,
}

/// Dialog state for popup interactions
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DialogState {
    #[default]
    None,
    Help,
    EscapeMenu(EscapeMenuOption),
    ConfirmQuit(bool),
    ConfirmReset(bool),
    /// Numeric input for setting a player's cumulative score
    ScoreInput {
        target: ScoreTarget,
        buffer: String,
    },
}

/// Board pane tab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardTab {
    #[default]
    Board,
    Logs,
}

impl BoardTab {
    pub fn toggle(&self) -> Self {
        match self {
            BoardTab::Board => BoardTab::Logs,
            BoardTab::Logs => BoardTab::Board,
        }
    }
}

/// Input focus area
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Board,
    Rack,
    Suggestions,
}

impl Focus {
    pub fn next(&self) -> Focus {
        match self {
            Focus::Board => Focus::Rack,
            Focus::Rack => Focus::Suggestions,
            Focus::Suggestions => Focus::Board,
        }
    }
}

/// Sort mode for suggestions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    #[default]
    Equity,
    MctsEquity,
    WinPct,
    Score,
    Leave,
    WordLength,
    Alpha,
}

impl SortMode {
    pub fn next(&self, has_mcts: bool) -> SortMode {
        match self {
            SortMode::Equity => {
                if has_mcts {
                    SortMode::MctsEquity
                } else {
                    SortMode::Score
                }
            }
            SortMode::MctsEquity => SortMode::WinPct,
            SortMode::WinPct => SortMode::Score,
            SortMode::Score => SortMode::Leave,
            SortMode::Leave => SortMode::WordLength,
            SortMode::WordLength => SortMode::Alpha,
            SortMode::Alpha => SortMode::Equity,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SortMode::Equity => "Equity",
            SortMode::MctsEquity => "RefEngine",
            SortMode::WinPct => "Win%",
            SortMode::Score => "Score",
            SortMode::Leave => "Leave",
            SortMode::WordLength => "Length",
            SortMode::Alpha => "A-Z",
        }
    }
}

/// Saved state for undo
#[derive(Clone)]
struct UndoState {
    state: SolverState,
    our_score: u32,
    opp_score: u32,
    message: String,
}

/// MCTS progress state
pub struct MctsProgress {
    pub total_candidates: usize,
    pub spinner_frame: usize,
}

impl MctsProgress {
    /// Returns the spinner character for the current animation frame.
    pub fn spinner_char(&self) -> char {
        const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
        SPINNER[self.spinner_frame % SPINNER.len()]
    }
}

/// Application state
pub struct App {
    pub state: SolverState,
    pub dictionary: Option<Arc<Dictionary>>,
    pub cursor: (usize, usize),
    pub focus: Focus,
    pub typing_direction: Direction,
    pub suggestions: Vec<Move>,
    pub selected_suggestion: usize,
    pub sort_mode: SortMode,
    pub message: Option<String>,
    undo_stack: Vec<UndoState>,
    pub mcts_results: Option<Vec<MctsResult>>,
    pub refengine_results: Option<Vec<RefSimResult>>,
    mcts_receiver: Option<Receiver<Vec<MctsResult>>>,
    refengine_receiver: Option<Receiver<Result<SimulationOutput, String>>>,
    endgame_receiver: Option<Receiver<Vec<EndgameMove>>>,
    pub mcts_progress: Option<MctsProgress>,
    refengine_simulator: Option<Arc<RefSimulator>>,
    pub dialog: DialogState,
    pub should_quit: bool,
    pub board_tab: BoardTab,
    pub log_scroll: usize,
    pub sidebar_visible: bool,
    pub inferred_opponent_rack: Option<InferenceResult>,
    last_applied_board: Option<Board>,
    /// Opponent tiles detected during auto-scoring in find_moves(), saved for
    /// RefEngine inference which runs after the board snapshot is updated.
    pending_opponent_tiles: Option<String>,
    /// Board state before the opponent played, saved alongside pending_opponent_tiles.
    pre_opponent_board: Option<Board>,
    pub settings: Settings,
    pub our_score: u32,
    pub opp_score: u32,
}

impl App {
    /// Create a new App with default settings loaded from disk.
    pub fn new() -> Self {
        Self::with_settings(Settings::load())
    }

    /// Create a new App with the given settings.
    pub fn with_settings(settings: Settings) -> Self {
        let refengine_simulator = match RefSimulator::new("external/refengine/data", settings::LEXICON)
        {
            Ok(sim) => Some(Arc::new(sim)),
            Err(e) => {
                eprintln!("RefEngine init failed: {}", e);
                None
            }
        };

        let sidebar_visible = settings.display.sidebar_visible;
        let sort_mode = settings.display.default_sort.to_sort_mode();

        App {
            state: SolverState::new(),
            dictionary: None,
            cursor: (7, 7),
            focus: Focus::Board,
            typing_direction: Direction::Horizontal,
            suggestions: Vec::new(),
            selected_suggestion: 0,
            sort_mode,
            message: Some("Loading dictionary...".to_string()),
            undo_stack: Vec::new(),
            mcts_results: None,
            refengine_results: None,
            mcts_receiver: None,
            refengine_receiver: None,
            endgame_receiver: None,
            mcts_progress: None,
            refengine_simulator,
            inferred_opponent_rack: None,
            last_applied_board: None,
            pending_opponent_tiles: None,
            pre_opponent_board: None,
            dialog: DialogState::None,
            should_quit: false,
            board_tab: BoardTab::default(),
            log_scroll: 0,
            sidebar_visible,
            settings,
            our_score: 0,
            opp_score: 0,
        }
    }

    // ── Event handlers ──────────────────────────────────────────────

    /// Handle a keyboard event and return the action for the main loop.
    pub fn handle_key(&mut self, key: KeyEvent) -> AppAction {
        // Ctrl+C always quits.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return AppAction::Quit;
        }

        // F1 toggles help (unless another dialog is open).
        if key.code == KeyCode::F(1) && !self.has_dialog() {
            self.toggle_help();
            return AppAction::Redraw;
        }

        // Backslash toggles sidebar.
        if key.code == KeyCode::Char('\\') && !self.has_dialog() {
            self.toggle_sidebar();
            return AppAction::Redraw;
        }

        // Dialog input.
        if self.has_dialog() {
            self.handle_dialog_key(key);
            if self.should_quit {
                return AppAction::Quit;
            }
            return AppAction::Redraw;
        }

        // Focus-specific input.
        match self.focus {
            Focus::Board => self.handle_board_key(key),
            Focus::Rack => self.handle_rack_key(key),
            Focus::Suggestions => self.handle_suggestions_key(key),
        }

        AppAction::Redraw
    }

    /// Handle a mouse event and return the action for the main loop.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> AppAction {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(target) = resolve_click(mouse.column, mouse.row) {
                    self.handle_click(target)
                } else {
                    AppAction::Nothing
                }
            }
            MouseEventKind::ScrollUp => {
                match self.focus {
                    Focus::Suggestions => self.move_suggestion_selection(-3),
                    Focus::Board if self.board_tab == BoardTab::Logs => {
                        self.scroll_log_up(3);
                    }
                    _ => {}
                }
                AppAction::Redraw
            }
            MouseEventKind::ScrollDown => {
                match self.focus {
                    Focus::Suggestions => self.move_suggestion_selection(3),
                    Focus::Board if self.board_tab == BoardTab::Logs => {
                        self.scroll_log_down(3);
                    }
                    _ => {}
                }
                AppAction::Redraw
            }
            _ => AppAction::Nothing,
        }
    }

    /// Handle a resolved click target.
    fn handle_click(&mut self, target: HitTarget) -> AppAction {
        match target {
            HitTarget::BoardCell(row, col) => {
                self.focus = Focus::Board;
                self.board_tab = BoardTab::Board;
                self.cursor = (row, col);
            }
            HitTarget::RackSlot(_) => {
                self.focus = Focus::Rack;
            }
            HitTarget::SuggestionRow(idx) => {
                if self.focus == Focus::Suggestions && self.selected_suggestion == idx {
                    // Double-click on same row: apply the move.
                    self.apply_suggestion_and_focus();
                } else {
                    self.focus = Focus::Suggestions;
                    self.selected_suggestion = idx.min(self.suggestions.len().saturating_sub(1));
                }
            }
            HitTarget::TabHeader(tab) => {
                self.board_tab = tab;
                self.focus = Focus::Board;
                if tab == BoardTab::Logs {
                    self.log_scroll = 0;
                }
            }
            HitTarget::SortTab(mode) => {
                self.focus = Focus::Suggestions;
                self.set_sort_mode(mode);
            }
            HitTarget::FocusPane(focus) => {
                self.focus = focus;
            }
            HitTarget::StatusBar => {}
        }
        AppAction::Redraw
    }

    // ── Focus-specific key handlers ─────────────────────────────────

    fn handle_board_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if key.code == KeyCode::Char('`') {
            self.toggle_board_tab();
            return;
        }

        match self.board_tab {
            BoardTab::Logs => match key.code {
                KeyCode::Up => self.scroll_log_up(1),
                KeyCode::Down => self.scroll_log_down(1),
                KeyCode::PageUp => self.scroll_log_up(20),
                KeyCode::PageDown => self.scroll_log_down(20),
                KeyCode::Home => self.scroll_log_up(10000),
                KeyCode::End => self.scroll_log_to_bottom(),
                KeyCode::Tab => self.next_focus(),
                KeyCode::Esc => self.show_escape_menu(),
                KeyCode::Char('s') if ctrl => self.find_moves_and_focus(),
                KeyCode::Char('r') if ctrl => self.show_reset_confirm(),
                KeyCode::Char('z') if ctrl => self.undo(),
                _ => {}
            },
            BoardTab::Board => match key.code {
                KeyCode::Up => self.move_cursor(-1, 0),
                KeyCode::Down => self.move_cursor(1, 0),
                KeyCode::Left => self.move_cursor(0, -1),
                KeyCode::Right => self.move_cursor(0, 1),
                KeyCode::Char('s') if ctrl => self.find_moves_and_focus(),
                KeyCode::Char('r') if ctrl => self.show_reset_confirm(),
                KeyCode::Char('z') if ctrl => self.undo(),
                KeyCode::Char('=') if !ctrl => self.open_score_input(ScoreTarget::Opponent),
                KeyCode::Char('+') if !ctrl => self.open_score_input(ScoreTarget::Player),
                KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                    self.place_on_board(c.to_ascii_uppercase());
                }
                KeyCode::Char('?') => self.place_on_board('?'),
                KeyCode::Backspace => self.backspace_on_board(),
                KeyCode::Delete => self.remove_from_board(),
                KeyCode::Tab => self.next_focus(),
                KeyCode::Esc => self.show_escape_menu(),
                KeyCode::Char(' ') => self.toggle_direction(),
                KeyCode::Enter => {
                    if !self.suggestions.is_empty() {
                        self.apply_suggestion();
                    }
                }
                _ => {}
            },
        }
    }

    fn handle_rack_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('s') if ctrl => self.find_moves_and_focus(),
            KeyCode::Char('r') if ctrl => self.show_reset_confirm(),
            KeyCode::Char('z') if ctrl => self.undo(),
            KeyCode::Enter => self.find_moves_and_focus(),
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                self.add_to_rack(c.to_ascii_uppercase());
            }
            KeyCode::Char('?') => {
                self.add_to_rack('?');
            }
            KeyCode::Backspace => self.pop_from_rack(),
            KeyCode::Delete => self.clear_rack(),
            KeyCode::Tab => self.next_focus(),
            KeyCode::Esc => {
                self.focus = Focus::Board;
            }
            _ => {}
        }
    }

    fn handle_suggestions_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => self.move_suggestion_selection(-1),
            KeyCode::Down => self.move_suggestion_selection(1),
            KeyCode::Enter => self.apply_suggestion_and_focus(),
            KeyCode::Tab => self.next_focus(),
            KeyCode::Esc => {
                self.focus = Focus::Board;
            }
            KeyCode::Char('s') if !ctrl => self.cycle_sort_mode(),
            KeyCode::Char('s') if ctrl => self.find_moves_and_focus(),
            KeyCode::Char('r') if ctrl => self.show_reset_confirm(),
            KeyCode::Char('z') if ctrl => self.undo(),
            KeyCode::Char('m') | KeyCode::Char('M') if !ctrl => self.run_mcts_evaluation(),
            _ => {}
        }
    }

    fn handle_dialog_key(&mut self, key: KeyEvent) {
        match self.dialog {
            DialogState::None => {}

            DialogState::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::F(1)) {
                    self.close_dialog();
                }
            }

            DialogState::EscapeMenu(selected) => match key.code {
                KeyCode::Up => {
                    self.dialog = DialogState::EscapeMenu(selected.prev());
                }
                KeyCode::Down => {
                    self.dialog = DialogState::EscapeMenu(selected.next());
                }
                KeyCode::Esc => {
                    self.close_dialog();
                }
                KeyCode::Enter => match selected {
                    EscapeMenuOption::Quit => {
                        self.dialog = DialogState::ConfirmQuit(false);
                    }
                    EscapeMenuOption::Reset => {
                        self.dialog = DialogState::ConfirmReset(false);
                    }
                    EscapeMenuOption::Cancel => {
                        self.close_dialog();
                    }
                },
                _ => {}
            },

            DialogState::ConfirmQuit(yes_selected) => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    self.dialog = DialogState::ConfirmQuit(!yes_selected);
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.should_quit = true;
                    self.close_dialog();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.close_dialog();
                }
                KeyCode::Enter => {
                    if yes_selected {
                        self.should_quit = true;
                    }
                    self.close_dialog();
                }
                _ => {}
            },

            DialogState::ConfirmReset(yes_selected) => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    self.dialog = DialogState::ConfirmReset(!yes_selected);
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.close_dialog();
                    self.reset();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.close_dialog();
                }
                KeyCode::Enter => {
                    if yes_selected {
                        self.close_dialog();
                        self.reset();
                    } else {
                        self.close_dialog();
                    }
                }
                _ => {}
            },

            DialogState::ScoreInput {
                ref target,
                ref mut buffer,
            } => match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if buffer.len() < 4 {
                        buffer.push(c);
                    }
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Enter => {
                    let value: u32 = buffer.parse().unwrap_or(0);
                    let target = *target;
                    self.commit_score_input(target, value);
                }
                KeyCode::Esc => {
                    self.close_dialog();
                }
                _ => {}
            },
        }
    }

    // ── Business logic ──────────────────────────────────────────────

    /// Returns true if any background simulation is in progress.
    pub fn is_mcts_running(&self) -> bool {
        self.mcts_receiver.is_some()
            || self.refengine_receiver.is_some()
            || self.endgame_receiver.is_some()
    }

    /// Poll background simulation channels and incorporate results. Returns true if new results arrived.
    pub fn poll_mcts(&mut self) -> bool {
        // Check Crossplay endgame solver results first.
        if let Some(ref receiver) = self.endgame_receiver {
            match receiver.try_recv() {
                Ok(endgame_results) => {
                    let count = endgame_results.len();

                    // Convert EndgameMove results to RefSimResult for display compatibility
                    let sim_results: Vec<RefSimResult> = endgame_results
                        .iter()
                        .map(|eg| {
                            let val = eg.endgame_value as f32;
                            RefSimResult {
                                word: eg.mv.word.clone(),
                                row: eg.mv.row,
                                col: eg.mv.col,
                                direction: eg.mv.direction,
                                score: eg.mv.score,
                                leave: eg
                                    .mv
                                    .leave
                                    .iter()
                                    .map(|t| if t.is_blank { '?' } else { t.letter })
                                    .collect(),
                                win_pct: if val > 0.0 {
                                    100.0
                                } else if val < 0.0 {
                                    0.0
                                } else {
                                    50.0
                                },
                                sim_equity: val,
                                iterations: 0,
                                ply1_score: 0.0,
                                ply1_bingo_pct: 0.0,
                                ply2_score: 0.0,
                                ply2_bingo_pct: 0.0,
                                is_endgame: true,
                            }
                        })
                        .collect();

                    // Replace suggestions with fully-evaluated endgame moves
                    // (preserves all moves, not just top N)
                    let mut new_suggestions = Vec::with_capacity(endgame_results.len());
                    for eg in &endgame_results {
                        let mut mv = eg.mv.clone();
                        mv.equity = eg.endgame_value as f32;
                        new_suggestions.push(mv);
                    }
                    self.suggestions = new_suggestions;

                    self.refengine_results = Some(sim_results);
                    self.mcts_results = None;
                    // Don't clear inferred_opponent_rack here — the endgame
                    // solver doesn't produce inference, so preserve any existing
                    // inference from a prior RefEngine run.
                    self.last_applied_board = Some(self.state.board.clone());
                    self.sort_mode = SortMode::MctsEquity;
                    self.apply_sort();
                    self.selected_suggestion = 0;
                    self.endgame_receiver = None;
                    self.mcts_progress = None;

                    // Log PV for best move
                    if let Some(best) = endgame_results.first() {
                        let pv_str: Vec<String> = best
                            .pv
                            .iter()
                            .map(|p| format!("P{}:{} {}pts", p.player + 1, p.word, p.score))
                            .collect();
                        log(&format!("Endgame PV: {}", pv_str.join(" → ")));
                    }

                    self.message = Some(format!(
                        "Endgame: {} moves evaluated (Crossplay rules, no tile penalty)",
                        count
                    ));
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(ref mut progress) = self.mcts_progress {
                        progress.spinner_frame += 1;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.endgame_receiver = None;
                    self.mcts_progress = None;
                    self.message = Some("Endgame solver failed".to_string());
                    return true;
                }
            }
        }

        // Check RefEngine results (for non-endgame simulation).
        if let Some(ref receiver) = self.refengine_receiver {
            match receiver.try_recv() {
                Ok(Ok(output)) => {
                    let results = output.sim_results;
                    self.inferred_opponent_rack = output.inference;

                    let mut added_count = 0;
                    for result in &results {
                        let exists = self.suggestions.iter().any(|s| {
                            s.word.eq_ignore_ascii_case(&result.word)
                                && s.row == result.row
                                && s.col == result.col
                                && s.direction == result.direction
                        });

                        if !exists {
                            let data = crate::solver::ExternalMoveData {
                                word: &result.word,
                                row: result.row,
                                col: result.col,
                                direction: result.direction,
                                score: result.score,
                                equity: result.sim_equity,
                            };
                            match Move::from_external(&data, &self.state.board, &self.state.rack) {
                                Some(mv) => {
                                    self.suggestions.push(mv);
                                    added_count += 1;
                                }
                                None => {
                                    log(&format!(
                                        "Warning: could not reconstruct RefEngine move '{}' at ({},{}) {:?} from current rack",
                                        result.word, result.row, result.col, result.direction
                                    ));
                                }
                            }
                        }
                    }

                    self.refengine_results = Some(results);
                    self.mcts_results = None;
                    self.last_applied_board = Some(self.state.board.clone());
                    self.sort_mode = SortMode::MctsEquity;
                    self.apply_sort();
                    self.selected_suggestion = 0;
                    self.refengine_receiver = None;
                    self.mcts_progress = None;

                    let count = self.refengine_results.as_ref().map(|r| r.len()).unwrap_or(0);
                    let infer_note = if self.inferred_opponent_rack.is_some() {
                        " + opp rack"
                    } else {
                        ""
                    };
                    let msg = if added_count > 0 {
                        format!(
                            "RefEngine: {} moves (+{} new){}",
                            count, added_count, infer_note
                        )
                    } else {
                        format!("RefEngine: simulated {} moves{}", count, infer_note)
                    };
                    self.message = Some(msg);
                    return true;
                }
                Ok(Err(e)) => {
                    self.refengine_receiver = None;
                    self.mcts_progress = None;
                    self.message = Some(format!("RefEngine error: {}", e));
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(ref mut progress) = self.mcts_progress {
                        progress.spinner_frame += 1;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.refengine_receiver = None;
                    self.mcts_progress = None;
                    self.message = Some("RefEngine evaluation failed".to_string());
                    return true;
                }
            }
        }

        // Then check MCTS results (fallback).
        if let Some(ref receiver) = self.mcts_receiver {
            match receiver.try_recv() {
                Ok(results) => {
                    let mut new_suggestions = Vec::with_capacity(results.len());
                    for result in &results {
                        new_suggestions.push(result.mv.clone());
                    }

                    let evaluated_count = results.len();
                    if evaluated_count < self.suggestions.len() {
                        new_suggestions.extend(self.suggestions[evaluated_count..].iter().cloned());
                    }

                    self.suggestions = new_suggestions;
                    self.mcts_results = Some(results);
                    self.refengine_results = None;
                    self.selected_suggestion = 0;
                    self.sort_mode = SortMode::MctsEquity;
                    self.mcts_receiver = None;
                    self.mcts_progress = None;

                    let count = self.mcts_results.as_ref().map(|r| r.len()).unwrap_or(0);
                    self.message = Some(format!("RefEngine: evaluated {} moves", count));
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(ref mut progress) = self.mcts_progress {
                        progress.spinner_frame += 1;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.mcts_receiver = None;
                    self.mcts_progress = None;
                    self.message = Some("RefEngine evaluation failed".to_string());
                    return true;
                }
            }
        }
        false
    }

    /// Advance the spinner animation by one frame.
    pub fn tick_spinner(&mut self) {
        if let Some(ref mut progress) = self.mcts_progress {
            progress.spinner_frame += 1;
        }
    }

    /// Toggle the board typing direction between horizontal and vertical.
    pub fn toggle_direction(&mut self) {
        self.typing_direction = match self.typing_direction {
            Direction::Horizontal => Direction::Vertical,
            Direction::Vertical => Direction::Horizontal,
        };
    }

    /// Toggle the help dialog open or closed.
    pub fn toggle_help(&mut self) {
        self.dialog = match self.dialog {
            DialogState::Help => DialogState::None,
            DialogState::None => DialogState::Help,
            _ => return,
        };
    }

    /// Open the escape menu dialog.
    pub fn show_escape_menu(&mut self) {
        self.dialog = DialogState::EscapeMenu(EscapeMenuOption::Cancel);
    }

    /// Open the board-reset confirmation dialog.
    pub fn show_reset_confirm(&mut self) {
        self.dialog = DialogState::ConfirmReset(false);
    }

    /// Close any open dialog.
    pub fn close_dialog(&mut self) {
        self.dialog = DialogState::None;
    }

    /// Open the score input dialog, pre-filled with the current value.
    pub fn open_score_input(&mut self, target: ScoreTarget) {
        let current = match target {
            ScoreTarget::Player => self.our_score,
            ScoreTarget::Opponent => self.opp_score,
        };
        self.dialog = DialogState::ScoreInput {
            target,
            buffer: if current > 0 {
                current.to_string()
            } else {
                String::new()
            },
        };
    }

    /// Commit a score from the input dialog.
    fn commit_score_input(&mut self, target: ScoreTarget, value: u32) {
        // Push undo so the score change is reversible
        self.undo_stack.push(UndoState {
            state: self.state.clone(),
            our_score: self.our_score,
            opp_score: self.opp_score,
            message: format!(
                "Score: {}-{}",
                self.our_score, self.opp_score
            ),
        });

        let label = match target {
            ScoreTarget::Player => {
                self.our_score = value;
                "Your"
            }
            ScoreTarget::Opponent => {
                self.opp_score = value;
                "Opponent"
            }
        };
        let diff = self.our_score as i32 - self.opp_score as i32;
        let diff_str = if diff >= 0 {
            format!("+{}", diff)
        } else {
            format!("{}", diff)
        };
        self.message = Some(format!(
            "{} score set to {} \u{2014} {}-{} ({})",
            label, value, self.our_score, self.opp_score, diff_str
        ));
        self.close_dialog();
    }

    /// Persist current settings to disk. Logs errors silently.
    fn save_settings(&mut self) {
        use crate::settings::DefaultSort;
        self.settings.display.sidebar_visible = self.sidebar_visible;
        self.settings.display.default_sort = DefaultSort::from_sort_mode(self.sort_mode);
        if let Err(e) = self.settings.save() {
            log(&format!("Failed to save settings: {e}"));
        }
    }

    /// Returns true if any dialog is currently open.
    pub fn has_dialog(&self) -> bool {
        self.dialog != DialogState::None
    }

    /// Switch between the Board and Logs tabs.
    pub fn toggle_board_tab(&mut self) {
        self.board_tab = self.board_tab.toggle();
        if self.board_tab == BoardTab::Logs {
            self.log_scroll = 0;
        }
    }

    /// Toggle sidebar (rack + suggestions) visibility and persist.
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
        self.save_settings();
    }

    /// Scroll the log viewer up by the given number of lines.
    pub fn scroll_log_up(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_add(lines);
    }

    /// Scroll the log viewer down by the given number of lines.
    pub fn scroll_log_down(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_sub(lines);
    }

    /// Reset the log viewer scroll position to the bottom.
    pub fn scroll_log_to_bottom(&mut self) {
        self.log_scroll = 0;
    }

    /// Set the dictionary used for move generation.
    pub fn set_dictionary(&mut self, dict: Dictionary) {
        let count = dict.len();
        self.dictionary = Some(Arc::new(dict));
        self.message = Some(format!(
            "Loaded {} words. Place tiles on board, set rack, press S to solve.",
            count
        ));
    }

    /// Move the board cursor by the given row/column delta.
    pub fn move_cursor(&mut self, dr: i32, dc: i32) {
        let new_row = (self.cursor.0 as i32 + dr).clamp(0, 14) as usize;
        let new_col = (self.cursor.1 as i32 + dc).clamp(0, 14) as usize;
        self.cursor = (new_row, new_col);
    }

    /// Place a letter on the board at the cursor and advance in the typing direction.
    pub fn place_on_board(&mut self, letter: char) {
        let (row, col) = self.cursor;

        if letter == '?' {
            if let Some(tile) = self.state.get_tile_mut(row, col) {
                tile.is_blank = !tile.is_blank;
                self.clear_suggestions();
                self.message = None;
                return;
            }
            self.state.place_tile(row, col, 'A', true);
        } else {
            self.state.place_tile(row, col, letter, false);
        }

        self.clear_suggestions();
        self.message = None;

        match self.typing_direction {
            Direction::Horizontal => {
                if self.cursor.1 < 14 {
                    self.cursor.1 += 1;
                }
            }
            Direction::Vertical => {
                if self.cursor.0 < 14 {
                    self.cursor.0 += 1;
                }
            }
        }
    }

    /// Remove the tile at the current cursor position.
    pub fn remove_from_board(&mut self) {
        let (row, col) = self.cursor;
        if self.state.remove_tile(row, col).is_some() {
            self.clear_suggestions();
            self.message = None;
        }
    }

    /// Delete tile at cursor, or step back and delete if cursor cell is empty.
    pub fn backspace_on_board(&mut self) {
        let (row, col) = self.cursor;
        if self.state.get_tile(row, col).is_some() {
            self.state.remove_tile(row, col);
            self.clear_suggestions();
            self.message = None;
        } else {
            let (dr, dc) = self.typing_direction.delta();
            let new_row = row as i32 - dr;
            let new_col = col as i32 - dc;
            if (0..BOARD_SIZE as i32).contains(&new_row)
                && (0..BOARD_SIZE as i32).contains(&new_col)
            {
                self.cursor = (new_row as usize, new_col as usize);
                if self
                    .state
                    .remove_tile(new_row as usize, new_col as usize)
                    .is_some()
                {
                    self.clear_suggestions();
                    self.message = None;
                }
            }
        }
    }

    /// Add a letter tile to the rack, or show a message if full.
    pub fn add_to_rack(&mut self, letter: char) {
        if self.state.add_to_rack(letter) {
            self.clear_suggestions();
            self.message = None;
        } else {
            self.message = Some("Rack is full (max 7 tiles)".to_string());
        }
    }

    /// Remove the last tile from the rack.
    pub fn pop_from_rack(&mut self) {
        if self.state.pop_from_rack() {
            self.clear_suggestions();
            self.message = None;
        }
    }

    /// Remove all tiles from the rack.
    pub fn clear_rack(&mut self) {
        self.state.clear_rack();
        self.clear_suggestions();
        self.message = Some("Rack cleared".to_string());
    }

    fn clear_suggestions(&mut self) {
        self.suggestions.clear();
        self.mcts_results = None;
        self.refengine_results = None;
        // Intentionally preserve inferred_opponent_rack: inference results
        // are expensive to compute and should persist until replaced by new
        // inference from the next RefEngine simulation.
    }

    /// Run the solver to generate all legal moves for the current board and rack.
    pub fn find_moves(&mut self) {
        if self.state.rack.is_empty() {
            self.message = Some("Set your rack first (Tab to rack, type letters)".to_string());
            return;
        }

        // Auto-score the opponent's move if new tiles appeared since our last play.
        // Save the opponent tiles and pre-opponent board BEFORE updating the
        // snapshot, so RefEngine inference can still detect them later.
        self.pending_opponent_tiles = self.detect_opponent_tiles();
        self.pre_opponent_board = self.last_applied_board.clone();

        if let Some((word, score)) = self.score_opponent_move() {
            // Push undo state so the auto-score is reversible
            self.undo_stack.push(UndoState {
                state: self.state.clone(),
                our_score: self.our_score,
                opp_score: self.opp_score,
                message: format!("Before opp: {}", word),
            });
            self.opp_score += score;
            // Snapshot the board now so we don't re-score the same tiles next time
            self.last_applied_board = Some(self.state.board.clone());
            let diff = self.our_score as i32 - self.opp_score as i32;
            let diff_str = if diff >= 0 {
                format!("+{}", diff)
            } else {
                format!("{}", diff)
            };
            log(&format!(
                "Auto-scored opponent: {} ({} pts) \u{2014} {}-{} ({})",
                word, score, self.our_score, self.opp_score, diff_str
            ));
        }

        if let Some(dict) = &self.dictionary {
            let mut all_suggestions = self.state.get_suggestions(dict);
            let total_count = all_suggestions.len();

            let is_last_move = self.state.unseen_tile_count() == 0;

            if is_last_move {
                all_suggestions.sort_by(|a, b| b.score.cmp(&a.score));
            } else {
                const MAX_SUGGESTIONS: usize = 100;
                all_suggestions.truncate(MAX_SUGGESTIONS);
            }

            self.suggestions = all_suggestions;
            self.selected_suggestion = 0;
            self.mcts_results = None;
            self.refengine_results = None;

            if self.suggestions.is_empty() {
                self.message = Some("No valid moves found".to_string());
            } else if is_last_move {
                self.message = Some(format!("Last move: {} plays", total_count));
            } else if total_count > 100 {
                self.message = Some(format!("Showing top 100 of {} moves", total_count));
            } else {
                self.message = Some(format!("Found {} moves", self.suggestions.len()));
            }
        } else {
            self.message = Some("Dictionary not loaded".to_string());
        }
    }

    fn detect_opponent_tiles(&self) -> Option<String> {
        let prev = match self.last_applied_board.as_ref() {
            Some(b) => b,
            None => {
                log("detect_opponent_tiles: no board snapshot exists yet (first solve?)");
                return None;
            }
        };

        let mut snapshot_tiles = 0;
        let mut current_tiles = 0;
        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                if prev.get_tile(row, col).is_some() {
                    snapshot_tiles += 1;
                }
                if self.state.board.get_tile(row, col).is_some() {
                    current_tiles += 1;
                }
            }
        }
        log(&format!(
            "detect_opponent_tiles: snapshot has {} tiles, current board has {} tiles",
            snapshot_tiles, current_tiles
        ));

        let mut tiles = String::new();
        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                if prev.get_tile(row, col).is_none() {
                    if let Some(tile) = self.state.board.get_tile(row, col) {
                        if tile.is_blank {
                            tiles.push(tile.letter.to_ascii_lowercase());
                        } else {
                            tiles.push(tile.letter);
                        }
                    }
                }
            }
        }

        if tiles.is_empty() {
            log("detect_opponent_tiles: no new tiles found (boards are identical)");
            None
        } else {
            log(&format!(
                "detect_opponent_tiles: found opponent tiles: '{}'",
                tiles
            ));
            Some(tiles)
        }
    }

    /// Detect and score the opponent's move by diffing `last_applied_board`
    /// against the current board. Returns `(word, score)` if a move is found.
    ///
    /// The scoring uses `last_applied_board` (where premiums at opponent's
    /// positions are still unused) so premium multipliers apply correctly.
    fn score_opponent_move(&self) -> Option<(String, u32)> {
        let prev = self.last_applied_board.as_ref()?;
        let dict = self.dictionary.as_ref()?;

        // Collect placed tiles with positions
        let mut placed: Vec<(usize, usize, Tile)> = Vec::new();
        for row in 0..BOARD_SIZE {
            for col in 0..BOARD_SIZE {
                if prev.get_tile(row, col).is_none() {
                    if let Some(tile) = self.state.board.get_tile(row, col) {
                        placed.push((row, col, *tile));
                    }
                }
            }
        }

        if placed.is_empty() {
            return None;
        }

        // Determine direction
        let dir = if placed.len() == 1 {
            // Single tile: check which direction has adjacent existing tiles
            let (r, c, _) = placed[0];
            let has_h = (c > 0 && prev.get_tile(r, c - 1).is_some())
                || (c + 1 < BOARD_SIZE && prev.get_tile(r, c + 1).is_some());
            if has_h {
                Direction::Horizontal
            } else {
                Direction::Vertical
            }
        } else {
            let all_same_row = placed.iter().all(|(r, _, _)| *r == placed[0].0);
            if all_same_row {
                Direction::Horizontal
            } else {
                Direction::Vertical
            }
        };

        // Sort placed tiles in word order
        let mut placed = placed;
        match dir {
            Direction::Horizontal => placed.sort_by_key(|(_, c, _)| *c),
            Direction::Vertical => placed.sort_by_key(|(r, _, _)| *r),
        }

        // Score against the pre-opponent board (premiums still available)
        let solver = Solver::new(prev, dict);
        let score = solver.score_placed_tiles(&placed, dir);

        // Reconstruct the full word (placed tiles + existing tiles on prev board)
        let (dr, dc) = dir.delta();
        let (first_r, first_c, _) = placed[0];
        let mut start_r = first_r as i32;
        let mut start_c = first_c as i32;
        while Board::is_valid_position(start_r - dr, start_c - dc) {
            if prev
                .get_tile((start_r - dr) as usize, (start_c - dc) as usize)
                .is_some()
            {
                start_r -= dr;
                start_c -= dc;
            } else {
                break;
            }
        }

        let placed_set: std::collections::HashSet<(usize, usize)> =
            placed.iter().map(|(r, c, _)| (*r, *c)).collect();

        let mut word = String::new();
        let mut r = start_r;
        let mut c = start_c;
        loop {
            if !Board::is_valid_position(r, c) {
                break;
            }
            let ur = r as usize;
            let uc = c as usize;
            if let Some(tile) = if placed_set.contains(&(ur, uc)) {
                self.state.board.get_tile(ur, uc)
            } else {
                prev.get_tile(ur, uc)
            } {
                word.push(tile.letter);
                r += dr;
                c += dc;
            } else {
                break;
            }
        }

        log(&format!(
            "score_opponent_move: {} {:?} = {} pts ({} tiles placed)",
            word,
            dir,
            score,
            placed.len()
        ));

        Some((word, score))
    }

    /// Launch RefEngine simulation (or endgame solver) on a background thread.
    pub fn run_mcts_evaluation(&mut self) {
        if self.is_mcts_running() {
            return;
        }

        if self.suggestions.is_empty() {
            self.message = Some("Generate suggestions first (press S)".to_string());
            return;
        }

        let unseen = RefSimulator::unseen_count(&self.state);
        if unseen == 0 {
            // No opponent tiles remain — this is our absolute last move.
            // Covers the case where we emptied the bag, the opponent already
            // played their final turn, and now it's our turn with nothing left
            // to respond to. Pure score maximization (no 2-ply needed).
            self.sort_mode = SortMode::Score;
            self.apply_sort();
            self.selected_suggestion = 0;
            self.message = Some(format!(
                "Last move: {} plays (sorted by score)",
                self.suggestions.len()
            ));
            return;
        }

        // Endgame: use Crossplay-native solver (no leftover tile penalties).
        //
        // When unseen > 0 and is_endgame(), the opponent still holds tiles and
        // will play after us. opponent_has_turn=true is always correct here:
        //
        //   Crossplay turn order after bag empties:
        //     1. Player who did NOT empty the bag plays their last turn
        //     2. Player who emptied the bag plays their final turn
        //
        //   From this tool's perspective, we are always choosing OUR next move.
        //   If the opponent has tiles (unseen > 0), they will respond to our
        //   move, so we need 2-ply search (our move minus opponent's best reply).
        //
        //   The "we go last, no response" case is handled above (unseen == 0):
        //   the opponent already played, their tiles are gone, and we just
        //   maximize our own score.
        if RefSimulator::is_endgame(&self.state) && unseen > 0 {
            if let Some(dict) = self.dictionary.clone() {
                let state = self.state.clone();
                let total_moves = self.suggestions.len();

                self.mcts_progress = Some(MctsProgress {
                    total_candidates: total_moves,

                    spinner_frame: 0,
                });
                self.message = Some(format!(
                    "Endgame solver: evaluating {} moves ({} opp tiles)...",
                    total_moves, unseen
                ));

                let (tx, rx) = mpsc::channel();
                self.endgame_receiver = Some(rx);

                thread::spawn(move || {
                    let solver = EndgameSolver::new(&dict);

                    // Build opponent rack from unseen tiles
                    let unseen_tiles = state.unseen_tiles();
                    let mut opp_rack = Vec::new();
                    for (letter, count) in &unseen_tiles {
                        for _ in 0..*count {
                            if *letter == '?' {
                                opp_rack.push(crate::tile::Tile::blank(' '));
                            } else {
                                opp_rack.push(crate::tile::Tile::new(*letter));
                            }
                        }
                    }

                    let our_rack: Vec<_> = state.rack.tiles();
                    // 2-ply: we play, opponent responds with their best move.
                    // See comment above for why opponent_has_turn is always true here.
                    let results = solver.solve(&state.board, &our_rack, &opp_rack, true);
                    let _ = tx.send(results);
                });

                return;
            }
        }

        self.mcts_progress = Some(MctsProgress {
            total_candidates: self.suggestions.len().min(15),

            spinner_frame: 0,
        });

        if let Some(ref refengine) = self.refengine_simulator {
            let state = self.state.clone();
            let refengine = Arc::clone(refengine);

            let phase = crate::refengine_mcts::RefSimConfig::phase_label(unseen);
            self.message = Some(format!("Simulating ({})...", phase));

            // Use opponent tiles saved during find_moves() (before auto-scoring
            // updated the board snapshot). Falls back to live detection.
            let opponent_tiles = self
                .pending_opponent_tiles
                .take()
                .or_else(|| self.detect_opponent_tiles());
            let pre_opponent_board = self
                .pre_opponent_board
                .take()
                .or_else(|| self.last_applied_board.clone());
            let scores = (self.our_score, self.opp_score);

            log(&format!(
                "run_mcts_evaluation: opponent_tiles={:?}, has_pre_board={}, scores={:?}",
                opponent_tiles,
                pre_opponent_board.is_some(),
                scores
            ));

            let (tx, rx) = mpsc::channel();
            self.refengine_receiver = Some(rx);

            thread::spawn(move || {
                let result = refengine.simulate_and_infer(
                    &state,
                    opponent_tiles.as_deref(),
                    pre_opponent_board.as_ref(),
                    scores,
                );
                let _ = tx.send(result);
            });

            return;
        }

        let Some(dict) = self.dictionary.clone() else {
            self.message = Some("Dictionary not loaded".to_string());
            self.mcts_progress = None;
            return;
        };

        let config = MctsConfig::default();
        let state = self.state.clone();
        let candidates = self.suggestions.clone();

        let (tx, rx) = mpsc::channel();
        self.mcts_receiver = Some(rx);

        thread::spawn(move || {
            let evaluator = MctsEvaluator::new(config);
            let results = evaluator.evaluate(&state, &candidates, &dict);
            let _ = tx.send(results);
        });

        self.message = None;
    }

    /// Find moves, focus the suggestions pane, and start simulation.
    pub fn find_moves_and_focus(&mut self) {
        self.find_moves();
        if !self.suggestions.is_empty() {
            self.focus = Focus::Suggestions;
            self.run_mcts_evaluation();
        }
    }

    /// Apply the currently selected suggestion to the board.
    pub fn apply_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }

        self.undo_stack.push(UndoState {
            state: self.state.clone(),
            our_score: self.our_score,
            opp_score: self.opp_score,
            message: format!(
                "Before: {}",
                self.suggestions[self.selected_suggestion].word
            ),
        });

        let mv = self.suggestions[self.selected_suggestion].clone();
        self.state.apply_move(&mv);
        self.our_score += mv.score;
        self.last_applied_board = Some(self.state.board.clone());
        let diff = self.our_score as i32 - self.opp_score as i32;
        let diff_str = if diff >= 0 {
            format!("+{}", diff)
        } else {
            format!("{}", diff)
        };
        self.message = Some(format!(
            "Applied: {} ({} pts) \u{2014} Score: {}-{} ({})",
            mv.word, mv.score, self.our_score, self.opp_score, diff_str
        ));
        self.clear_suggestions();
    }

    /// Undo the last applied move, restoring the previous board, rack, and scores.
    pub fn undo(&mut self) {
        if let Some(undo_state) = self.undo_stack.pop() {
            self.state = undo_state.state;
            self.our_score = undo_state.our_score;
            self.opp_score = undo_state.opp_score;
            self.clear_suggestions();
            self.message = Some(format!("Undone: {}", undo_state.message));
        } else {
            self.message = Some("Nothing to undo".to_string());
        }
    }

    /// Apply the selected suggestion and return focus to the board.
    pub fn apply_suggestion_and_focus(&mut self) {
        self.apply_suggestion();
        self.focus = Focus::Board;
    }

    /// Move the suggestion selection up or down by delta, wrapping around.
    pub fn move_suggestion_selection(&mut self, delta: i32) {
        if self.suggestions.is_empty() {
            return;
        }
        let len = self.suggestions.len() as i32;
        let new_idx = (self.selected_suggestion as i32 + delta).rem_euclid(len) as usize;
        self.selected_suggestion = new_idx;
    }

    /// Reset the board, rack, and all state to initial defaults.
    pub fn reset(&mut self) {
        self.state.reset();
        self.cursor = (7, 7);
        self.typing_direction = Direction::Horizontal;
        self.clear_suggestions();
        self.inferred_opponent_rack = None;
        self.last_applied_board = None;
        self.pending_opponent_tiles = None;
        self.pre_opponent_board = None;
        self.our_score = 0;
        self.opp_score = 0;
        self.message = Some("Reset complete".to_string());
    }

    /// Cycle input focus to the next pane.
    pub fn next_focus(&mut self) {
        self.focus = self.focus.next();
    }

    /// Advance to the next sort mode, re-sort suggestions, and persist.
    pub fn cycle_sort_mode(&mut self) {
        let has_sim = self.mcts_results.is_some() || self.refengine_results.is_some();
        self.sort_mode = self.sort_mode.next(has_sim);
        self.apply_sort();
        self.selected_suggestion = 0;
        self.message = Some(format!("Sort: {}", self.sort_mode.label()));
        self.save_settings();
    }

    /// Set a specific sort mode and re-sort suggestions.
    pub fn set_sort_mode(&mut self, mode: SortMode) {
        self.sort_mode = mode;
        self.apply_sort();
        self.selected_suggestion = 0;
    }

    fn apply_sort(&mut self) {
        match self.sort_mode {
            SortMode::Equity => {
                self.suggestions.sort_by(|a, b| {
                    b.equity
                        .partial_cmp(&a.equity)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            SortMode::MctsEquity => {
                if let Some(ref results) = self.refengine_results {
                    let sim_map: std::collections::HashMap<_, _> = results
                        .iter()
                        .map(|r| {
                            let word_upper = r.word.to_uppercase();
                            ((word_upper, r.row, r.col, r.direction), r.sim_equity)
                        })
                        .collect();

                    self.suggestions.sort_by(|a, b| {
                        let a_word = a.word.to_uppercase();
                        let b_word = b.word.to_uppercase();
                        let a_sim = sim_map.get(&(a_word.clone(), a.row, a.col, a.direction));
                        let b_sim = sim_map.get(&(b_word.clone(), b.row, b.col, b.direction));

                        match (a_sim, b_sim) {
                            (Some(a_eq), Some(b_eq)) => {
                                b_eq.partial_cmp(a_eq).unwrap_or(std::cmp::Ordering::Equal)
                            }
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => b
                                .equity
                                .partial_cmp(&a.equity)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        }
                    });
                } else if let Some(ref results) = self.mcts_results {
                    let mcts_map: std::collections::HashMap<_, _> = results
                        .iter()
                        .map(|r| {
                            (
                                (&r.mv.word, r.mv.row, r.mv.col, r.mv.direction),
                                r.mcts_equity,
                            )
                        })
                        .collect();

                    self.suggestions.sort_by(|a, b| {
                        let a_mcts = mcts_map.get(&(&a.word, a.row, a.col, a.direction));
                        let b_mcts = mcts_map.get(&(&b.word, b.row, b.col, b.direction));

                        match (a_mcts, b_mcts) {
                            (Some(a_eq), Some(b_eq)) => {
                                b_eq.partial_cmp(a_eq).unwrap_or(std::cmp::Ordering::Equal)
                            }
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => b
                                .equity
                                .partial_cmp(&a.equity)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        }
                    });
                } else {
                    self.suggestions.sort_by(|a, b| {
                        b.equity
                            .partial_cmp(&a.equity)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }
            SortMode::WinPct => {
                if let Some(ref results) = self.refengine_results {
                    let win_map: std::collections::HashMap<_, _> = results
                        .iter()
                        .map(|r| {
                            let word_upper = r.word.to_uppercase();
                            ((word_upper, r.row, r.col, r.direction), r.win_pct)
                        })
                        .collect();

                    self.suggestions.sort_by(|a, b| {
                        let a_word = a.word.to_uppercase();
                        let b_word = b.word.to_uppercase();
                        let a_win = win_map.get(&(a_word, a.row, a.col, a.direction));
                        let b_win = win_map.get(&(b_word, b.row, b.col, b.direction));

                        match (a_win, b_win) {
                            (Some(a_w), Some(b_w)) => {
                                b_w.partial_cmp(a_w).unwrap_or(std::cmp::Ordering::Equal)
                            }
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => b
                                .equity
                                .partial_cmp(&a.equity)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        }
                    });
                } else {
                    // No RefEngine results — fall back to equity sort
                    self.suggestions.sort_by(|a, b| {
                        b.equity
                            .partial_cmp(&a.equity)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }
            SortMode::Score => {
                self.suggestions.sort_by(|a, b| b.score.cmp(&a.score));
            }
            SortMode::Leave => {
                self.suggestions.sort_by(|a, b| {
                    let a_leave = a.equity - a.score as f32;
                    let b_leave = b.equity - b.score as f32;
                    b_leave
                        .partial_cmp(&a_leave)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            SortMode::WordLength => {
                self.suggestions.sort_by(|a, b| {
                    b.word.len().cmp(&a.word.len()).then_with(|| {
                        b.equity
                            .partial_cmp(&a.equity)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                });
            }
            SortMode::Alpha => {
                self.suggestions.sort_by(|a, b| a.word.cmp(&b.word));
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
