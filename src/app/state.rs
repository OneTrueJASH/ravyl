//! App state: the `App` struct, its enums and records, construction,
//! and the small state toggles.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::board::{Board, Direction};
use crate::dictionary::Dictionary;
use crate::endgame::{EndgameMove, FinalTurn};
use crate::game::SolverState;
use crate::logging::log;
use crate::refengine_mcts::InferenceResult;
use crate::settings::Settings;
use crate::sim::{ArmStatus, ArmSummary, SimSnapshot, SimState};
use crate::solver::Move;
use crate::ui::menu_popup::EscapeMenuOption;

use super::format::group_thousands;
use super::session::SaveAsPurpose;

/// Which player's score is being edited
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreTarget {
    Player,
    Opponent,
}

/// What a message-bar message is, which decides its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageKind {
    #[default]
    Info,
    Success,
    Error,
    Progress,
}

/// Where a suggestion stands in an evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimStatus {
    /// Exact value from the endgame solver.
    Exact,
    /// Still being simulated.
    Racing,
    /// Clearly behind; playouts stopped at `iterations`.
    Stopped,
    /// Reached the per-candidate playout cap.
    Capped,
    /// Not simulated yet, or below the playout floor.
    Pending,
}

impl SimStatus {
    pub(super) fn from_arm(status: ArmStatus) -> Self {
        match status {
            ArmStatus::Pending => SimStatus::Pending,
            ArmStatus::Racing => SimStatus::Racing,
            ArmStatus::Stopped => SimStatus::Stopped,
            ArmStatus::Capped => SimStatus::Capped,
        }
    }
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
    /// Standard error of `win_pct`, in percentage points.
    pub win_se: f32,
    /// Expected final spread change (playouts) or exact 2-ply value (endgame)
    pub sim_equity: f32,
    /// Standard error of `sim_equity`.
    pub spread_se: f32,
    /// Spread minus the best candidate's, paired over common playouts
    /// (exact for endgame lines), and its standard error.
    pub delta: f32,
    pub delta_se: f32,
    /// Percent of playouts in which the opponent's reply was a bingo
    pub ply1_bingo_pct: f32,
    /// Percent of playouts in which our follow-up was a bingo
    pub ply2_bingo_pct: f32,
    /// From the endgame solver (exact) rather than playouts
    pub is_endgame: bool,
    /// Playouts behind the estimate (0 for exact endgame lines)
    pub iterations: usize,
    pub status: SimStatus,
}

impl SimResult {
    /// A display record for `mv` from a simulation arm.
    pub(super) fn from_arm(mv: &Move, arm: &ArmSummary) -> Self {
        SimResult {
            word: mv.word.clone(),
            row: mv.row,
            col: mv.col,
            direction: mv.direction,
            win_pct: arm.win * 100.0,
            win_se: arm.win_se * 100.0,
            sim_equity: arm.spread,
            spread_se: arm.spread_se,
            delta: arm.delta,
            delta_se: arm.delta_se,
            ply1_bingo_pct: arm.opp_bingo * 100.0,
            ply2_bingo_pct: arm.our_bingo * 100.0,
            is_endgame: false,
            iterations: arm.n,
            status: SimStatus::from_arm(arm.status),
        }
    }

    /// True when there is a real estimate to show.
    pub fn is_simulated(&self) -> bool {
        self.is_endgame || self.iterations > 0
    }
}

/// Progress of a running simulation, for the header and message bar.
#[derive(Debug, Clone, Copy)]
pub struct SimProgress {
    /// Candidates with at least one playout.
    pub simulated: usize,
    /// Candidates in total.
    pub total: usize,
    pub playouts: u64,
    /// Time spent on this position, including resumed runs.
    pub elapsed: Duration,
    /// This run's share of its budget (0-1), when it has one.
    pub budget_fraction: Option<f32>,
    pub done: bool,
}

