//! Solving and evaluation: move generation, the simulation and endgame
//! jobs, their cancellation, and polling their results.

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::endgame::{EndgameSolver, FinalTurn};
use crate::game::observed_move_loss;
use crate::logging::log;
use crate::sim::{SimSettings, SimSnapshot, Simulation};
use crate::solver::Move;

use super::format::group_thousands;
use super::opponent::OPP_LOSS_SAMPLES;
use super::state::{
    App, Focus, Job, MctsProgress, MessageKind, SimJob, SimProgress, SimResult, SimStatus, SortMode,
};

/// Run `f` on a new thread, catching a panic so it cannot take the app
/// down. A panicking worker sends nothing: its receiver reports
/// `Disconnected`, which the poller turns into a failure message (the
/// panic itself goes to the log through the panic hook).
pub(super) fn spawn_worker<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    thread::spawn(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    });
}

impl App {
    // ── Business logic ──────────────────────────────────────────────

    /// Returns true if any background evaluation is in progress.
    pub fn is_mcts_running(&self) -> bool {
        self.sim_job.is_some() || self.endgame_job.is_some()
    }

    /// Stop any running evaluation and forget its result. Returns true if
    /// one was running.
    pub fn cancel_evaluation(&mut self) -> bool {
        let mut cancelled = false;
        if let Some(job) = self.sim_job.take() {
            job.cancel.store(true, AtomicOrdering::Relaxed);
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
    /// It also marks the game for the next autosave.
    pub(super) fn position_changed(&mut self) {
        self.position_stamp += 1;
        self.mark_dirty();
        if self.cancel_evaluation() {
            log("Evaluation cancelled: the position changed");
        }
    }

    /// Returns true if anything at all is still running in the background,
    /// including opponent-rack inference (which outlives the ranking).
    pub fn has_background_work(&self) -> bool {
        self.is_mcts_running() || self.inference_receiver.is_some()
    }

    /// Show a simulation snapshot: results for every candidate, re-sorted
    /// with the selection kept on the same move.
    pub(super) fn apply_sim_snapshot(
        &mut self,
        candidates: &[Move],
        snap: &SimSnapshot,
        first: bool,
    ) {
        let results: Vec<SimResult> = candidates
            .iter()
            .zip(&snap.arms)
            .map(|(mv, arm)| SimResult::from_arm(mv, arm))
            .collect();
        let selected = self
            .suggestions
            .get(self.selected_suggestion)
            .map(Self::move_key);
        self.sim_results = Some(results);
        if first {
            self.sort_mode = SortMode::MctsEquity;
        }
        self.apply_sort();
        // The first results re-rank the list: start at the top. Later
        // snapshots keep the selection on the same move.
        self.selected_suggestion = selected
            .filter(|_| !first)
            .and_then(|k| self.suggestions.iter().position(|m| Self::move_key(m) == k))
            .unwrap_or(0);
        let progress = SimProgress {
            simulated: snap.simulated,
            total: snap.total,
            playouts: snap.playouts,
            elapsed: snap.elapsed,
            budget_fraction: snap
                .budget
                .map(|b| (snap.run_elapsed.as_secs_f32() / b.as_secs_f32()).min(1.0)),
            done: snap.done,
        };
        self.sim_progress = Some(progress);
        if let Some(p) = self.mcts_progress.as_mut() {
            p.sim = Some(progress);
        }
    }

    /// Poll background channels and incorporate results. Returns true if new results arrived.
    pub fn poll_mcts(&mut self) -> bool {
        // Opponent-rack inference (RefEngine) finishes independently of ranking.
        // A result is only kept if the opponent has not moved again since it
        // was started: a late result describes a rack they no longer hold.
        let mut changed = false;
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
                    changed = true;
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
                            let best = endgame_results
                                .first()
                                .map_or(0.0, |b| b.endgame_value as f32);
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
                                win_se: 0.0,
                                sim_equity: val,
                                spread_se: 0.0,
                                delta: val - best,
                                delta_se: 0.0,
                                ply1_bingo_pct: 0.0,
                                ply2_bingo_pct: 0.0,
                                is_endgame: true,
                                iterations: 0,
                                status: SimStatus::Exact,
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

                    self.set_message(
                        MessageKind::Success,
                        format!(
                            "Endgame: {} moves ranked by score minus opponent's best reply",
                            count
                        ),
                    );
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.endgame_job = None;
                    self.mcts_progress = None;
                    self.set_message(
                        MessageKind::Error,
                        "Endgame evaluation failed (see the log)".to_string(),
                    );
                    return true;
                }
            }
        }

