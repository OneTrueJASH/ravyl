//! Editing the position: board and rack entry, applying moves, undo and
//! redo, the coordinate prompt, the clipboard and the CGP position.

use crate::board::{Board, Direction, BOARD_SIZE};
use crate::endgame::FinalTurn;
use crate::game::SolverState;
use crate::solver::Solver;
use crate::tile::Tile;

use super::state::{App, Focus, MessageKind, ScoreTarget, UndoState};

/// A play entered by coordinates (`:` then `8D QUIXOTIC`), checked against
/// the board and the dictionary.
#[derive(Debug, Clone)]
pub struct QuickPlay {
    /// The new tiles, in word order.
    pub tiles: Vec<(usize, usize, Tile)>,
    /// The main word as it reads on the board.
    pub word: String,
    pub score: u32,
    /// Why the play is not legal, if it is not (words not in the
    /// dictionary, or not connected).
    pub problem: Option<String>,
}

/// Parse `8D` (row first: horizontal) or `D8` (column first: vertical)
/// into (row, col, direction).
pub fn parse_coords(text: &str) -> Option<(usize, usize, Direction)> {
    let t = text.trim().to_ascii_uppercase();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let letters: String = t.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if digits.is_empty() || letters.len() != 1 || digits.len() + 1 != t.len() {
        return None;
    }
    let row: usize = digits.parse().ok()?;
    let col = (letters.as_bytes()[0] - b'A') as usize;
    if row == 0 || row > BOARD_SIZE || col >= BOARD_SIZE {
        return None;
    }
    let dir = if t.starts_with(|c: char| c.is_ascii_digit()) {
        Direction::Horizontal
    } else {
        Direction::Vertical
    };
    Some((row - 1, col, dir))
}