/// The simulation resume cache, tagged with a generation so a worker can
/// only store a run for the game it was started in.
///
/// Cancelling a run does not wait for its worker: the worker notices the
/// flag, leaves `Simulation::run` and stores its state on the way out. When
/// the open game is cleared in between, that late store would put the old
/// game's run into the new game's cache. `clear` bumps the generation and
/// workers store only under the generation they started with; both happen
/// under the cache's mutex, so no clear can slip between check and store.
#[derive(Debug, Default)]
pub struct SimCache {
    generation: u64,
    state: Option<SimState>,
}

impl SimCache {
    /// The generation a worker started now belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The saved run, if one was stored in `generation`.
    pub fn take(&mut self, generation: u64) -> Option<SimState> {
        if generation == self.generation {
            self.state.take()
        } else {
            None
        }
    }

    /// Save a worker's run unless the cache was cleared since it started.
    pub fn store(&mut self, generation: u64, state: SimState) {
        if generation == self.generation {
            self.state = Some(state);
        }
    }

    /// Drop the saved run and refuse stores from workers started earlier.
    pub fn clear(&mut self) {
        self.generation += 1;
        self.state = None;
    }

    pub fn state(&self) -> Option<&SimState> {
        self.state.as_ref()
    }
}

/// Dialog state for popup interactions
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DialogState {
    #[default]
    None,
    Help,
    EscapeMenu(EscapeMenuOption),
    ConfirmQuit {
        yes: bool,
    },
    ConfirmReset {
        yes: bool,
    },
    /// Numeric input for setting a player's cumulative score
    ScoreInput {
        target: ScoreTarget,
        buffer: String,
    },
    /// Coordinate entry of a play: `8D QUIXOTIC` (lowercase = blank).
    QuickEntry {
        buffer: String,
    },
    /// The saved games (`App::picker_games`); `startup` when shown at
    /// launch.
    GamePicker {
        selected: usize,
        startup: bool,
    },
    /// "Save game as": a name for the open game (or, from the picker, a
    /// new name for a saved one). `pristine` while the buffer still holds
    /// an automatic name, which the first key typed replaces.
    SaveAs {
        buffer: String,
        purpose: SaveAsPurpose,
        pristine: bool,
    },
    /// Saving as `name` would replace another saved game: Yes replaces it,
    /// No returns to the prompt.
    ConfirmOverwrite {
        name: String,
        purpose: SaveAsPurpose,
        yes: bool,
    },
    /// Delete a saved game from the picker (then back to the picker).
    ConfirmDelete {
        slug: String,
        name: String,
        startup: bool,
        yes: bool,
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

    pub fn prev(&self) -> Focus {
        match self {
            Focus::Board => Focus::Suggestions,
            Focus::Rack => Focus::Board,
            Focus::Suggestions => Focus::Rack,
        }
    }

    /// The previous pane before this one that `visible` accepts.
    pub fn prev_visible(&self, visible: impl Fn(Focus) -> bool) -> Focus {
        let mut f = self.prev();
        while f != Focus::Board && !visible(f) {
            f = f.prev();
        }
        f
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
    /// Playouts behind each estimate.
    Playouts,
    /// Our follow-up bingo rate.
    Bingo,
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
            SortMode::WinPct => SortMode::Playouts,
            SortMode::Playouts => SortMode::Bingo,
            SortMode::Bingo => SortMode::Score,
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
            SortMode::Playouts => "n",
            SortMode::Bingo => "Bingo",
        }
    }
}

/// Saved state for undo
#[derive(Clone)]
pub(super) struct UndoState {
    pub(super) state: SolverState,
    pub(super) our_score: u32,
    pub(super) opp_score: u32,
    pub(super) last_applied_board: Option<Board>,
    pub(super) final_turn: FinalTurn,
    pub(super) final_turn_known: bool,
    pub(super) opp_loss_total: f32,
    pub(super) opp_moves_measured: u32,
    pub(super) awaiting_opp_reply: bool,
    pub(super) message: String,
}

/// A background evaluation: its result channel, the position stamp it was
/// started for, and the flag that stops it.
pub(super) struct Job<T> {
    pub(super) stamp: u64,
    pub(super) rx: Receiver<T>,
    pub(super) cancel: Arc<AtomicBool>,
}

