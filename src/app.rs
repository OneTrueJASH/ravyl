//! Central application state, event handling, and business logic.
//!
//! Contains the [`App`] struct which owns all game state, dialog state,
//! and coordinates between the solver, MCTS engine, and UI.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::board::{Board, Direction, BOARD_SIZE};
use crate::dictionary::Dictionary;
use crate::endgame::{EndgameMove, EndgameSolver};
use crate::game::{observed_move_loss, SolverState};
use crate::logging::log;
use crate::refengine_mcts::{InferenceResult, RefSimulator};
use crate::mcts::{MctsConfig, MctsEvaluator, MctsResult, OpponentModel, TilePool};
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

/// Evaluation result for one suggestion, as the suggestions table shows it.
#[derive(Debug, Clone)]
pub struct SimResult {
    pub word: String,
    pub row: usize,
    pub col: usize,
    pub direction: Direction,
    /// Win probability estimate in percent (exact 0/50/100 for endgame lines)
    pub win_pct: f32,
    /// Expected final spread change (playouts) or exact 2-ply value (endgame)
    pub sim_equity: f32,
    /// Percent of playouts in which the opponent's reply was a bingo
    pub ply1_bingo_pct: f32,
    /// Percent of playouts in which our follow-up was a bingo
    pub ply2_bingo_pct: f32,
    /// From the endgame solver (exact) rather than playouts
    pub is_endgame: bool,
    /// Playouts behind the estimate (0 for exact endgame lines)
    pub iterations: usize,
}

/// Who plays the very last move of the game.
///
/// Crossplay's equal-turn rule: when a player draws the last tile from the
/// bag, the other player gets one more turn and then the player who emptied
/// the bag plays the final move. So when the bag is empty on our turn, the
/// opponent only gets to reply if *they* emptied the bag. The app infers
/// this from moves applied through the suggestions list and the user can
/// override it with Ctrl+E.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FinalTurn {
    /// The opponent emptied the bag: we play our last turn, they reply.
    #[default]
    Opponent,
    /// We emptied the bag: the opponent has already played their last
    /// turn, so our next move ends the game.
    Us,
}

impl FinalTurn {
    pub fn toggle(self) -> Self {
        match self {
            FinalTurn::Opponent => FinalTurn::Us,
            FinalTurn::Us => FinalTurn::Opponent,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FinalTurn::Opponent => "OPP",
            FinalTurn::Us => "YOU",
        }
    }
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
            SortMode::MctsEquity => "Sim",
            SortMode::WinPct => "Win%",
            SortMode::Score => "Score",
            SortMode::Leave => "Leave",
            SortMode::WordLength => "Length",
            SortMode::Alpha => "A-Z",
        }
    }
}

/// An opponent move found by board diff: word, score, and placed tiles.
type DetectedOpponentMove = (String, u32, Vec<(usize, usize, Tile)>);

/// Saved state for undo
#[derive(Clone)]
struct UndoState {
    state: SolverState,
    our_score: u32,
    opp_score: u32,
    last_applied_board: Option<Board>,
    final_turn: FinalTurn,
    final_turn_known: bool,
    opp_loss_total: f32,
    opp_moves_measured: u32,
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
    /// Evaluation results for the current suggestions (playouts or endgame).
    pub sim_results: Option<Vec<SimResult>>,
    playout_receiver: Option<Receiver<Vec<MctsResult>>>,
    /// The in-flight inference, tagged with the opponent-move stamp it was
    /// started for so a late result for an old move is dropped.
    inference_receiver: Option<(u64, Receiver<Option<InferenceResult>>)>,
    endgame_receiver: Option<Receiver<Vec<EndgameMove>>>,
    pub mcts_progress: Option<MctsProgress>,
    refengine_simulator: Option<Arc<RefSimulator>>,
    /// Why RefEngine could not be started, when it could not.
    refengine_unavailable: Option<String>,
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
    /// Who has the final move once the bag is empty (see [`FinalTurn`]).
    pub final_turn: FinalTurn,
    /// False until the final turn was inferred from an applied move or set
    /// with Ctrl+E; a position entered by hand starts with the default.
    pub final_turn_known: bool,
    /// Cumulative equity the opponent's observed moves gave up versus their
    /// best alternatives (see [`crate::game::observed_move_loss`]), and how
    /// many moves were measured. Their mean drives how strongly the
    /// playouts' simulated opponent plays.
    pub opp_loss_total: f32,
    pub opp_moves_measured: u32,
    /// Bumped whenever a new opponent move is auto-scored; inference
    /// results carry the stamp they were started under, so a result that
    /// arrives after the opponent moved again is recognisably stale.
    opp_move_stamp: u64,
}

