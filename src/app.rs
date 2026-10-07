//! Central application state, event handling, and business logic.
//!
//! Contains the [`App`] struct which owns all game state, dialog state,
//! and coordinates between the solver, MCTS engine, and UI.

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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

    /// The next pane after this one that `visible` accepts. The board is
    /// always visible, so this terminates.
    pub fn next_visible(&self, visible: impl Fn(Focus) -> bool) -> Focus {
        let mut f = self.next();
        while f != Focus::Board && !visible(f) {
            f = f.next();
        }
        f
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
    awaiting_opp_reply: bool,
    message: String,
}

/// A background evaluation: its result channel, the position stamp it was
/// started for, and the flag that stops it.
struct Job<T> {
    stamp: u64,
    rx: Receiver<T>,
    cancel: Arc<AtomicBool>,
}

impl<T> Job<T> {
    fn cancel(&self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}

/// Run `f` on a new thread, catching a panic so it cannot take the app
/// down. A panicking worker sends nothing: its receiver reports
/// `Disconnected`, which the poller turns into a failure message (the
/// panic itself goes to the log through the panic hook).
fn spawn_worker<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    thread::spawn(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    });
}

/// Two clicks on the same suggestion row within this window apply it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

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
    playout_job: Option<Job<Vec<MctsResult>>>,
    /// The in-flight inference, tagged with the opponent-move stamp it was
    /// started for so a late result for an old move is dropped.
    inference_receiver: Option<(u64, Receiver<Option<InferenceResult>>)>,
    endgame_job: Option<Job<Vec<EndgameMove>>>,
    /// Bumped on every change to the position (board, rack, scores, final
    /// turn, apply, undo, reset, re-solve). Evaluation results carry the
    /// stamp they were started under and are dropped if it moved on.
    position_stamp: u64,
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
    /// Score of the detected opponent move, for inference.
    pending_opponent_score: u32,
    /// True after we apply a move until the next solve: if that solve finds
    /// no new tiles, the opponent passed or exchanged.
    awaiting_opp_reply: bool,
    /// Last click on a suggestion row, for double-click detection.
    last_row_click: Option<(usize, Instant)>,
    /// Terminal size from the last frame, so focus cycling can skip panes
    /// the layout does not show. `None` until the first frame.
    pub terminal_size: Option<(u16, u16)>,
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
            playout_job: None,
            inference_receiver: None,
            endgame_job: None,
            position_stamp: 0,
            mcts_progress: None,
            refengine_simulator,
            refengine_unavailable,
            inferred_opponent_rack: None,
            last_applied_board: None,
            pending_opponent_tiles: None,
            pre_opponent_board: None,
            pending_opponent_score: 0,
            awaiting_opp_reply: false,
            last_row_click: None,
            terminal_size: None,
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
        if self.has_dialog() {
            return self.handle_dialog_mouse(mouse);
        }
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

    /// Mouse input while a dialog is open: only the dialog reacts. A click
    /// on a button acts on it, a click elsewhere inside the dialog does
    /// nothing, and a click outside it cancels the dialog.
    fn handle_dialog_mouse(&mut self, mouse: MouseEvent) -> AppAction {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return AppAction::Nothing;
        }
        match resolve_click(mouse.column, mouse.row) {
            Some(HitTarget::DialogYes) => match self.dialog {
                DialogState::ConfirmQuit(_) => {
                    self.close_dialog();
                    return AppAction::Quit;
                }
                DialogState::ConfirmReset(_) => {
                    self.close_dialog();
                    self.reset();
                }
                _ => {}
            },
            Some(HitTarget::DialogNo) => self.close_dialog(),
            Some(HitTarget::MenuItem(option)) => {
                if matches!(self.dialog, DialogState::EscapeMenu(_)) {
                    self.choose_menu_option(option);
                }
            }
            Some(HitTarget::DialogArea) => {}
            _ => self.close_dialog(),
        }
        AppAction::Redraw
    }

    /// Act on an escape-menu choice (Enter or a click).
    fn choose_menu_option(&mut self, option: EscapeMenuOption) {
        match option {
            EscapeMenuOption::Quit => self.dialog = DialogState::ConfirmQuit(false),
            EscapeMenuOption::Reset => self.dialog = DialogState::ConfirmReset(false),
            EscapeMenuOption::Cancel => self.close_dialog(),
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
                let now = Instant::now();
                let double = self.focus == Focus::Suggestions
                    && self.selected_suggestion == idx
                    && self
                        .last_row_click
                        .is_some_and(|(row, at)| row == idx && now - at <= DOUBLE_CLICK);
                if double {
                    // A real double-click: apply the move.
                    self.last_row_click = None;
                    self.apply_suggestion_and_focus();
                } else {
                    self.focus = Focus::Suggestions;
                    self.selected_suggestion = idx.min(self.suggestions.len().saturating_sub(1));
                    self.last_row_click = Some((idx, now));
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
                self.save_settings();
            }
            HitTarget::FocusPane(focus) => {
                self.focus = focus;
            }
            HitTarget::DialogYes
            | HitTarget::DialogNo
            | HitTarget::DialogArea
            | HitTarget::MenuItem(_) => {}
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
                KeyCode::Enter => self.choose_menu_option(selected),
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
        self.playout_job.is_some() || self.endgame_job.is_some()
    }

    /// Stop any running evaluation and forget its result. Returns true if
    /// one was running.
    pub fn cancel_evaluation(&mut self) -> bool {
        let mut cancelled = false;
        if let Some(job) = self.playout_job.take() {
            job.cancel();
            cancelled = true;
        }
        if let Some(job) = self.endgame_job.take() {
            job.cancel();
            cancelled = true;
        }
        self.mcts_progress = None;
        cancelled
    }

    /// The position changed: results computed for the old one are stale.
    fn position_changed(&mut self) {
        self.position_stamp += 1;
        if self.cancel_evaluation() {
            log("Evaluation cancelled: the position changed");
        }
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
                        if let Some(n) = result.as_ref().and_then(|r| r.total_racks) {
                            log(&format!(
                                "Inference: {} racks consistent with their play",
                                n
                            ));
                        }
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
        if let Some(ref job) = self.endgame_job {
            match job.rx.try_recv() {
                Ok(_) if job.stamp != self.position_stamp => {
                    log("Dropped a stale endgame result (the position changed)");
                    self.endgame_job = None;
                    self.mcts_progress = None;
                }
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
                    self.sort_mode = SortMode::MctsEquity;
                    self.apply_sort();
                    self.selected_suggestion = 0;
                    self.endgame_job = None;
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
                    self.endgame_job = None;
                    self.mcts_progress = None;
                    self.message = Some("Endgame evaluation failed (see the log)".to_string());
                    return true;
                }
            }
        }

        // Playout results.
        if let Some(ref job) = self.playout_job {
            match job.rx.try_recv() {
                Ok(_) if job.stamp != self.position_stamp => {
                    log("Dropped a stale playout result (the position changed)");
                    self.playout_job = None;
                    self.mcts_progress = None;
                }
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
                    self.selected_suggestion = 0;
                    self.sort_mode = SortMode::MctsEquity;
                    self.apply_sort();
                    self.playout_job = None;
                    self.mcts_progress = None;

                    let full = results.iter().map(|r| r.iterations).max().unwrap_or(0);
                    let shortened = results.iter().any(|r| r.iterations < full);
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
                    self.playout_job = None;
                    self.mcts_progress = None;
                    self.message = Some("Playout evaluation failed (see the log)".to_string());
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
        let snapshot = self.undo_snapshot(format!("Score: {}-{}", self.our_score, self.opp_score));
        self.undo_stack.push(snapshot);
        // Win estimates depend on the score difference.
        self.position_changed();
        self.sim_results = None;

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
        self.ensure_focus_visible();
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
        self.position_changed();
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

        // A re-solve replaces whatever was being evaluated.
        self.position_changed();
        // One log section per solve keeps the log viewer readable.
        crate::logging::start_section();

        // Auto-score the opponent's move if new tiles appeared since our last play.
        // Save the opponent tiles and pre-opponent board BEFORE updating the
        // snapshot, so RefEngine inference can still detect them later.
        self.pending_opponent_tiles = self.detect_opponent_tiles();
        self.pre_opponent_board = self.last_applied_board.clone();

        let opponent_move = self.score_opponent_move();
        if opponent_move.is_none() && self.awaiting_opp_reply {
            // We moved, and the opponent's turn put nothing on the board:
            // they passed or exchanged. Whatever inference said about the
            // tiles they kept no longer describes their rack.
            self.opp_move_stamp += 1;
            if self.inferred_opponent_rack.take().is_some() {
                log("Opponent passed or exchanged: cleared the rack inference");
            }
        }
        self.awaiting_opp_reply = false;

        if let Some((word, score, placed)) = opponent_move {
            // Push undo state so the auto-score is reversible
            let snapshot = self.undo_snapshot(format!("Before opp: {}", word));
            self.undo_stack.push(snapshot);
            self.pending_opponent_score = score;
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
                        // RefEngine's `infer` reads played tiles as a rack:
                        // a lowercase letter would be un-blanked into the
                        // natural letter, so a played blank must be '?'.
                        tiles.push(if tile.is_blank { '?' } else { tile.letter });
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
        // A new request replaces a running evaluation.
        self.cancel_evaluation();

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
            // The board as it stands when we evaluate is the reference for
            // detecting the opponent's next move.
            self.last_applied_board = Some(self.state.board.clone());

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
            let cancel = Arc::new(AtomicBool::new(false));
            self.endgame_job = Some(Job {
                stamp: self.position_stamp,
                rx,
                cancel: Arc::clone(&cancel),
            });

            spawn_worker(move || {
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
                if let Some(results) = solver.solve_cancellable(
                    &state.board,
                    &our_rack,
                    &opp_rack,
                    true,
                    Some(&cancel),
                ) {
                    let _ = tx.send(results);
                }
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
        // Snapshot after inference has taken the opponent's tiles: this
        // board is the reference for detecting their next move.
        self.last_applied_board = Some(self.state.board.clone());

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = config.with_cancel(Arc::clone(&cancel));
        self.playout_job = Some(Job {
            stamp: self.position_stamp,
            rx,
            cancel: Arc::clone(&cancel),
        });
        spawn_worker(move || {
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
            if !cancel.load(AtomicOrdering::Relaxed) {
                let _ = tx.send(results);
            }
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
        // RefEngine wants the scores before the opponent's move.
        let move_score = self.pending_opponent_score;
        let scores = (self.our_score, self.opp_score.saturating_sub(move_score));
        let state = self.state.clone();
        let refengine = Arc::clone(refengine);

        let (tx, rx) = mpsc::channel();
        let (tx_eval, rx_eval) = mpsc::channel();
        self.inference_receiver = Some((self.opp_move_stamp, rx));
        spawn_worker(move || {
            let result = refengine.infer_opponent_rack(
                &state,
                &opponent_tiles,
                move_score,
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

        let snapshot = self.undo_snapshot(format!(
            "Before: {}",
            self.suggestions[self.selected_suggestion].word
        ));
        self.undo_stack.push(snapshot);

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
        self.awaiting_opp_reply = true;
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
            self.awaiting_opp_reply = undo_state.awaiting_opp_reply;
            self.clear_suggestions();
            self.message = Some(format!("Undone: {}", undo_state.message));
        } else {
            self.message = Some("Nothing to undo".to_string());
        }
    }

    /// The undo record for the current state.
    fn undo_snapshot(&self, message: String) -> UndoState {
        UndoState {
            state: self.state.clone(),
            our_score: self.our_score,
            opp_score: self.opp_score,
            last_applied_board: self.last_applied_board.clone(),
            final_turn: self.final_turn,
            final_turn_known: self.final_turn_known,
            opp_loss_total: self.opp_loss_total,
            opp_moves_measured: self.opp_moves_measured,
            awaiting_opp_reply: self.awaiting_opp_reply,
            message,
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
        self.pending_opponent_score = 0;
        self.awaiting_opp_reply = false;
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
        self.focus = self.focus.next_visible(|f| self.pane_visible(f));
    }

    /// Whether the current layout shows the pane for `focus` (always true
    /// before the first frame, when the terminal size is unknown).
    pub fn pane_visible(&self, focus: Focus) -> bool {
        let Some((w, h)) = self.terminal_size else {
            return true;
        };
        let layout = crate::ui::layout::AppLayout::compute(
            ratatui::layout::Rect::new(0, 0, w, h),
            self.sidebar_visible,
        );
        match focus {
            Focus::Board => true,
            Focus::Rack => layout.rack_area.is_some(),
            Focus::Suggestions => layout.suggestions_area.is_some(),
        }
    }

    /// Move focus to the board if the focused pane is no longer shown.
    pub fn ensure_focus_visible(&mut self) {
        if !self.pane_visible(self.focus) {
            self.focus = Focus::Board;
        }
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
                    // rank them after those that received the full budget,
                    // and any that never got a playout after both.
                    let full = results.iter().map(|r| r.iterations).max().unwrap_or(0);
                    let sim_map: std::collections::HashMap<_, _> = results
                        .iter()
                        .map(|r| {
                            let key = (r.word.to_uppercase(), r.row, r.col, r.direction);
                            let tier = if r.is_endgame || r.iterations == full {
                                2.0
                            } else if r.iterations > 0 {
                                1.0
                            } else {
                                0.0
                            };
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
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(mut app) = solved_app() else {
            return;
        };
        app.run_mcts_evaluation();
        let flag = Arc::clone(&app.playout_job.as_ref().expect("job started").cancel);
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
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = test_app();
        app.suggestions = vec![fake_move(2)];
        let before = app.suggestions.clone();
        let (tx, rx) = mpsc::channel();
        app.playout_job = Some(Job {
            stamp: app.position_stamp,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        // The position moves on without going through the usual paths
        // (which would also cancel the job), then the old result arrives.
        app.position_stamp += 1;
        tx.send(vec![MctsResult {
            mv: fake_move(3),
            mcts_equity: 99.0,
            win_rate: 1.0,
            iterations: 10,
            opp_bingo_rate: 0.0,
            our_bingo_rate: 0.0,
        }])
        .unwrap();
        app.poll_mcts();
        assert!(app.playout_job.is_none());
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
        let _lock = crate::datafiles::test_data::refengine_lock();
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
        let _lock = crate::datafiles::test_data::refengine_lock();
        let Some(mut app) = solved_app() else {
            return;
        };
        app.run_mcts_evaluation();
        let first = Arc::clone(&app.playout_job.as_ref().unwrap().cancel);
        app.focus = Focus::Suggestions;
        app.handle_key(key(KeyCode::Char('m')));
        assert!(first.load(AtomicOrdering::Relaxed), "first run cancelled");
        let second = &app.playout_job.as_ref().expect("a new run started").cancel;
        assert!(!second.load(AtomicOrdering::Relaxed));
        app.cancel_evaluation();
    }

    /// Candidates that never got a playout show no numbers and sort after
    /// every simulated one, whatever their placeholder values say.
    #[test]
    fn unsimulated_candidates_sort_last() {
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = test_app();
        let mut moves: Vec<Move> = (1..=3).map(fake_move).collect();
        for (i, m) in moves.iter_mut().enumerate() {
            m.row = i;
        }
        app.suggestions = moves.clone();
        let sim = |m: &Move, eq: f32, n: usize| SimResult {
            word: m.word.clone(),
            row: m.row,
            col: m.col,
            direction: m.direction,
            win_pct: 50.0,
            sim_equity: eq,
            ply1_bingo_pct: 0.0,
            ply2_bingo_pct: 0.0,
            is_endgame: false,
            iterations: n,
        };
        app.sim_results = Some(vec![
            sim(&moves[0], 5.0, 100),
            sim(&moves[1], 50.0, 0),
            sim(&moves[2], 10.0, 40),
        ]);
        app.set_sort_mode(SortMode::MctsEquity);
        let order: Vec<usize> = app.suggestions.iter().map(|m| m.row).collect();
        assert_eq!(order, vec![0, 2, 1]);
    }

    /// One click on the selected row only selects; a second click on the
    /// same row within the double-click window applies it.
    #[test]
    fn applying_from_the_list_needs_a_real_double_click() {
        let _lock = crate::datafiles::test_data::refengine_lock();
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
        let _lock = crate::datafiles::test_data::refengine_lock();
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

        // Too narrow for a sidebar at all.
        app.sidebar_visible = true;
        app.terminal_size = Some((80, 24));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Board);
    }

    /// When the opponent's turn adds no tiles (a pass or an exchange), the
    /// stored rack inference no longer applies and is cleared.
    #[test]
    fn opponent_pass_or_exchange_clears_inference() {
        let _lock = crate::datafiles::test_data::refengine_lock();
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
        let _lock = crate::datafiles::test_data::refengine_lock();
        let mut app = test_app();
        let (tx, rx) = mpsc::channel::<Vec<MctsResult>>();
        app.playout_job = Some(Job {
            stamp: app.position_stamp,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        });
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
}