        // Simulation snapshots: the final one, or else the latest live one.
        if let Some(job) = self.sim_job.as_mut() {
            if job.stamp != self.position_stamp {
                log("Dropped a stale simulation (the position changed)");
                job.cancel.store(true, AtomicOrdering::Relaxed);
                self.sim_job = None;
                self.mcts_progress = None;
                return false;
            }
            match job.rx.try_recv() {
                Ok(snap) => {
                    let candidates = Arc::clone(&job.candidates);
                    let first = job.first;
                    self.sim_job = None;
                    self.apply_sim_snapshot(&candidates, &snap, first);
                    self.mcts_progress = None;
                    self.set_message(MessageKind::Success, Self::sim_summary(&snap));
                    log(&Self::sim_summary(&snap));
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    let live = job.live.lock().ok().and_then(|mut slot| slot.take());
                    if let Some(snap) = live {
                        let candidates = Arc::clone(&job.candidates);
                        let first = job.first;
                        job.first = false;
                        self.apply_sim_snapshot(&candidates, &snap, first);
                        return true;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.sim_job = None;
                    self.mcts_progress = None;
                    self.set_message(
                        MessageKind::Error,
                        "Simulation failed (see the log)".to_string(),
                    );
                    return true;
                }
            }
        }
        changed
    }

    /// One-line summary of a finished (or stopped) simulation.
    pub(super) fn sim_summary(snap: &SimSnapshot) -> String {
        format!(
            "Simulated {}/{} plays \u{00b7} {} playouts \u{00b7} {:.1}s",
            group_thousands(snap.simulated as u64),
            group_thousands(snap.total as u64),
            group_thousands(snap.playouts),
            snap.elapsed.as_secs_f32()
        )
    }

    /// The suggestion list ordered by static equity, best first, regardless
    /// of the sort mode on screen: playouts evaluate the top of this list.
    pub(super) fn candidates_by_static_equity(&self) -> Vec<Move> {
        let mut candidates = self.suggestions.clone();
        candidates.sort_by(|a, b| {
            b.equity
                .partial_cmp(&a.equity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.score.cmp(&a.score))
        });
        candidates
    }