impl<T> Job<T> {
    pub(super) fn cancel(&self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}

/// A running simulation: live snapshots land in `live` (latest wins), the
/// final one comes through `rx` (a worker that dies drops its sender, which
/// the poller reports as a failure).
pub(super) struct SimJob {
    pub(super) stamp: u64,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) live: Arc<Mutex<Option<SimSnapshot>>>,
    pub(super) rx: Receiver<SimSnapshot>,
    /// The simulated candidates, in the order of the snapshots' arms.
    pub(super) candidates: Arc<Vec<Move>>,
    /// No snapshot applied yet (the first one switches to the Sim sort).
    pub(super) first: bool,
}

/// MCTS progress state
pub struct MctsProgress {
    /// When the evaluation started; drives the spinner.
    pub started: Instant,
    /// Live progress of a simulation (playout path only).
    pub sim: Option<SimProgress>,
}

impl MctsProgress {
    /// Returns the spinner character for the current animation frame.
    pub fn spinner_char(&self) -> char {
        const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
        let frame = (self.started.elapsed().as_millis() / 120) as usize;
        SPINNER[frame % SPINNER.len()]
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
    /// While a simulation runs, keep the selection on the leader (row 1)
    /// until the user moves it; then it follows the move they picked.
    pub(super) follow_leader: bool,
    pub sort_mode: SortMode,
    pub message: Option<String>,
    /// How the message bar colors `message`.
    pub message_kind: MessageKind,
    pub(super) undo_stack: Vec<UndoState>,
    /// States undone, newest last; cleared by any new undoable change.
    pub(super) redo_stack: Vec<UndoState>,
    /// Text filter on the suggestions list (`/`), and whether it is being
    /// typed.
    pub filter: Option<String>,
    pub filter_editing: bool,
    /// First suggestions row shown, kept between frames (the list scrolls
    /// only as far as needed to keep the selection in view).
    pub list_offset: Cell<usize>,
    /// Rows the suggestions table showed last frame (for paging).
    pub list_height: Cell<usize>,
    /// Text for the system clipboard (OSC 52), written by the main loop.
    pub pending_clipboard: Option<String>,
    /// First line shown of the one-column help.
    pub help_scroll: u16,
    /// How far the help can scroll at the current size (set while drawing).
    pub help_max_scroll: Cell<u16>,
    /// Evaluation results for the current suggestions (playouts or endgame).
    pub sim_results: Option<Vec<SimResult>>,
    /// For each suggestion, the index of its entry in `sim_results`, as
    /// of the last sort (the table checks each entry before using it).
    pub sim_index: Vec<Option<usize>>,
    pub(super) sim_job: Option<SimJob>,
    /// Saved state of the last simulation run, so M on the same position
    /// continues where it stopped.
    pub(super) sim_cache: Arc<Mutex<SimCache>>,
    /// The in-flight inference, tagged with the opponent-move stamp it was
    /// started for so a late result for an old move is dropped.
    pub(super) inference_receiver: Option<(u64, Receiver<Option<InferenceResult>>)>,
    pub(super) endgame_job: Option<Job<Vec<EndgameMove>>>,
    /// Bumped on every change to the position (board, rack, scores, final
    /// turn, apply, undo, reset, re-solve). Evaluation results carry the
    /// stamp they were started under and are dropped if it moved on.
    pub(super) position_stamp: u64,
    pub mcts_progress: Option<MctsProgress>,
    /// Coverage of the last simulation on this position (kept after it
    /// ends, for the suggestions header).
    pub sim_progress: Option<SimProgress>,
    pub dialog: DialogState,
    pub should_quit: bool,
    pub board_tab: BoardTab,
    pub log_scroll: usize,
    pub sidebar_visible: bool,
    pub inferred_opponent_rack: Option<InferenceResult>,
    pub(super) last_applied_board: Option<Board>,
    /// Opponent tiles detected during auto-scoring in find_moves(), saved for
    /// RefEngine inference which runs after the board snapshot is updated.
    pub(super) pending_opponent_tiles: Option<String>,
    /// Board state before the opponent played, saved alongside pending_opponent_tiles.
    pub(super) pre_opponent_board: Option<Board>,
    /// Score of the detected opponent move, for inference.
    pub(super) pending_opponent_score: u32,
    /// True after we apply a move until the next solve: if that solve finds
    /// no new tiles, the opponent passed or exchanged.
    pub(super) awaiting_opp_reply: bool,
    /// Last click on a suggestion row, for double-click detection.
    pub(super) last_row_click: Option<(usize, Instant)>,
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
    pub(super) opp_move_stamp: u64,
    /// Where saved games live; `None` turns saving off (tests, unless they
    /// set a temporary directory).
    pub games_dir: Option<PathBuf>,
    /// The open game's display name; `None` until a new game is first
    /// saved (it then gets an automatic `Untitled …` name). This window
    /// holds the game's lock while it is open.
    pub game_name: Option<String>,
    /// The position changed since the last save.
    pub(super) game_dirty: bool,
    /// When the game was last saved, for the autosave throttle.
    pub(super) last_autosave: Option<Instant>,
    /// The game picker's list, loaded when it opens.
    pub picker_games: Vec<crate::games::GameEntry>,
    /// Last click on a picker row, for double-click detection.
    pub(super) last_picker_click: Option<(usize, Instant)>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Create a new App with default settings loaded from disk.
    pub fn new() -> Self {
        Self::with_settings(Settings::load())
    }

    /// Create a new App with the given settings.
    pub fn with_settings(settings: Settings) -> Self {
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
            follow_leader: false,
            sort_mode,
            message: Some("Loading dictionary...".to_string()),
            message_kind: MessageKind::Progress,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            filter: None,
            filter_editing: false,
            list_offset: Cell::new(0),
            list_height: Cell::new(10),
            pending_clipboard: None,
            help_scroll: 0,
            help_max_scroll: Cell::new(0),
            sim_results: None,
            sim_index: Vec::new(),
            sim_job: None,
            sim_cache: Arc::new(Mutex::new(SimCache::default())),
            inference_receiver: None,
            endgame_job: None,
            position_stamp: 0,
            mcts_progress: None,
            sim_progress: None,
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
            games_dir: None,
            game_name: None,
            game_dirty: false,
            last_autosave: None,
            picker_games: Vec::new(),
            last_picker_click: None,
        }
    }

