//! The opponent: detecting and scoring their move from the board,
//! measuring how strongly they play, and rack inference.

use super::*;

/// An opponent move found by board diff: word, score, and placed tiles.
pub(super) type DetectedOpponentMove = (String, u32, Vec<(usize, usize, Tile)>);

/// Cap on the playout opponent's softmax temperature: beyond roughly the
/// tempo of a turn the playouts stop resembling the game.
pub(super) const MAX_OPP_TEMPERATURE: f32 = 12.0;

/// How many rack completions each observed opponent move is measured
/// against. The measurement runs synchronously in Ctrl+S: typically some
/// tens of milliseconds, with a rare slow tail when sampled completions
/// hold blanks. Kept on this thread because the running total feeds undo
/// (snapshotted in the same push as the auto-score); moving it to a
/// background thread would need its own staleness accounting.
pub(super) const OPP_LOSS_SAMPLES: usize = 10;

/// Fitted by `study_loss_calibration` (mcts/studies.rs, 120 games per point): the
/// estimator reads `measured ~= 7.78 + 0.557 * tau` for a softmax-`tau`
/// opponent. The baseline exists because a sampled completion of their
/// rack usually admits a better play than the tiles they really held, so
/// even a perfect opponent measures near 8 points of "loss".
pub(super) const OPP_LOSS_BASELINE: f32 = 7.78;

pub(super) const OPP_LOSS_PER_TEMPERATURE: f32 = 0.557;

/// From the same study, a softmax-`tau` opponent's true mean equity loss
/// per move is about `0.66 * tau`; used for the display estimate.
pub(super) const OPP_TRUE_LOSS_PER_TEMPERATURE: f32 = 0.66;

impl App {
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

    pub(super) fn detect_opponent_tiles(&self) -> Option<String> {
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
    pub(super) fn score_opponent_move(&self) -> Option<DetectedOpponentMove> {
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

    /// Build the playout opponent model from an inference result and the
    /// measured opponent strength. `None` when there is nothing to model
    /// (no marginals and a perfect-greedy temperature).
    pub(super) fn opponent_model(
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
    pub(super) fn start_inference(&mut self) -> Option<Receiver<Option<InferenceResult>>> {
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
}