    /// Run the solver to generate all legal moves for the current board and rack.
    pub fn find_moves(&mut self) {
        if self.state.rack.is_empty() {
            self.set_message(
                MessageKind::Error,
                "Set your rack first (Tab to rack, type letters)".to_string(),
            );
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
            self.push_undo(format!("Before opp: {}", word));
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
            } else if self.state.bag_size() > 0 {
                // Passing is a candidate too while the game goes on (the
                // endgame solver adds its own pass).
                all_suggestions.push(Move::pass(&self.state.rack.tiles()));
            }

            self.suggestions = all_suggestions;
            self.selected_suggestion = 0;
            self.sim_results = None;
            self.sim_progress = None;
            self.filter = None;
            self.list_offset.set(0);

            if total_count == 0 {
                self.set_message(MessageKind::Error, "No valid moves found".to_string());
            } else if is_last_move {
                self.set_message(
                    MessageKind::Info,
                    format!(
                        "Final move of the game: {} plays, sorted by score",
                        total_count
                    ),
                );
            } else {
                self.set_message(
                    MessageKind::Info,
                    format!(
                        "Found {} moves",
                        group_thousands(self.suggestions.len() as u64)
                    ),
                );
            }
        } else {
            self.set_message(MessageKind::Error, "Dictionary not loaded".to_string());
        }
    }

    /// Launch the evaluation for the current position on a background
    /// thread. Every phase is judged under Crossplay rules by the native
    /// engines; RefEngine only infers the opponent's likely rack.
    pub fn run_mcts_evaluation(&mut self) {
        // A new request replaces a running evaluation.
        self.cancel_evaluation();

        if self.suggestions.is_empty() {
            self.set_message(
                MessageKind::Error,
                "Generate suggestions first (Ctrl+S)".to_string(),
            );
            return;
        }

        let Some(dict) = self.dictionary.clone() else {
            self.set_message(MessageKind::Error, "Dictionary not loaded".to_string());
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
            self.set_message(
                MessageKind::Info,
                format!(
                    "Final move of the game: {} plays, sorted by score",
                    self.suggestions.len()
                ),
            );
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
                started: Instant::now(),
                sim: None,
            });
            let caveat = if self.final_turn_known {
                ""
            } else {
                " — assuming they emptied the bag; Ctrl+E if you did"
            };
            self.set_message(
                MessageKind::Progress,
                format!(
                    "Endgame: searching the opponent's replies ({} tiles){}...",
                    unseen, caveat
                ),
            );

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
                    FinalTurn::Opponent,
                    Some(&cancel),
                ) {
                    let _ = tx.send(results);
                }
            });
            return;
        }

        // Bag not empty: simulate every candidate (plays, exchanges, pass)
        // under Crossplay rules. The playout depth follows the bag: with
        // few tiles left the game ends inside the horizon and the
        // equal-turn ending is played exactly.
        let bag = self.state.bag_size();
        let sim_cfg = &self.settings.simulation;
        let settings = SimSettings::for_bag(
            bag,
            sim_cfg.min_playouts,
            Some(Duration::from_secs(sim_cfg.budget_secs.max(1))),
        )
        .with_score_diff(self.our_score as i32 - self.opp_score as i32);
        let candidates = Arc::new(self.candidates_by_static_equity());
        let count = candidates.len();
        self.mcts_progress = Some(MctsProgress {
            started: Instant::now(),
            sim: None,
        });
        self.set_message(
            MessageKind::Progress,
            format!(
                "{}: simulating {} candidates ({} in bag, {} plies)... Esc stops",
                crate::stats::GamePhase::from_unseen_count(unseen as u32).label(),
                group_thousands(count as u64),
                bag,
                settings.max_plies
            ),
        );
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
        let live = Arc::new(Mutex::new(None));
        self.sim_job = Some(SimJob {
            stamp: self.position_stamp,
            cancel: Arc::clone(&cancel),
            live: Arc::clone(&live),
            rx,
            candidates: Arc::clone(&candidates),
            first: true,
        });
        let cache = Arc::clone(&self.sim_cache);
        let generation = cache.lock().map_or(0, |c| c.generation());
        spawn_worker(move || {
            let inference = fresh_inference
                .and_then(|rx| rx.recv_timeout(Duration::from_secs(3)).ok())
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
            // A saved run for this same position and model continues;
            // anything else starts afresh.
            let resume = cache.lock().ok().and_then(|mut c| c.take(generation));
            let mut sim =
                Simulation::new(settings, &dict, &state, &candidates, model.as_ref(), resume);
            let resumed = sim.snapshot(Duration::ZERO, /* done= */ false).playouts;
            if resumed > 0 {
                log(&format!("Resuming a simulation with {} playouts", resumed));
            }
            let started = Instant::now();
            let last = sim.run(&cancel, &mut |snap| {
                if let Ok(mut slot) = live.lock() {
                    *slot = Some(snap);
                }
            });
            if let Ok(mut c) = cache.lock() {
                c.store(generation, sim.into_state(started.elapsed()));
            }
            let _ = tx.send(last);
        });
    }

    /// Find moves, focus the suggestions pane, and start simulation.
    pub fn find_moves_and_focus(&mut self) {
        self.find_moves();
        if !self.suggestions.is_empty() {
            self.focus = Focus::Suggestions;
            self.run_mcts_evaluation();
        }
    }
}