/// Cap on the playout opponent's softmax temperature: beyond roughly the
/// tempo of a turn the playouts stop resembling the game.
const MAX_OPP_TEMPERATURE: f32 = 12.0;

/// How many rack completions each observed opponent move is measured
/// against. The measurement runs synchronously in Ctrl+S: typically some
/// tens of milliseconds, with a rare slow tail when sampled completions
/// hold blanks. Kept on this thread because the running total feeds undo
/// (snapshotted in the same push as the auto-score); moving it to a
/// background thread would need its own staleness accounting.
const OPP_LOSS_SAMPLES: usize = 10;

/// Fitted by `study_loss_calibration` (mcts.rs, 120 games per point): the
/// estimator reads `measured ~= 7.78 + 0.557 * tau` for a softmax-`tau`
/// opponent. The baseline exists because a sampled completion of their
/// rack usually admits a better play than the tiles they really held, so
/// even a perfect opponent measures near 8 points of "loss".
const OPP_LOSS_BASELINE: f32 = 7.78;
const OPP_LOSS_PER_TEMPERATURE: f32 = 0.557;

/// From the same study, a softmax-`tau` opponent's true mean equity loss
/// per move is about `0.66 * tau`; used for the display estimate.
const OPP_TRUE_LOSS_PER_TEMPERATURE: f32 = 0.66;

impl App {
    /// Create a new App with default settings loaded from disk.
    pub fn new() -> Self {
        Self::with_settings(Settings::load())
    }

    /// Create a new App with the given settings.
    pub fn with_settings(settings: Settings) -> Self {
        let refengine = crate::datafiles::refengine_data_dir()
            .and_then(|dir| RefSimulator::new(&dir.to_string_lossy(), settings::LEXICON));
        let (refengine_simulator, refengine_unavailable) = match refengine {
            Ok(sim) => (Some(Arc::new(sim)), None),
            Err(e) => {
                log(&format!("Opponent inference unavailable: {}", e));
                (None, Some(e))
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
            sim_results: None,
            playout_receiver: None,
            inference_receiver: None,
            endgame_receiver: None,
            mcts_progress: None,
            refengine_simulator,
            refengine_unavailable,
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
            final_turn: FinalTurn::default(),
            final_turn_known: false,
            opp_loss_total: 0.0,
            opp_moves_measured: 0,
            opp_move_stamp: 0,
        }
    }

    /// Softmax temperature for the playouts' simulated opponent, from the
    /// calibrated inversion of their measured mean loss, shrunk toward
    /// zero (a perfect opponent) while few moves have been measured so a
    /// single noisy reading cannot swing the playouts.
    pub fn opp_temperature(&self) -> f32 {
        let n = self.opp_moves_measured;
        if n == 0 {
            return 0.0;
        }
        let measured = self.opp_loss_total / n as f32;
        let temperature = (measured - OPP_LOSS_BASELINE) / OPP_LOSS_PER_TEMPERATURE;
        let shrink = n as f32 / (n as f32 + 2.0);
        (temperature * shrink).clamp(0.0, MAX_OPP_TEMPERATURE)
    }

    /// Note for the stats pane when opponent inference cannot run.
    pub fn inference_note(&self) -> Option<&'static str> {
        self.refengine_unavailable
            .as_ref()
            .map(|_| "inference unavailable")
    }