/// Base64 (standard alphabet, padded), for OSC 52 clipboard writes.
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl App {
    /// Commit a score from the input dialog.
    pub(super) fn commit_score_input(&mut self, target: ScoreTarget, value: u32) {
        // Push undo so the score change is reversible
        self.push_undo(format!("Score: {}-{}", self.our_score, self.opp_score));
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
        self.set_message(
            MessageKind::Info,
            format!(
                "{} score set to {} \u{2014} {}-{} ({})",
                label, value, self.our_score, self.opp_score, diff_str
            ),
        );
        self.close_dialog();
    }

    /// Move the board cursor by the given row/column delta.
    pub fn move_cursor(&mut self, dr: i32, dc: i32) {
        let new_row = (self.cursor.0 as i32 + dr).clamp(0, 14) as usize;
        let new_col = (self.cursor.1 as i32 + dc).clamp(0, 14) as usize;
        self.cursor = (new_row, new_col);
    }

    /// Place a letter on the board at the cursor and advance in the typing
    /// direction. `blank` places it as a blank. Typing the letter a square
    /// already holds keeps that tile (and its blank flag); the cursor then
    /// skips over occupied squares, so typing only the new tiles of a word
    /// fills the gaps, and it stops at the edge. `?` toggles the blank flag
    /// of the tile under the cursor (or places a blank A).
    pub fn place_on_board(&mut self, letter: char, blank: bool) {
        let (row, col) = self.cursor;

        if letter == '?' {
            if let Some(tile) = self.state.get_tile_mut(row, col) {
                tile.is_blank = !tile.is_blank;
                self.clear_suggestions();
                self.message = None;
                return;
            }
            self.state.place_tile(row, col, Tile::blank('A'));
        } else {
            match self.state.get_tile(row, col).copied() {
                Some(existing) if existing.letter == letter && !blank => {}
                Some(existing) if existing.letter == letter && existing.is_blank => {}
                _ => {
                    let tile = if blank {
                        Tile::blank(letter)
                    } else {
                        Tile::new(letter)
                    };
                    self.state.place_tile(row, col, tile);
                }
            }
        }

        self.clear_suggestions();
        self.message = None;
        self.advance_cursor();
    }

    /// Step the cursor in the typing direction past any occupied squares,
    /// staying put at the edge of the board.
    pub(super) fn advance_cursor(&mut self) {
        let (dr, dc) = self.typing_direction.delta();
        let (mut r, mut c) = (self.cursor.0 as i32 + dr, self.cursor.1 as i32 + dc);
        while Board::is_valid_position(r, c)
            && self.state.get_tile(r as usize, c as usize).is_some()
        {
            r += dr;
            c += dc;
        }
        if Board::is_valid_position(r, c) {
            self.cursor = (r as usize, c as usize);
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
            self.set_message(MessageKind::Error, "Rack is full (max 7 tiles)".to_string());
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
        if self.state.rack.is_empty() {
            return;
        }
        self.push_undo("Rack cleared".to_string());
        self.state.clear_rack();
        self.clear_suggestions();
        self.set_message(MessageKind::Info, "Rack cleared".to_string());
    }

    pub(super) fn clear_suggestions(&mut self) {
        self.position_changed();
        self.suggestions.clear();
        self.sim_results = None;
        self.sim_progress = None;
        // Intentionally preserve inferred_opponent_rack: inference results
        // are expensive to compute and should persist until replaced by new
        // inference from the next RefEngine simulation.
    }

    /// Apply the currently selected suggestion to the board.
    pub fn apply_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }

        self.push_undo(format!(
            "Before: {}",
            self.suggestions[self.selected_suggestion].word
        ));

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
        self.set_message(
            MessageKind::Success,
            match &mv.kind {
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
            },
        );
        self.clear_suggestions();
    }

    /// Record the current state for undo, before a change; any redo
    /// history is dropped.
    pub(super) fn push_undo(&mut self, message: String) {
        let snapshot = self.undo_snapshot(message);
        self.undo_stack.push(snapshot);
        self.redo_stack.clear();
    }

    /// Put a saved state back.
    pub(super) fn restore(&mut self, saved: UndoState) {
        self.state = saved.state;
        self.our_score = saved.our_score;
        self.opp_score = saved.opp_score;
        self.last_applied_board = saved.last_applied_board;
        self.final_turn = saved.final_turn;
        self.final_turn_known = saved.final_turn_known;
        self.opp_loss_total = saved.opp_loss_total;
        self.opp_moves_measured = saved.opp_moves_measured;
        self.awaiting_opp_reply = saved.awaiting_opp_reply;
        self.clear_suggestions();
    }

    /// Undo the last change (applied move, auto-scored opponent move,
    /// score edit, reset or rack clear), restoring the previous board,
    /// rack, scores, and board snapshot (so opponent auto-scoring works
    /// against the correct state).
    pub fn undo(&mut self) {
        if let Some(saved) = self.undo_stack.pop() {
            let message = saved.message.clone();
            let current = self.undo_snapshot(message.clone());
            self.redo_stack.push(current);
            self.restore(saved);
            self.set_message(MessageKind::Info, format!("Undone: {}", message));
        } else {
            self.set_message(MessageKind::Error, "Nothing to undo");
        }
    }

    /// Redo the last undone change.
    pub fn redo(&mut self) {
        if let Some(saved) = self.redo_stack.pop() {
            let message = saved.message.clone();
            let current = self.undo_snapshot(message.clone());
            self.undo_stack.push(current);
            self.restore(saved);
            self.set_message(MessageKind::Info, format!("Redone: {}", message));
        } else {
            self.set_message(MessageKind::Error, "Nothing to redo");
        }
    }

    /// The undo record for the current state.
    pub(super) fn undo_snapshot(&self, message: String) -> UndoState {
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
        let exchange = self
            .suggestions
            .get(self.selected_suggestion)
            .is_some_and(|m| matches!(m.kind, crate::solver::MoveKind::Exchange(_)));
        self.apply_suggestion();
        // After an exchange the next thing to do is type the new tiles.
        self.focus = if exchange { Focus::Rack } else { Focus::Board };
    }

    /// Reset the board, rack, and all state to initial defaults.
    pub fn reset(&mut self) {
        self.push_undo("Reset".to_string());
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
        self.set_message(MessageKind::Info, "Reset complete".to_string());
    }

    /// Check a coordinate entry against the board: placement, connection
    /// and words. `Err` means it cannot even be placed.
    pub fn parse_quick_entry(&self, text: &str) -> Result<QuickPlay, String> {
        let mut parts = text.split_whitespace();
        let coords = parts
            .next()
            .ok_or("type a square and a word, e.g. 8D QUIXOTIC")?;
        let word = parts.next().ok_or("add the word after the square")?;
        if parts.next().is_some() {
            return Err("one square and one word".to_string());
        }
        let (row, col, dir) = parse_coords(coords).ok_or_else(|| {
            format!(
                "'{}' is not a square: row then column (8D) across, column then row (D8) down",
                coords
            )
        })?;
        let dict = self.dictionary.as_ref().ok_or("dictionary not loaded")?;
        let (dr, dc) = dir.delta();
        let board = &self.state.board;

        let mut tiles = Vec::new();
        for (i, ch) in word.chars().enumerate() {
            let (r, c) = (row as i32 + dr * i as i32, col as i32 + dc * i as i32);
            if !Board::is_valid_position(r, c) {
                return Err("the word runs off the board".to_string());
            }
            let (r, c) = (r as usize, c as usize);
            match board.get_tile(r, c) {
                Some(existing) => {
                    if ch != '.' && !ch.eq_ignore_ascii_case(&existing.letter) {
                        return Err(format!(
                            "{} already holds {}",
                            crate::ui::board_view::square_name(r, c),
                            existing.letter
                        ));
                    }
                }
                None => {
                    if !ch.is_ascii_alphabetic() {
                        return Err(format!("'{}' is not a letter", ch));
                    }
                    let tile = if ch.is_ascii_lowercase() {
                        Tile::blank(ch)
                    } else {
                        Tile::new(ch)
                    };
                    tiles.push((r, c, tile));
                }
            }
        }
        if tiles.is_empty() {
            return Err("no new tiles in that word".to_string());
        }

        // The words the play forms, read off a copy of the board.
        let mut after = board.clone();
        for &(r, c, t) in &tiles {
            after.place_tile(r, c, t);
        }
        let read_word = |b: &Board, r: usize, c: usize, d: Direction| -> String {
            let (dr, dc) = d.delta();
            let (mut sr, mut sc) = (r as i32, c as i32);
            while Board::is_valid_position(sr - dr, sc - dc)
                && b.get_tile((sr - dr) as usize, (sc - dc) as usize).is_some()
            {
                sr -= dr;
                sc -= dc;
            }
            let mut w = String::new();
            while Board::is_valid_position(sr, sc) {
                match b.get_tile(sr as usize, sc as usize) {
                    Some(t) => w.push(t.letter),
                    None => break,
                }
                sr += dr;
                sc += dc;
            }
            w
        };
        let main = read_word(&after, tiles[0].0, tiles[0].1, dir);
        let mut problems = Vec::new();
        if main.len() < 2 {
            problems.push("a single letter is not a word".to_string());
        } else if !dict.is_valid(&main) {
            problems.push(format!("{} is not a word", main));
        }
        let mut touches = false;
        for &(r, c, _) in &tiles {
            let cross = read_word(&after, r, c, dir.perpendicular());
            if cross.len() >= 2 {
                touches = true;
                if !dict.is_valid(&cross) {
                    problems.push(format!("{} is not a word", cross));
                }
            }
        }
        let board_empty =
            (0..BOARD_SIZE).all(|r| (0..BOARD_SIZE).all(|c| board.get_tile(r, c).is_none()));
        if board_empty {
            if !tiles.iter().any(|&(r, c, _)| (r, c) == (7, 7)) {
                problems.push("the first play must cover the centre square".to_string());
            }
        } else if !touches && main.len() == tiles.len() {
            problems.push("the play does not touch the tiles on the board".to_string());
        }

        let score = Solver::new(board, dict).score_placed_tiles(&tiles, dir);
        Ok(QuickPlay {
            tiles,
            word: main,
            score,
            problem: (!problems.is_empty()).then(|| problems.join("; ")),
        })
    }

    /// Place a legal coordinate entry on the board (undoable). The next
    /// solve scores it for the opponent like tiles typed on the board.
    pub(super) fn commit_quick_entry(&mut self, text: &str) {
        match self.parse_quick_entry(text) {
            Ok(play) if play.problem.is_none() => {
                self.push_undo(format!("Entered {}", play.word));
                for &(r, c, t) in &play.tiles {
                    self.state.board.place_tile(r, c, t);
                }
                self.close_dialog();
                self.clear_suggestions();
                self.focus = Focus::Board;
                self.set_message(
                    MessageKind::Success,
                    format!(
                        "Placed {} ({} pts) \u{2014} Ctrl+S scores it for the opponent",
                        play.word, play.score
                    ),
                );
            }
            Ok(play) => {
                let problem = play.problem.unwrap_or_default();
                self.set_message(MessageKind::Error, format!("Not placed: {}", problem));
            }
            Err(e) => self.set_message(MessageKind::Error, format!("Not placed: {}", e)),
        }
    }

    /// Copy the selected move (`8D QUIXOTIC`, blanks lowercase) to the
    /// clipboard.
    pub(super) fn copy_selected_move(&mut self) {
        let Some(mv) = self.suggestions.get(self.selected_suggestion) else {
            return;
        };
        let text = match mv.kind {
            crate::solver::MoveKind::Play => {
                format!(
                    "{} {}",
                    mv.coords(),
                    crate::ui::suggestions::display_word(mv)
                )
            }
            _ => mv.word.clone(),
        };
        self.set_message(MessageKind::Success, format!("Copied {}", text));
        self.pending_clipboard = Some(text);
    }

    /// The position as a CGP line with this app's own opcodes for who has
    /// the final turn (see [`Self::load_position`]).
    pub fn position_cgp(&self) -> String {
        format!(
            "{} cpsolver.final {}; cpsolver.final_known {};",
            self.state.to_cgp((self.our_score, self.opp_score)),
            match self.final_turn {
                FinalTurn::Us => "us",
                FinalTurn::Opponent => "opp",
            },
            u8::from(self.final_turn_known)
        )
    }

    /// Copy the position (CGP) to the clipboard.
    pub(super) fn copy_position(&mut self) {
        self.pending_clipboard = Some(self.position_cgp());
        self.set_message(MessageKind::Success, "Copied the position (CGP)");
    }

    /// Load a CGP position: board, our rack, scores (when given) and, from
    /// a saved game, who has the final turn. The board becomes the
    /// reference for detecting the opponent's next move.
    pub fn load_position(&mut self, text: &str, undoable: bool) -> Result<(), String> {
        let text = text.trim();
        let state = SolverState::from_cgp(text)?;
        let fields: Vec<&str> = text.split_whitespace().collect();
        let scores = fields.get(2).and_then(|f| {
            let (a, b) = f.split_once('/')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        });
        let opcode = |name: &str| -> Option<&str> {
            let at = text.find(name)?;
            text[at + name.len()..]
                .trim_start()
                .split([';', ' '])
                .next()
        };
        if undoable {
            self.push_undo("Loaded a position".to_string());
        }
        self.state = state;
        if let Some((ours, theirs)) = scores {
            self.our_score = ours;
            self.opp_score = theirs;
        }
        match opcode("cpsolver.final ") {
            Some("us") => self.final_turn = FinalTurn::Us,
            Some("opp") => self.final_turn = FinalTurn::Opponent,
            _ => {}
        }
        self.final_turn_known = opcode("cpsolver.final_known ") == Some("1");
        self.last_applied_board = Some(self.state.board.clone());
        self.awaiting_opp_reply = false;
        self.clear_suggestions();
        Ok(())
    }
}