    /// Show `text` in the message bar.
    pub fn set_message(&mut self, kind: MessageKind, text: impl Into<String>) {
        self.message = Some(text.into());
        self.message_kind = kind;
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
        self.help_scroll = 0;
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
        self.dialog = DialogState::ConfirmReset { yes: false };
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

    /// Persist current settings to disk. Logs errors silently.
    pub(super) fn save_settings(&mut self) {
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
        self.set_message(
            MessageKind::Info,
            if self.sidebar_visible {
                "Sidebar shown".to_string()
            } else {
                "Sidebar hidden (\\ restores it)".to_string()
            },
        );
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
        self.set_message(
            MessageKind::Info,
            format!(
                "Loaded {} words \u{00b7} F1 for help",
                group_thousands(count as u64)
            ),
        );
    }

    /// Flip who has the final move of the game (see [`FinalTurn`]).
    pub fn toggle_final_turn(&mut self) {
        self.final_turn = self.final_turn.toggle();
        self.final_turn_known = true;
        self.clear_suggestions();
        self.set_message(
            MessageKind::Info,
            match self.final_turn {
                FinalTurn::Opponent => {
                    "Final turn: opponent (they emptied the bag — they reply to your play)"
                        .to_string()
                }
                FinalTurn::Us => {
                    "Final turn: you (you emptied the bag — no reply after your play)".to_string()
                }
            },
        );
    }

    /// True when our next play is the last move of the game: either the
    /// opponent has no tiles, or the bag is empty and we emptied it (the
    /// opponent has already taken their last turn).
    pub fn our_move_ends_game(&self) -> bool {
        let unseen = self.state.unseen_tile_count();
        unseen == 0 || (self.state.is_endgame() && self.final_turn == FinalTurn::Us)
    }
}