    /// Estimated true equity loss per opponent move for the stats pane,
    /// with the measurement count. `None` until a move has been measured.
    pub fn opp_error_estimate(&self) -> Option<(f32, u32)> {
        if self.opp_moves_measured == 0 {
            return None;
        }
        Some((
            self.opp_temperature() * OPP_TRUE_LOSS_PER_TEMPERATURE,
            self.opp_moves_measured,
        ))
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

        // Ctrl+E toggles who has the final turn of the game.
        if key.code == KeyCode::Char('e')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !self.has_dialog()
        {
            self.toggle_final_turn();
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
                KeyCode::Enter if !self.suggestions.is_empty() => {
                    self.apply_suggestion();
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

    /// Returns true if any background evaluation is in progress.
    pub fn is_mcts_running(&self) -> bool {
        self.playout_receiver.is_some() || self.endgame_receiver.is_some()
    }

    /// Returns true if anything at all is still running in the background,
    /// including opponent-rack inference (which outlives the ranking).
    pub fn has_background_work(&self) -> bool {
        self.is_mcts_running() || self.inference_receiver.is_some()
    }

    /// Convert a playout result into the display record the suggestions
    /// table reads (bingo rates as percentages, win rate as a percentage).
    fn sim_result_from_playout(r: &MctsResult) -> SimResult {
        SimResult {
            word: r.mv.word.clone(),
            row: r.mv.row,
            col: r.mv.col,
            direction: r.mv.direction,
            win_pct: r.win_rate * 100.0,
            sim_equity: r.mcts_equity,
            ply1_bingo_pct: r.opp_bingo_rate * 100.0,
            ply2_bingo_pct: r.our_bingo_rate * 100.0,
            is_endgame: false,
            iterations: r.iterations,
        }
    }

    /// Poll background channels and incorporate results. Returns true if new results arrived.
    pub fn poll_mcts(&mut self) -> bool {
        // Opponent-rack inference (RefEngine) finishes independently of ranking.
        // A result is only kept if the opponent has not moved again since it
        // was started: a late result describes a rack they no longer hold.
        if let Some((stamp, ref receiver)) = self.inference_receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    if result.is_some() && stamp == self.opp_move_stamp {
                        self.inferred_opponent_rack = result;
                    } else if result.is_some() {
                        log("Dropped a stale inference result (opponent moved since)");
                    }
                    self.inference_receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.inference_receiver = None;
                }
            }
        }

        // Crossplay endgame solver results.
        if let Some(ref receiver) = self.endgame_receiver {
            match receiver.try_recv() {
                Ok(endgame_results) => {
                    let count = endgame_results.len();

                    // Convert EndgameMove results into display records. The
                    // game ends after these two plies, so the final margin is
                    // known exactly: win% is 100/50/0.
                    let score_diff = self.our_score as i32 - self.opp_score as i32;
                    let sim_results: Vec<SimResult> = endgame_results
                        .iter()
                        .map(|eg| {
                            let val = eg.endgame_value as f32;
                            let final_margin = score_diff + eg.endgame_value;
                            SimResult {
                                word: eg.mv.word.clone(),
                                row: eg.mv.row,
                                col: eg.mv.col,
                                direction: eg.mv.direction,
                                win_pct: match final_margin.cmp(&0) {
                                    std::cmp::Ordering::Greater => 100.0,
                                    std::cmp::Ordering::Less => 0.0,
                                    std::cmp::Ordering::Equal => 50.0,
                                },
                                sim_equity: val,
                                ply1_bingo_pct: 0.0,
                                ply2_bingo_pct: 0.0,
                                is_endgame: true,
                                iterations: 0,
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

                    self.sim_results = Some(sim_results);
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
                        "Endgame: {} moves ranked by score minus opponent's best reply",
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

        // Playout results.
        if let Some(ref receiver) = self.playout_receiver {
            match receiver.try_recv() {
                Ok(results) => {
                    let mut new_suggestions = Vec::with_capacity(results.len());
                    for result in &results {
                        log(&format!(
                            "Playout: {} {} score={} spread={:+.1} win={:.0}% ({} playouts)",
                            result.mv.word,
                            result.mv.coords(),
                            result.mv.score,
                            result.mcts_equity,
                            result.win_rate * 100.0,
                            result.iterations
                        ));
                        new_suggestions.push(result.mv.clone());
                    }

                    // Evaluated moves first, then the rest of the list.
                    let evaluated: std::collections::HashSet<(String, usize, usize, Direction)> =
                        results
                            .iter()
                            .map(|r| (r.mv.word.clone(), r.mv.row, r.mv.col, r.mv.direction))
                            .collect();
                    new_suggestions.extend(
                        self.suggestions
                            .iter()
                            .filter(|m| {
                                !evaluated.contains(&(m.word.clone(), m.row, m.col, m.direction))
                            })
                            .cloned(),
                    );

                    self.suggestions = new_suggestions;
                    self.sim_results =
                        Some(results.iter().map(Self::sim_result_from_playout).collect());
                    self.last_applied_board = Some(self.state.board.clone());
                    self.selected_suggestion = 0;
                    self.sort_mode = SortMode::MctsEquity;
                    self.apply_sort();
                    self.playout_receiver = None;
                    self.mcts_progress = None;

                    let shortened = results.iter().any(|r| r.iterations < results[0].iterations);
                    let count = results.len();
                    self.message = Some(if shortened {
                        format!(
                            "Playout: {} moves ranked by expected final spread (some cut short)",
                            count
                        )
                    } else {
                        format!("Playout: {} moves ranked by expected final spread", count)
                    });
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(ref mut progress) = self.mcts_progress {
                        progress.spinner_frame += 1;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.playout_receiver = None;
                    self.mcts_progress = None;
                    self.message = Some("Playout evaluation failed".to_string());
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
            last_applied_board: self.last_applied_board.clone(),
            final_turn: self.final_turn,
            final_turn_known: self.final_turn_known,
            opp_loss_total: self.opp_loss_total,
            opp_moves_measured: self.opp_moves_measured,
            message: format!("Score: {}-{}", self.our_score, self.opp_score),
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
        self.message = Some(if self.sidebar_visible {
            "Sidebar shown".to_string()
        } else {
            "Sidebar hidden (\\ restores it)".to_string()
        });
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
            "Loaded {} words. Place tiles on board, set rack, Ctrl+S to solve.",
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
        self.sim_results = None;
        // Intentionally preserve inferred_opponent_rack: inference results
        // are expensive to compute and should persist until replaced by new
        // inference from the next RefEngine simulation.
    }

    /// The suggestion list ordered by static equity, best first, regardless
    /// of the sort mode on screen: playouts evaluate the top of this list.
    fn candidates_by_static_equity(&self) -> Vec<Move> {
        let mut candidates = self.suggestions.clone();
        candidates.sort_by(|a, b| {
            b.equity
                .partial_cmp(&a.equity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.score.cmp(&a.score))
        });
        candidates
    }

    /// Flip who has the final move of the game (see [`FinalTurn`]).
    pub fn toggle_final_turn(&mut self) {
        self.final_turn = self.final_turn.toggle();
        self.final_turn_known = true;
        self.clear_suggestions();
        self.message = Some(match self.final_turn {
            FinalTurn::Opponent => {
                "Final turn: opponent (they emptied the bag — they reply to your play)".to_string()
            }
            FinalTurn::Us => {
                "Final turn: you (you emptied the bag — no reply after your play)".to_string()
            }
        });
    }

    /// True when our next play is the last move of the game: either the
    /// opponent has no tiles, or the bag is empty and we emptied it (the
    /// opponent has already taken their last turn).
    pub fn our_move_ends_game(&self) -> bool {
        let unseen = self.state.unseen_tile_count();
        unseen == 0 || (self.state.is_endgame() && self.final_turn == FinalTurn::Us)
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

        if let Some((word, score, placed)) = self.score_opponent_move() {
            // Push undo state so the auto-score is reversible
            self.undo_stack.push(UndoState {
                state: self.state.clone(),
                our_score: self.our_score,
                opp_score: self.opp_score,
                last_applied_board: self.last_applied_board.clone(),
                final_turn: self.final_turn,
                final_turn_known: self.final_turn_known,
                opp_loss_total: self.opp_loss_total,
                opp_moves_measured: self.opp_moves_measured,
                message: format!("Before opp: {}", word),
            });
            self.opp_score += score;
            // Snapshot the board now so we don't re-score the same tiles next time
            self.last_applied_board = Some(self.state.board.clone());
            // This move changed their rack: the stored inference describes
            // tiles they have now partly played, so it must not bias deals.
            self.opp_move_stamp += 1;
            self.inferred_opponent_rack = None;
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

            // Measure how much equity the move gave up against sampled
            // completions of their rack; the running mean sets how strongly
            // the playouts' simulated opponent plays.
            if let (Some(prev), Some(dict)) = (&self.pre_opponent_board, &self.dictionary) {
                let pool = self.state.unseen_tiles();
                if let Some(loss) = observed_move_loss(
                    prev,
                    &placed,
                    score,
                    &pool,
                    dict,
                    OPP_LOSS_SAMPLES,
                    0xB1A5 ^ (self.opp_score as u64),
                ) {
                    self.opp_loss_total += loss;
                    self.opp_moves_measured += 1;
                    log(&format!(
                        "Opponent gave up ~{:.1} equity ({} moves measured, mean {:.1})",
                        loss,
                        self.opp_moves_measured,
                        self.opp_loss_total / self.opp_moves_measured as f32
                    ));
                }
            }
        }

        if let Some(dict) = &self.dictionary {
            let mut all_suggestions = self.state.get_suggestions(dict);
            let total_count = all_suggestions.len();

            let is_last_move = self.our_move_ends_game();

            if is_last_move {
                // No reply is coming and there is no leave to protect:
                // rank by raw score.
                all_suggestions.sort_by_key(|m| std::cmp::Reverse(m.score));
            } else {
                const MAX_SUGGESTIONS: usize = 100;
                all_suggestions.truncate(MAX_SUGGESTIONS);
            }

            self.suggestions = all_suggestions;
            self.selected_suggestion = 0;
            self.sim_results = None;

            if self.suggestions.is_empty() {
                self.message = Some("No valid moves found".to_string());
            } else if is_last_move {
                self.message = Some(format!(
                    "Final move of the game: {} plays, sorted by score",
                    total_count
                ));
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
    /// against the current board. Returns `(word, score, placed tiles)` if
    /// a move is found.
    ///
    /// The scoring uses `last_applied_board` (where premiums at opponent's
    /// positions are still unused) so premium multipliers apply correctly.
    fn score_opponent_move(&self) -> Option<DetectedOpponentMove> {
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

        Some((word, score, placed))
    }

    /// Launch the evaluation for the current position on a background
    /// thread. Every phase is judged under Crossplay rules by the native
    /// engines; RefEngine only infers the opponent's likely rack.
    pub fn run_mcts_evaluation(&mut self) {
        if self.is_mcts_running() {
            return;
        }
        // One log per evaluation keeps the log viewer readable.
        crate::logging::clear_log();

        if self.suggestions.is_empty() {
            self.message = Some("Generate suggestions first (Ctrl+S)".to_string());
            return;
        }

        let Some(dict) = self.dictionary.clone() else {
            self.message = Some("Dictionary not loaded".to_string());
            return;
        };

        let unseen = self.state.unseen_tile_count();
        if self.our_move_ends_game() {
            // Nothing follows our play: either the opponent has no tiles, or
            // we emptied the bag and they have already taken their last turn.
            // Pure score maximization, no search needed.
            self.sort_mode = SortMode::Score;
            self.apply_sort();
            self.selected_suggestion = 0;
            self.message = Some(format!(
                "Final move of the game: {} plays, sorted by score",
                self.suggestions.len()
            ));
            return;
        }

        // Bag empty, opponent still holds tiles and gets the final move:
        // exact 2-ply search (our score minus the opponent's best reply,
        // passing included; no leftover-tile penalties).
        if self.state.is_endgame() {
            let state = self.state.clone();

            // The solver regenerates every play itself (the list on
            // screen is capped), so no count is known up front.
            self.mcts_progress = Some(MctsProgress {
                total_candidates: 0,
                spinner_frame: 0,
            });
            let caveat = if self.final_turn_known {
                ""
            } else {
                " — assuming they emptied the bag; Ctrl+E if you did"
            };
            self.message = Some(format!(
                "Endgame: searching the opponent's replies ({} tiles){}...",
                unseen, caveat
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
                let results = solver.solve(&state.board, &our_rack, &opp_rack, true);
                let _ = tx.send(results);
            });
            return;
        }

        // Bag not empty: Monte Carlo playouts under Crossplay rules. The
        // playout depth follows the bag: with few tiles left the game ends
        // inside the horizon and the equal-turn ending is played exactly.
        let bag = self.state.bag_size();
        let config =
            MctsConfig::for_bag(bag).with_score_diff(self.our_score as i32 - self.opp_score as i32);
        let candidates = self.candidates_by_static_equity();
        let count = candidates.len().min(config.num_candidates);
        self.mcts_progress = Some(MctsProgress {
            total_candidates: count,
            spinner_frame: 0,
        });
        self.message = Some(format!(
            "{}: playing out {} candidates ({} in bag, {} plies)...",
            crate::stats::GamePhase::from_unseen_count(unseen as u32).label(),
            count,
            bag,
            config.max_plies
        ));
        let state = self.state.clone();

        // Opponent model for the playouts: their measured strength, plus
        // rack marginals from inference. A fresh inference for the move
        // just detected is worth a short wait. The stored result is used
        // otherwise; it is cleared whenever a new opponent move is scored
        // and stale arrivals are dropped by stamp, so whatever it holds
        // describes the rack they hold right now (an M re-run, or a fresh
        // result that landed earlier). If the wait times out the deals are
        // uniform this evaluation and the result still reaches the pane.
        let fresh_inference = self.start_inference();
        let stored_inference = self.inferred_opponent_rack.clone();
        let temperature = self.opp_temperature();

        let (tx, rx) = mpsc::channel();
        self.playout_receiver = Some(rx);
        thread::spawn(move || {
            let inference = fresh_inference
                .and_then(|rx| rx.recv_timeout(std::time::Duration::from_secs(3)).ok())
                .flatten()
                .or(stored_inference);
            let model = Self::opponent_model(inference.as_ref(), &state, temperature);
            match &model {
                Some(m) if m.has_marginals() => log(&format!(
                    "Opponent model: rack marginals from inference, temperature {:.1}",
                    m.temperature
                )),
                Some(m) => log(&format!(
                    "Opponent model: temperature {:.1}, no rack inference",
                    m.temperature
                )),
                None => log("Opponent model: none (uniform deals, greedy opponent)"),
            }
            let evaluator = MctsEvaluator::new(config);
            let results = evaluator.evaluate_with_model(&state, &candidates, &dict, model.as_ref());
            let _ = tx.send(results);
        });
    }

    /// Build the playout opponent model from an inference result and the
    /// measured opponent strength. `None` when there is nothing to model
    /// (no marginals and a perfect-greedy temperature).
    fn opponent_model(
        inference: Option<&InferenceResult>,
        state: &SolverState,
        temperature: f32,
    ) -> Option<OpponentModel> {
        let mut model = OpponentModel::new(temperature);
        if let Some(inf) = inference {
            let kept = crate::tile::RACK_SIZE.saturating_sub(inf.played_tiles.chars().count());
            let marginals: Vec<(char, f32)> = inf
                .letter_marginals
                .iter()
                .map(|m| (m.letter, m.expected))
                .collect();
            let pool = TilePool::from_remaining(&state.unseen_tiles());
            model = model.with_marginals(&marginals, kept, &pool);
        }
        if model.has_marginals() || temperature > 0.0 {
            Some(model)
        } else {
            None
        }
    }

    /// Ask RefEngine what the opponent probably kept, if their last move was
    /// detected on the board and the bag still has tiles. Runs in the
    /// background; the result updates the stats pane, and the returned
    /// second receiver lets the evaluation thread wait briefly for it so
    /// fresh inference can bias the playout deals.
    fn start_inference(&mut self) -> Option<Receiver<Option<InferenceResult>>> {
        let refengine = self.refengine_simulator.as_ref()?;
        if self.inference_receiver.is_some() || self.state.is_endgame() {
            return None;
        }
        // Use opponent tiles saved during find_moves() (before auto-scoring
        // updated the board snapshot). Falls back to live detection.
        let opponent_tiles = self
            .pending_opponent_tiles
            .take()
            .or_else(|| self.detect_opponent_tiles())?;
        let pre_opponent_board = self
            .pre_opponent_board
            .take()
            .or_else(|| self.last_applied_board.clone());
        let scores = (self.our_score, self.opp_score);
        let state = self.state.clone();
        let refengine = Arc::clone(refengine);

        let (tx, rx) = mpsc::channel();
        let (tx_eval, rx_eval) = mpsc::channel();
        self.inference_receiver = Some((self.opp_move_stamp, rx));
        thread::spawn(move || {
            let result = refengine.infer_opponent_rack(
                &state,
                &opponent_tiles,
                pre_opponent_board.as_ref(),
                scores,
            );
            let _ = tx_eval.send(result.clone());
            let _ = tx.send(result);
        });
        Some(rx_eval)
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
            last_applied_board: self.last_applied_board.clone(),
            final_turn: self.final_turn,
            final_turn_known: self.final_turn_known,
            opp_loss_total: self.opp_loss_total,
            opp_moves_measured: self.opp_moves_measured,
            message: format!(
                "Before: {}",
                self.suggestions[self.selected_suggestion].word
            ),
        });

        let mv = self.suggestions[self.selected_suggestion].clone();

        // Track who empties the bag. While the bag has tiles the opponent
        // holds a full rack, so bag = unseen - RACK_SIZE. If this play draws
        // the last tile we get the final move; otherwise the opponent's next
        // play is the one that could empty it, which would give them the
        // final move.
        let bag_before = self
            .state
            .unseen_tile_count()
            .saturating_sub(crate::tile::RACK_SIZE);
        if bag_before > 0 {
            self.final_turn = if mv.tiles_used.len() >= bag_before {
                FinalTurn::Us
            } else {
                FinalTurn::Opponent
            };
            self.final_turn_known = true;
        }

        self.state.apply_move(&mv);
        self.our_score += mv.score;
        self.last_applied_board = Some(self.state.board.clone());
        let diff = self.our_score as i32 - self.opp_score as i32;
        let diff_str = if diff >= 0 {
            format!("+{}", diff)
        } else {
            format!("{}", diff)
        };
        self.message = Some(match &mv.kind {
            crate::solver::MoveKind::Play => format!(
                "Applied: {} ({} pts) \u{2014} Score: {}-{} ({})",
                mv.word, mv.score, self.our_score, self.opp_score, diff_str
            ),
            crate::solver::MoveKind::Exchange(tiles) => format!(
                "Exchanged {} tiles \u{2014} type the new tiles into the rack",
                tiles.len()
            ),
            crate::solver::MoveKind::Pass => {
                format!(
                    "Passed \u{2014} Score: {}-{} ({})",
                    self.our_score, self.opp_score, diff_str
                )
            }
        });
        self.clear_suggestions();
    }

    /// Undo the last applied move, restoring the previous board, rack, scores,
    /// and board snapshot (so opponent auto-scoring works against the correct state).
    pub fn undo(&mut self) {
        if let Some(undo_state) = self.undo_stack.pop() {
            self.state = undo_state.state;
            self.our_score = undo_state.our_score;
            self.opp_score = undo_state.opp_score;
            self.last_applied_board = undo_state.last_applied_board;
            self.final_turn = undo_state.final_turn;
            self.final_turn_known = undo_state.final_turn_known;
            self.opp_loss_total = undo_state.opp_loss_total;
            self.opp_moves_measured = undo_state.opp_moves_measured;
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
        self.final_turn = FinalTurn::default();
        self.final_turn_known = false;
        self.opp_loss_total = 0.0;
        self.opp_moves_measured = 0;
        self.message = Some("Reset complete".to_string());
    }

    /// Cycle input focus to the next pane.
    pub fn next_focus(&mut self) {
        self.focus = self.focus.next();
    }

    /// Advance to the next sort mode, re-sort suggestions, and persist.
    pub fn cycle_sort_mode(&mut self) {
        let has_sim = self.sim_results.is_some();
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
            SortMode::MctsEquity | SortMode::WinPct => {
                let by_win = self.sort_mode == SortMode::WinPct;
                if let Some(ref results) = self.sim_results {
                    // Candidates pruned early carry a short-run estimate;
                    // rank them after those that received the full budget.
                    let full = results.iter().map(|r| r.iterations).max().unwrap_or(0);
                    let sim_map: std::collections::HashMap<_, _> = results
                        .iter()
                        .map(|r| {
                            let key = (r.word.to_uppercase(), r.row, r.col, r.direction);
                            let tier = if r.iterations == full { 1.0 } else { 0.0 };
                            (key, (tier, if by_win { r.win_pct } else { r.sim_equity }))
                        })
                        .collect();

                    self.suggestions.sort_by(|a, b| {
                        let a_sim =
                            sim_map.get(&(a.word.to_uppercase(), a.row, a.col, a.direction));
                        let b_sim =
                            sim_map.get(&(b.word.to_uppercase(), b.row, b.col, b.direction));

                        match (a_sim, b_sim) {
                            (Some(a_v), Some(b_v)) => {
                                b_v.partial_cmp(a_v).unwrap_or(std::cmp::Ordering::Equal)
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
                    // No simulation results — fall back to equity sort
                    self.suggestions.sort_by(|a, b| {
                        b.equity
                            .partial_cmp(&a.equity)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }
            SortMode::Score => {
                self.suggestions.sort_by_key(|m| std::cmp::Reverse(m.score));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::SolverState;

    /// Build an App without touching the user's settings file. Callers
    /// must hold `RefEngine test lock for as long as the App may use RefEngine
    /// (construction and any inference it starts).
    fn test_app() -> App {
        App::with_settings(Settings::default())
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
            state.place_tile(i / BOARD_SIZE, i % BOARD_SIZE, ch, false);
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
        let _lock = crate::datafiles::test_data::refengine_lock();
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
        let _lock = crate::datafiles::test_data::refengine_lock();
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
        app.state.place_tile(0, 0, 'C', false);
        app.state.place_tile(0, 1, 'A', false);
        app.state.place_tile(0, 2, 'T', false);
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
        let _lock = crate::datafiles::test_data::refengine_lock();
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
        app.state.place_tile(7, 10, 'S', false);
        app.find_moves();
        app.run_mcts_evaluation();
        let started = std::time::Instant::now();
        while app.has_background_work() && started.elapsed().as_secs() < 60 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.poll_mcts();
        }
        assert!(!app.has_background_work());
        assert!(app.sim_results.is_some());
        if app.refengine_simulator.is_some() {
            assert!(
                app.inferred_opponent_rack.is_some(),
                "inference result never arrived"
            );
        }
    }

    #[test]
    fn our_move_ends_game_depends_on_final_turn() {
        let _lock = crate::datafiles::test_data::refengine_lock();
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
}
