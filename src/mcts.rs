//! Monte Carlo move evaluation under Crossplay rules.
//!
//! Each candidate move is played out many times against random draws from
//! the unseen tiles. Playouts follow Crossplay's ending exactly:
//!
//! - Players draw after playing; the player who draws the last tile from the
//!   bag "owns" the final turn. The other player then gets one last turn and
//!   the owner plays the final move. Nobody goes out for a bonus and leftover
//!   tiles are never scored.
//! - On a player's last turn (the reply after the bag empties) they pick the
//!   move that maximises their score minus the owner's best final reply,
//!   passing if that is better. On the final turn the owner simply takes the
//!   most points. Both players know each other's racks at that point (the
//!   unseen pool is the other rack), so this is exact given the playout
//!   reached that position.
//! - While the bag still has tiles, players play the best move by static
//!   equity (score plus phase-scaled leave value).
//!
//! If a playout hits the ply limit before the game ends, the position is
//! scored by the spread so far plus the phase-scaled leave values. This is
//! the pre-endgame engine (bag of 1–7 tiles), where RefEngine's simulation
//! would apply Scrabble's go-out rules, and the general fallback when
//! RefEngine is unavailable. It is not meant for positions where the bag is
//! already empty: who owns the final turn is then outside its knowledge,
//! and [`crate::endgame::EndgameSolver`] handles those exactly.

use std::time::{Duration, Instant};

use fastrand::Rng;
use rayon::prelude::*;

use crate::board::Board;
use crate::dictionary::Dictionary;
use crate::game::exchange_moves;
#[cfg(test)]
use crate::game::full_tile_set;
use crate::game::SolverState;
use crate::solver::{Move, MoveKind, Solver, EXCHANGE_MIN_BAG};
use crate::tile::{Rack, Tile, RACK_SIZE};

/// Configuration for Monte Carlo evaluation
#[derive(Debug, Clone)]
pub struct MctsConfig {
    /// Number of top candidates (by static equity) to evaluate
    pub num_candidates: usize,
    /// Playouts per candidate
    pub iterations_per_candidate: usize,
    /// Maximum plies per playout after the candidate move. The final two
    /// turns after the bag empties always run to completion; this bounds
    /// the greedy phase before that.
    pub max_plies: usize,
    /// Use parallel evaluation with Rayon
    pub parallel: bool,
    /// Current score difference (ours minus the opponent's) before the
    /// candidate move; playout win rates are measured against it.
    pub score_diff: i32,
    /// Wall-clock budget for the whole evaluation. Playouts stop when it
    /// runs out and every candidate reports the estimate it has; move
    /// generation with several unseen blanks can make playouts slow.
    pub time_budget: Option<Duration>,
}

impl Default for MctsConfig {
    fn default() -> Self {
        MctsConfig {
            num_candidates: 20,
            iterations_per_candidate: 200,
            max_plies: 2,
            parallel: true,
            score_diff: 0,
            time_budget: Some(Duration::from_secs(8)),
        }
    }
}

impl MctsConfig {
    /// Settings for the pre-endgame (bag of 1–7 tiles): the game ends
    /// within a few plies, so play every playout to the end.
    pub fn pre_endgame() -> Self {
        MctsConfig {
            num_candidates: 25,
            iterations_per_candidate: 120,
            max_plies: 10,
            parallel: true,
            score_diff: 0,
            time_budget: Some(Duration::from_secs(8)),
        }
    }

    /// Settings by bag size. The ending is always played exactly whenever
    /// a playout reaches it; the depth decides how often it does:
    /// - 1-7 in the bag: to the end of the game (see `pre_endgame`)
    /// - 8-21: four plies, enough for either side to empty the bag
    /// - more: two plies (our move, the reply, our follow-up) plus leaves
    pub fn for_bag(bag: usize) -> Self {
        if bag <= RACK_SIZE {
            Self::pre_endgame()
        } else if bag <= 3 * RACK_SIZE {
            MctsConfig {
                num_candidates: 25,
                iterations_per_candidate: 150,
                max_plies: 4,
                ..Self::default()
            }
        } else {
            Self::default()
        }
    }

    pub fn with_score_diff(mut self, score_diff: i32) -> Self {
        self.score_diff = score_diff;
        self
    }
}

/// Result of Monte Carlo evaluation for a single move
#[derive(Debug, Clone)]
pub struct MctsResult {
    /// The evaluated move
    pub mv: Move,
    /// Expected spread change from playing this move (our points minus the
    /// opponent's over the playout, plus leave values if it was cut short)
    pub mcts_equity: f32,
    /// Fraction of playouts we finished ahead in, counting the score
    /// difference before the move (ties count half). With the ply limit
    /// reached this is judged on spread plus leave values.
    pub win_rate: f32,
    /// Playouts completed
    pub iterations: usize,
    /// Share of playouts in which the opponent's reply was a bingo
    pub opp_bingo_rate: f32,
    /// Share of playouts in which our follow-up move was a bingo
    pub our_bingo_rate: f32,
}

/// Pool of available tiles for random drawing
pub struct TilePool {
    tiles: Vec<Tile>,
}

impl TilePool {
    /// Create a tile pool from remaining tile counts
    pub fn from_remaining(remaining: &std::collections::HashMap<char, u32>) -> Self {
        let mut tiles = Vec::with_capacity(100);
        for (&letter, &count) in remaining {
            for _ in 0..count {
                if letter == '?' {
                    tiles.push(Tile::blank(' '));
                } else {
                    tiles.push(Tile::new(letter));
                }
            }
        }
        // Canonical order, so the same seed gives the same draw sequence
        // regardless of HashMap iteration order.
        tiles.sort_by_key(|t| (t.is_blank, t.letter));
        TilePool { tiles }
    }

    /// Get the number of tiles in the pool
    #[inline]
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Check if pool is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// A shuffled copy of the pool's tiles.
    fn shuffled(&self, rng: &mut Rng) -> Vec<Tile> {
        let mut tiles = self.tiles.clone();
        rng.shuffle(&mut tiles);
        tiles
    }
}

fn rack_from(tiles: &[Tile]) -> Rack {
    let mut rack = Rack::new();
    for t in tiles {
        rack.add(*t);
    }
    rack
}

/// Which player is to move in a playout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Player {
    Us,
    Opp,
}

impl Player {
    fn other(self) -> Player {
        match self {
            Player::Us => Player::Opp,
            Player::Opp => Player::Us,
        }
    }
}

/// One point of a playout's history: after `mover`'s move, the spread from
/// our point of view and the tiles left in the bag.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))] // read by the self-play studies
pub struct TracePoint {
    pub mover_is_us: bool,
    pub spread: i32,
    pub bag: usize,
    pub was_bingo: bool,
    /// Phase-scaled leave value of our rack minus the opponent's, as the
    /// cut-off valuation in [`Playout::run`] would add it.
    pub leave_adj: f32,
}

/// One simulated game from the position after our candidate move.
struct Playout<'a> {
    dict: &'a Dictionary,
    board: Board,
    racks: [Vec<Tile>; 2],
    bag: Vec<Tile>,
    /// Signed spread from our point of view.
    spread: i32,
    /// Player who drew the last tile, once the bag is empty.
    final_turn_owner: Option<Player>,
    /// For returning exchanged tiles to random places in the bag.
    rng: Rng,
    /// Whether each player's first move in this playout was a bingo.
    first_move_bingo: [Option<bool>; 2],
    /// Move-by-move history, kept only when requested (self-play studies).
    trace: Option<Vec<TracePoint>>,
}

impl<'a> Playout<'a> {
    fn new(
        dict: &'a Dictionary,
        board: Board,
        racks: [Vec<Tile>; 2],
        bag: Vec<Tile>,
        seed: u64,
    ) -> Self {
        Playout {
            dict,
            board,
            racks,
            bag,
            spread: 0,
            final_turn_owner: None,
            rng: Rng::with_seed(seed),
            first_move_bingo: [None, None],
            trace: None,
        }
    }

    fn rack(&self, p: Player) -> &Vec<Tile> {
        &self.racks[p as usize]
    }

    fn rack_mut(&mut self, p: Player) -> &mut Vec<Tile> {
        &mut self.racks[p as usize]
    }

    fn unseen_for_phase(&self) -> usize {
        // From the mover's perspective the unseen count is bag + other rack,
        // which is the same for both players when both racks are full; use
        // bag + 7 as the phase indicator, floored at the real count.
        self.bag.len() + RACK_SIZE
    }

    /// Make `mv` for `p`: place or exchange its tiles, score it, draw, and
    /// for an exchange return the exchanged tiles to the bag afterwards (so
    /// the bag size is unchanged and an exchange can never empty it).
    fn commit(&mut self, p: Player, mv: &Move) {
        for (r, c, tile) in &mv.tiles_used {
            self.board.place_tile(*r, *c, *tile);
        }
        for tile in mv.tiles_from_rack() {
            let rack = self.rack_mut(p);
            let idx = rack
                .iter()
                .position(|t| {
                    t.is_blank == tile.is_blank && (t.is_blank || t.letter == tile.letter)
                })
                .expect("move tiles come from the mover's rack");
            rack.swap_remove(idx);
        }
        let signed = mv.score as i32;
        self.spread += if p == Player::Us { signed } else { -signed };
        if self.first_move_bingo[p as usize].is_none() {
            self.first_move_bingo[p as usize] = Some(mv.is_bingo);
        }

        let need = RACK_SIZE
            .saturating_sub(self.rack(p).len())
            .min(self.bag.len());
        if need > 0 {
            let drawn: Vec<Tile> = self.bag.drain(self.bag.len() - need..).collect();
            self.rack_mut(p).extend(drawn);
            if self.bag.is_empty() {
                self.final_turn_owner = Some(p);
            }
        }
        if let MoveKind::Exchange(returned) = &mv.kind {
            for &tile in returned {
                let at = self.rng.usize(..=self.bag.len());
                self.bag.insert(at, tile);
            }
        }
        if self.trace.is_some() {
            let leave_adj = self.leave_adjustment();
            if let Some(trace) = &mut self.trace {
                trace.push(TracePoint {
                    mover_is_us: p == Player::Us,
                    spread: self.spread,
                    bag: self.bag.len(),
                    was_bingo: mv.is_bingo,
                    leave_adj,
                });
            }
        }
    }

    /// Our rack's leave value minus the opponent's, phase-scaled.
    fn leave_adjustment(&self) -> f32 {
        let unseen = self.unseen_for_phase();
        self.dict.leave_value(self.rack(Player::Us), Some(unseen))
            - self.dict.leave_value(self.rack(Player::Opp), Some(unseen))
    }

    /// Highest-scoring move for `p` (None means pass).
    fn best_by_score(&self, p: Player) -> Option<Move> {
        let solver = Solver::new(&self.board, self.dict);
        let rack = rack_from(self.rack(p));
        solver.find_moves(&rack).into_iter().max_by_key(|m| m.score)
    }

    /// Best move by static equity for `p`, including exchanges when the bag
    /// allows them (None means pass).
    fn best_by_equity(&self, p: Player) -> Option<Move> {
        let solver = Solver::new(&self.board, self.dict);
        let rack = rack_from(self.rack(p));
        let best_play = solver.find_moves(&rack).into_iter().next();
        if self.bag.len() < EXCHANGE_MIN_BAG {
            return best_play;
        }
        let unseen = self.unseen_for_phase();
        let best_exchange = exchange_moves(self.rack(p), self.dict, Some(unseen))
            .into_iter()
            .max_by(|a, b| {
                a.equity
                    .partial_cmp(&b.equity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        match (best_play, best_exchange) {
            (Some(play), Some(exch)) => Some(if exch.equity > play.equity {
                exch
            } else {
                play
            }),
            (play, exch) => play.or(exch),
        }
    }

    /// `p`'s last turn with `p.other()` holding the final move: choose the
    /// play maximising score minus the other's best reply, or pass.
    fn best_last_turn(&self, p: Player) -> (Option<Move>, i32) {
        let solver = Solver::new(&self.board, self.dict);
        let rack = rack_from(self.rack(p));
        let other_rack = rack_from(self.rack(p.other()));
        let mut moves = solver.find_moves(&rack);
        moves.sort_by_key(|m| std::cmp::Reverse(m.score));

        let reply_score = |board: &Board| -> i32 {
            Solver::new(board, self.dict)
                .find_moves(&other_rack)
                .iter()
                .map(|m| m.score as i32)
                .max()
                .unwrap_or(0)
        };

        // Passing: the other player replies on the unchanged board.
        let mut best_value = -reply_score(&self.board);
        let mut best_move = None;

        for mv in moves {
            // value = score - reply <= score, so once scores drop to the
            // best value found nothing further can improve on it.
            if (mv.score as i32) <= best_value {
                break;
            }
            let mut board = self.board.clone();
            for (r, c, tile) in &mv.tiles_used {
                board.place_tile(*r, *c, *tile);
            }
            let value = mv.score as i32 - reply_score(&board);
            if value > best_value {
                best_value = value;
                best_move = Some(mv);
            }
        }
        (best_move, best_value)
    }

    /// A fresh game: empty board, shuffled full bag, both racks drawn.
    #[cfg(test)]
    fn new_game(dict: &'a Dictionary, seed: u64) -> Self {
        let mut rng = Rng::with_seed(seed);
        let mut bag = full_tile_set();
        rng.shuffle(&mut bag);
        let us: Vec<Tile> = bag.drain(bag.len() - RACK_SIZE..).collect();
        let opp: Vec<Tile> = bag.drain(bag.len() - RACK_SIZE..).collect();
        Self::new(dict, Board::new(), [us, opp], bag, rng.u64(..))
    }

    /// Play the game out from `to_move`'s turn. Returns the final spread
    /// (plus leave and tempo adjustments if the ply limit cut the game short).
    #[cfg(test)]
    fn run(&mut self, to_move: Player, max_plies: usize) -> f32 {
        self.run_with(to_move, max_plies, &mut |_, _| {}).value
    }

    /// As [`Self::run`], calling `observe(self, to_move)` before every move
    /// of the greedy phase (self-play studies sample positions this way).
    fn run_with(
        &mut self,
        mut to_move: Player,
        max_plies: usize,
        observe: &mut dyn FnMut(&Playout<'a>, Player),
    ) -> PlayoutOutcome {
        let finished = |p: &Playout| PlayoutOutcome {
            value: p.spread as f32,
            finished: true,
            bag: p.bag.len(),
        };
        let mut plies = 0;
        loop {
            match self.final_turn_owner {
                Some(owner) if owner == to_move => {
                    // Final move of the game: take the points.
                    if let Some(mv) = self.best_by_score(to_move) {
                        self.commit(to_move, &mv);
                    }
                    return finished(self);
                }
                Some(_) => {
                    // Last turn before the owner's final move: exact 2-ply.
                    let (mv, _) = self.best_last_turn(to_move);
                    if let Some(mv) = mv {
                        self.commit(to_move, &mv);
                    }
                    to_move = to_move.other();
                    continue;
                }
                None => {}
            }

            if plies >= max_plies {
                // Cut short: value the racks statically, plus the tempo of
                // whoever is to move.
                let tempo = if to_move == Player::Us { TEMPO } else { -TEMPO };
                return PlayoutOutcome {
                    value: self.spread as f32 + self.leave_adjustment() + tempo,
                    finished: false,
                    bag: self.bag.len(),
                };
            }

            observe(self, to_move);
            if let Some(mv) = self.best_by_equity(to_move) {
                self.commit(to_move, &mv);
            } else if self.bag.is_empty() {
                // No bag, no move: treat as the game ending here.
                return finished(self);
            } else if self.trace.is_some() {
                let leave_adj = self.leave_adjustment();
                if let Some(trace) = &mut self.trace {
                    trace.push(TracePoint {
                        mover_is_us: to_move == Player::Us,
                        spread: self.spread,
                        bag: self.bag.len(),
                        was_bingo: false,
                        leave_adj,
                    });
                }
            }
            plies += 1;
            to_move = to_move.other();
        }
    }
}

/// Per-candidate simulation state, kept across pruning rounds.
struct CandidateSim<'a> {
    mv: &'a Move,
    board: Board,
    our_leave: Vec<Tile>,
    pool: TilePool,
    bag_size: usize,
    draw_count: usize,
    score_diff: i32,
    rng: Rng,
    total: f32,
    wins: f32,
    iterations: usize,
    /// Playouts in which the opponent's reply / our follow-up was a bingo.
    opp_bingos: u32,
    our_bingos: u32,
    /// Every playout value, in order; candidates share draw sequences, so
    /// element-wise differences between candidates are paired samples.
    values: Vec<f32>,
}

impl<'a> CandidateSim<'a> {
    fn new(state: &SolverState, mv: &'a Move, score_diff: i32) -> Self {
        let mut base_state = state.clone();
        base_state.apply_move(mv);
        let our_leave: Vec<Tile> = base_state.rack.tiles();

        // Tiles we may draw from: the unseen tiles before our move (bag +
        // opponent's rack). A play moves tiles to the board, which leaves
        // this set unchanged; an exchange returns its tiles to the bag only
        // after the draw, so they must not be in the pool either. While the
        // bag has tiles the opponent holds a full rack.
        let pool = TilePool::from_remaining(&state.unseen_tiles());
        let bag_size = pool.len().saturating_sub(RACK_SIZE);
        let draw_count = mv.tiles_from_rack().len().min(bag_size);

        CandidateSim {
            mv,
            board: base_state.board,
            our_leave,
            pool,
            bag_size,
            draw_count,
            score_diff,
            // The unseen pool is the same for every candidate (it does not
            // depend on which move we play), so seeding every candidate
            // identically makes them face the same draw sequences: common
            // random numbers, which tightens comparisons between candidates
            // and makes repeated runs reproducible.
            rng: Rng::with_seed(0x5EED_C0DE),
            total: 0.0,
            wins: 0.0,
            iterations: 0,
            opp_bingos: 0,
            our_bingos: 0,
            values: Vec::new(),
        }
    }

    fn mean(&self) -> f32 {
        if self.iterations == 0 {
            self.mv.score as f32
        } else {
            self.total / self.iterations as f32
        }
    }

    /// Add one playout. A finished game contributes a win, a loss or half
    /// a point for a tie; a cut-off game contributes the probability that
    /// the remaining spread swing (normal with the bag-dependent sd)
    /// leaves us ahead.
    fn record(&mut self, outcome: PlayoutOutcome) {
        self.values.push(outcome.value);
        self.total += outcome.value;
        let final_diff = self.score_diff as f32 + outcome.value;
        self.wins += if outcome.finished {
            match final_diff.partial_cmp(&0.0) {
                Some(std::cmp::Ordering::Greater) => 1.0,
                Some(std::cmp::Ordering::Less) => 0.0,
                _ => 0.5,
            }
        } else {
            normal_cdf(final_diff / remaining_spread_sd(outcome.bag))
        };
        self.iterations += 1;
    }

    fn run(
        &mut self,
        dict: &Dictionary,
        playouts: usize,
        max_plies: usize,
        deadline: Option<Instant>,
    ) {
        if self.pool.is_empty() {
            // Opponent has nothing: our move ends the game.
            for _ in 0..playouts {
                self.record(PlayoutOutcome {
                    value: self.mv.score as f32,
                    finished: true,
                    bag: 0,
                });
            }
            return;
        }

        for _ in 0..playouts {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return;
            }
            let shuffled = self.pool.shuffled(&mut self.rng);
            let (drawn, rest) = shuffled.split_at(self.draw_count);
            let (opp_rack, bag) = rest.split_at(rest.len().min(RACK_SIZE));

            let mut our_rack = self.our_leave.clone();
            our_rack.extend_from_slice(drawn);
            let mut bag = bag.to_vec();
            if let MoveKind::Exchange(returned) = &self.mv.kind {
                // Exchanged tiles go back after we draw; the bag is shuffled
                // so the position does not matter.
                bag.extend_from_slice(returned);
                self.rng.shuffle(&mut bag);
            }

            let seed = self.rng.u64(..);
            let mut playout = Playout::new(
                dict,
                self.board.clone(),
                [our_rack, opp_rack.to_vec()],
                bag,
                seed,
            );
            playout.spread = self.mv.score as i32;
            // If our move emptied the bag, we own the final turn: the
            // opponent gets one last turn and then we play the final move.
            if playout.bag.is_empty() && self.bag_size > 0 {
                playout.final_turn_owner = Some(Player::Us);
            }

            let outcome = playout.run_with(Player::Opp, max_plies, &mut |_, _| {});
            self.opp_bingos +=
                u32::from(playout.first_move_bingo[Player::Opp as usize] == Some(true));
            self.our_bingos +=
                u32::from(playout.first_move_bingo[Player::Us as usize] == Some(true));
            self.record(outcome);
        }
    }

    fn result(&self) -> MctsResult {
        let n = self.iterations.max(1) as f32;
        MctsResult {
            mv: self.mv.clone(),
            mcts_equity: self.mean(),
            win_rate: if self.iterations == 0 {
                0.5
            } else {
                self.wins / n
            },
            iterations: self.iterations,
            opp_bingo_rate: self.opp_bingos as f32 / n,
            our_bingo_rate: self.our_bingos as f32 / n,
        }
    }
}

/// Worth of being the player to move when a playout is cut off mid-game,
/// in points of final spread. Measured by self-play (`study_spread_variance`,
/// 800 games, 2026-08-20): with the bag holding 8+ tiles, the final spread
/// minus (spread so far + leave values) averaged +17.6 / +19.6 with us to
/// move and -20.9 / -23.2 with the opponent to move, whichever side had
/// started the game. The same offset applies to every candidate, so it
/// changes win estimates, not rankings.
const TEMPO: f32 = 21.0;

/// Standard deviation of the spread still to come from a mid-game
/// position, by tiles left in the bag. Same study: the variance of the
/// residual grows linearly with the bag, var = 159 + 79.8 * bag (sd 12.6
/// with an empty bag, 64 with 50 tiles, 84 at the start of the game).
fn remaining_spread_sd(bag: usize) -> f32 {
    (159.4 + 79.8 * bag as f32).sqrt()
}

/// Standard normal CDF (Abramowitz & Stegun 7.1.26, |error| < 1.5e-7).
fn normal_cdf(x: f32) -> f32 {
    let z = (x as f64) / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z.abs());
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-z * z).exp();
    let erf = if z >= 0.0 { erf } else { -erf };
    (0.5 * (1.0 + erf)) as f32
}

/// Result of one playout.
#[derive(Debug, Clone, Copy)]
struct PlayoutOutcome {
    /// Spread change from our point of view (exact if `finished`, otherwise
    /// spread so far plus leave values plus the tempo of the side to move).
    value: f32,
    /// The game reached its end inside the ply limit.
    finished: bool,
    /// Tiles left in the bag where the playout stopped.
    bag: usize,
}

/// Candidates trailing the leader by more than this many points after a
/// pruning round — and by a statistically clear amount — are not simulated
/// further.
const PRUNE_MARGIN: f32 = 4.0;

/// How many standard errors of the paired difference the deficit must
/// exceed before a candidate is pruned.
const PRUNE_SIGMAS: f32 = 2.5;

/// Mean and standard error of the element-wise difference `a - b`.
fn paired_difference(a: &[f32], b: &[f32]) -> (f32, f32) {
    let n = a.len().min(b.len());
    if n == 0 {
        return (0.0, f32::INFINITY);
    }
    let diffs = a.iter().zip(b).map(|(x, y)| x - y);
    let mean = diffs.clone().sum::<f32>() / n as f32;
    if n < 2 {
        return (mean, f32::INFINITY);
    }
    let var = diffs.map(|d| (d - mean).powi(2)).sum::<f32>() / (n - 1) as f32;
    (mean, (var / n as f32).sqrt())
}

/// Monte Carlo evaluator for move selection
pub struct MctsEvaluator {
    config: MctsConfig,
}

impl MctsEvaluator {
    pub fn new(config: MctsConfig) -> Self {
        MctsEvaluator { config }
    }

    /// Evaluate the top candidates and return results sorted by equity.
    ///
    /// Playouts run in rounds; after each round, candidates that trail the
    /// leader by more than [`PRUNE_MARGIN`] points, by at least
    /// [`PRUNE_SIGMAS`] standard errors of the paired difference, stop and
    /// keep their estimate so far. The budget concentrates on the moves that
    /// can still finish first. Results list the fully simulated candidates
    /// first (by expected spread), then the pruned ones, so a stale
    /// short-run estimate never outranks a complete one.
    pub fn evaluate(
        &self,
        state: &SolverState,
        candidates: &[Move],
        dictionary: &Dictionary,
    ) -> Vec<MctsResult> {
        let num_to_evaluate = candidates.len().min(self.config.num_candidates);
        let mut sims: Vec<CandidateSim> = candidates[..num_to_evaluate]
            .iter()
            .map(|mv| CandidateSim::new(state, mv, self.config.score_diff))
            .collect();

        // Quarter, quarter, half: two pruning points.
        let total = self.config.iterations_per_candidate;
        let rounds = [total / 4, total / 4, total - 2 * (total / 4)];
        let mut active: Vec<usize> = (0..sims.len()).collect();
        let max_plies = self.config.max_plies;
        let deadline = self.config.time_budget.map(|b| Instant::now() + b);

        for (round, &playouts) in rounds.iter().enumerate() {
            if playouts == 0 {
                continue;
            }
            if self.config.parallel {
                let active_set: std::collections::HashSet<usize> = active.iter().copied().collect();
                sims.par_iter_mut()
                    .enumerate()
                    .filter(|(i, _)| active_set.contains(i))
                    .for_each(|(_, sim)| sim.run(dictionary, playouts, max_plies, deadline));
            } else {
                for &i in &active {
                    sims[i].run(dictionary, playouts, max_plies, deadline);
                }
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }

            if round + 1 < rounds.len() {
                active.sort_by(|&a, &b| {
                    sims[b]
                        .mean()
                        .partial_cmp(&sims[a].mean())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let leader = active[0];
                let contenders: Vec<usize> = active
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let (deficit, se) =
                            paired_difference(&sims[leader].values, &sims[i].values);
                        deficit <= PRUNE_MARGIN || deficit <= PRUNE_SIGMAS * se
                    })
                    .collect();
                if contenders.len() >= 4 {
                    active = contenders;
                } else {
                    active.truncate(4.min(active.len()));
                }
            }
        }

        let full = sims.iter().map(|s| s.iterations).max().unwrap_or(0);
        let mut results: Vec<MctsResult> = sims.iter().map(|s| s.result()).collect();
        results.sort_by(|a, b| {
            let a_full = a.iterations == full;
            let b_full = b.iterations == full;
            b_full.cmp(&a_full).then(
                b.mcts_equity
                    .partial_cmp(&a.mcts_equity)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::load_default_dictionary;

    #[test]
    fn test_tile_pool_creation() {
        let mut remaining = std::collections::HashMap::new();
        remaining.insert('A', 3);
        remaining.insert('B', 2);
        remaining.insert('?', 1);

        let pool = TilePool::from_remaining(&remaining);
        assert_eq!(pool.len(), 6);
        assert!(!pool.is_empty());
        let mut rng = Rng::with_seed(1);
        assert_eq!(pool.shuffled(&mut rng).len(), 6);
    }

    #[test]
    fn test_mcts_evaluator_basic() {
        let dict = load_default_dictionary().unwrap();
        let state = SolverState::new();

        let mut test_state = state.clone();
        for ch in ['C', 'A', 'T', 'S', 'E', 'R', 'N'] {
            test_state.add_to_rack(ch);
        }

        let suggestions = test_state.get_suggestions(&dict);
        assert!(!suggestions.is_empty());

        let config = MctsConfig {
            num_candidates: 3,
            iterations_per_candidate: 5,
            max_plies: 2,
            parallel: false,
            score_diff: 0,
            time_budget: None,
        };

        let evaluator = MctsEvaluator::new(config);
        let results = evaluator.evaluate(&test_state, &suggestions, &dict);

        assert!(!results.is_empty());
        assert!(results.len() <= 3);
        for result in &results {
            assert!(result.mcts_equity.is_finite());
            assert!(result.iterations >= 1 && result.iterations <= 5);
            assert!((0.0..=1.0).contains(&result.win_rate));
        }
        // The leader always receives the full budget.
        assert_eq!(results[0].iterations, 5);
    }

    /// Two-tile bag. Playing out the ending must respect the equal-turn
    /// rule: whoever draws the last tile gets the final move, nobody is
    /// penalised for leftover tiles, and the playout ends after the two
    /// final turns rather than when a rack empties.
    #[test]
    fn test_playout_follows_equal_turn_rule() {
        let dict = load_default_dictionary().unwrap();
        let mut board = Board::new();
        board.place_tile(7, 6, Tile::new('C'));
        board.place_tile(7, 7, Tile::new('A'));
        board.place_tile(7, 8, Tile::new('T'));

        // We move first and empty the bag (bag is empty from the start here,
        // with us as owner): the opponent gets a last turn, then we play.
        let mut playout = Playout::new(
            &dict,
            board.clone(),
            [
                vec![Tile::new('S'), Tile::new('E')],
                vec![Tile::new('Z'), Tile::new('A')],
            ],
            vec![],
            1,
        );
        playout.final_turn_owner = Some(Player::Us);
        let value = playout.run(Player::Opp, 10);
        // The opponent played (ZA somewhere, > 0 points) and we replied; the
        // playout stopped there even though both racks may still hold tiles.
        assert!(playout.racks[1].len() <= 2);
        assert_eq!(value, playout.spread as f32);

        // Reproduce by hand: the opponent's best last turn is the 2-ply
        // optimum, and our final move is our best score on the board after it.
        let mut check = Playout::new(
            &dict,
            board,
            [
                vec![Tile::new('S'), Tile::new('E')],
                vec![Tile::new('Z'), Tile::new('A')],
            ],
            vec![],
            1,
        );
        check.final_turn_owner = Some(Player::Us);
        let (opp_mv, opp_value) = check.best_last_turn(Player::Opp);
        let opp_score = opp_mv.as_ref().map(|m| m.score as i32).unwrap_or(0);
        if let Some(m) = &opp_mv {
            check.commit(Player::Opp, m);
        }
        let our_final = check
            .best_by_score(Player::Us)
            .map(|m| m.score as i32)
            .unwrap_or(0);
        assert_eq!(opp_value, opp_score - our_final);
        assert_eq!(value as i32, our_final - opp_score);
    }

    /// When our candidate draws the last tile we own the final turn; when it
    /// does not, the opponent's draw decides. Either way the playout must
    /// terminate with an empty bag and a finite value.
    #[test]
    fn test_pre_endgame_evaluation_terminates() {
        let dict = load_default_dictionary().unwrap();
        // 87 tiles on the board and 4 in our rack: 9 unseen, bag of 2.
        const MID: &str = "14U/5HELM3ZIN/4VIG7T/6AYES3BI/6DOWER2AL/9GOOFS1/1BRA4GOWN1I1/POETIC1QI2L1N1/OX3H1U3Y3/R3FINAlS5/T4V1K1P5/A4e1EME5/L6DON5/EtHER3AD5/D2RECOAT6";
        let state = SolverState::from_cgp(&format!("{} AEST", MID)).unwrap();
        assert_eq!(state.unseen_tile_count(), 9);

        let suggestions = state.get_suggestions(&dict);
        assert!(!suggestions.is_empty());
        let evaluator = MctsEvaluator::new(MctsConfig {
            num_candidates: 4,
            iterations_per_candidate: 10,
            max_plies: 10,
            parallel: false,
            score_diff: 0,
            time_budget: None,
        });
        let results = evaluator.evaluate(&state, &suggestions, &dict);
        assert_eq!(results.len(), 4.min(suggestions.len()));
        for r in &results {
            assert!(r.mcts_equity.is_finite());
            assert!(r.iterations >= 2);
        }
        assert_eq!(results[0].iterations, 10);
    }

    #[test]
    fn normal_cdf_and_spread_sd() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-6);
        assert!((normal_cdf(1.0) - 0.841_345).abs() < 1e-4);
        assert!((normal_cdf(-2.0) - 0.022_750).abs() < 1e-4);
        assert!(normal_cdf(10.0) > 0.999_99);
        assert!(remaining_spread_sd(0) > 12.0 && remaining_spread_sd(0) < 13.0);
        assert!(remaining_spread_sd(86) > 80.0 && remaining_spread_sd(86) < 90.0);
    }

    #[test]
    fn paired_difference_statistics() {
        let (m, se) = paired_difference(&[], &[]);
        assert_eq!(m, 0.0);
        assert!(se.is_infinite());
        let (m, se) = paired_difference(&[5.0], &[2.0]);
        assert_eq!(m, 3.0);
        assert!(se.is_infinite());
        // Constant difference: zero standard error.
        let (m, se) = paired_difference(&[1.0, 2.0, 3.0, 4.0], &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(m, 1.0);
        assert_eq!(se, 0.0);
        // Differences 2, -2, 2, -2: mean 0, sample sd = sqrt(16/3), se = sd/2.
        let (m, se) = paired_difference(&[2.0, 0.0, 2.0, 0.0], &[0.0, 2.0, 0.0, 2.0]);
        assert_eq!(m, 0.0);
        assert!((se - (16.0f32 / 3.0).sqrt() / 2.0).abs() < 1e-6);
    }

    /// Fully simulated candidates rank ahead of pruned ones regardless of
    /// their short-run means.
    #[test]
    fn fully_simulated_candidates_rank_first() {
        let dict = load_default_dictionary().unwrap();
        const MID: &str = "14U/5HELM3ZIN/4VIG7T/6AYES3BI/6DOWER2AL/9GOOFS1/1BRA4GOWN1I1/POETIC1QI2L1N1/OX3H1U3Y3/R3FINAlS5/T4V1K1P5/A4e1EME5/L6DON5/EtHER3AD5/D2RECOAT6";
        let state = SolverState::from_cgp(&format!("{} ES", MID)).unwrap();
        let suggestions = state.get_suggestions(&dict);
        let results = MctsEvaluator::new(MctsConfig {
            num_candidates: 12,
            iterations_per_candidate: 24,
            max_plies: 10,
            parallel: true,
            score_diff: 0,
            time_budget: None,
        })
        .evaluate(&state, &suggestions, &dict);
        let full = results.iter().map(|r| r.iterations).max().unwrap();
        assert_eq!(full, 24);
        let first_partial = results.iter().position(|r| r.iterations < full);
        if let Some(p) = first_partial {
            assert!(
                results[p..].iter().all(|r| r.iterations < full),
                "a fully simulated candidate is listed after a pruned one"
            );
            assert!(results[..p]
                .windows(2)
                .all(|w| w[0].mcts_equity >= w[1].mcts_equity));
        }
    }

    /// A tiny time budget stops playouts early and still returns a result
    /// for every candidate.
    #[test]
    fn time_budget_is_honoured() {
        let dict = load_default_dictionary().unwrap();
        const MID: &str = "14U/5HELM3ZIN/4VIG7T/6AYES3BI/6DOWER2AL/9GOOFS1/1BRA4GOWN1I1/POETIC1QI2L1N1/OX3H1U3Y3/R3FINAlS5/T4V1K1P5/A4e1EME5/L6DON5/EtHER3AD5/D2RECOAT6";
        let state = SolverState::from_cgp(&format!("{} AEST", MID)).unwrap();
        let suggestions = state.get_suggestions(&dict);
        let started = Instant::now();
        let results = MctsEvaluator::new(MctsConfig {
            num_candidates: 25,
            iterations_per_candidate: 400,
            max_plies: 10,
            parallel: true,
            score_diff: 0,
            time_budget: Some(Duration::from_millis(150)),
        })
        .evaluate(&state, &suggestions, &dict);
        // One playout may overrun the deadline per thread; allow slack.
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(results.len(), 25.min(suggestions.len()));
        assert!(results.iter().all(|r| r.mcts_equity.is_finite()));
        assert!(results.iter().any(|r| r.iterations < 400));
    }

    /// Timing harness for the pre-endgame playout. Run with
    /// `cargo test --release bench_pre_endgame -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_pre_endgame() {
        use std::time::Instant;
        let dict = load_default_dictionary().unwrap();
        const MID: &str = "14U/5HELM3ZIN/4VIG7T/6AYES3BI/6DOWER2AL/9GOOFS1/1BRA4GOWN1I1/POETIC1QI2L1N1/OX3H1U3Y3/R3FINAlS5/T4V1K1P5/A4e1EME5/L6DON5/EtHER3AD5/D2RECOAT6";
        // Same dense board with the three board blanks turned into real
        // tiles, so the three blanks are unseen and land in the opponent's
        // rack: the slowest realistic case for the 2-ply replies.
        const END_BLANKS_UNSEEN: &str = "3N3Q3M3/3OP2UNTEACH1/3BE2O3N3/3LA2T2MYOPE/2FER2H1LO4/2O6AB4/2R1F4U3DE/2TRIVET1D3AY/4LA1A1IODIZE/4KI1JINX2ES/5R1E1G4O/OUTIES1S3G2R/11L2E/8WAHINES/2EGALITE2A3";
        const EMPTY: &str = "15/15/15/15/15/15/15/15/15/15/15/15/15/15/15";
        const BEECH: &str = "15/15/15/15/15/15/15/3BEECH7/7I7/7K7/7E7/7r7/15/15/15";
        for (board, rack) in [
            (EMPTY, "AEINRST"),
            (BEECH, "AEINRS?"),
            (MID, "AEIST"),
            (MID, "AEST"),
            (MID, "ES"),
            (MID, "E"),
            (END_BLANKS_UNSEEN, "R"),
        ] {
            let state = SolverState::from_cgp(&format!("{} {}", board, rack)).unwrap();
            let bag = state.unseen_tile_count() - RACK_SIZE;
            let suggestions = state.get_suggestions(&dict);
            let config = MctsConfig::for_bag(bag);
            let t = Instant::now();
            let results = MctsEvaluator::new(config.clone()).evaluate(&state, &suggestions, &dict);
            let elapsed = t.elapsed();
            println!(
                "rack {:7} bag {:2} {} plies  {} candidates x {} playouts: {:?}",
                rack,
                bag,
                config.max_plies,
                results.len(),
                config.iterations_per_candidate,
                elapsed
            );
            for r in results.iter().take(5) {
                println!(
                    "   {:10} {:>4} score {:>3} static {:>6.1} sim {:>6.1} win {:.0}% bingo {:.0}/{:.0} n={}",
                    r.mv.word,
                    r.mv.coords(),
                    r.mv.score,
                    r.mv.equity,
                    r.mcts_equity,
                    r.win_rate * 100.0,
                    r.our_bingo_rate * 100.0,
                    r.opp_bingo_rate * 100.0,
                    r.iterations
                );
            }
        }
    }

    /// Solve the normal equations `a x = b` (small dense system) by Gaussian
    /// elimination with partial pivoting.
    fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
        let n = b.len();
        for col in 0..n {
            let pivot = (col..n)
                .max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())
                .unwrap();
            a.swap(col, pivot);
            b.swap(col, pivot);
            let p = a[col][col];
            if p.abs() < 1e-12 {
                continue;
            }
            for row in 0..n {
                if row == col {
                    continue;
                }
                let f = a[row][col] / p;
                if f == 0.0 {
                    continue;
                }
                let pivot_row = a[col].clone();
                for (x, pv) in a[row].iter_mut().zip(&pivot_row).skip(col) {
                    *x -= f * pv;
                }
                b[row] -= f * b[col];
            }
        }
        (0..n)
            .map(|i| {
                if a[i][i].abs() < 1e-12 {
                    0.0
                } else {
                    b[i] / a[i][i]
                }
            })
            .collect()
    }

    /// Logistic regression by Newton's method with a small ridge term.
    fn fit_logistic(xs: &[Vec<f64>], ys: &[f64], iters: usize) -> Vec<f64> {
        let d = xs[0].len();
        let mut w = vec![0.0; d];
        for _ in 0..iters {
            let mut grad = vec![0.0; d];
            let mut hess = vec![vec![0.0; d]; d];
            for (x, &y) in xs.iter().zip(ys) {
                let z: f64 = x.iter().zip(&w).map(|(a, b)| a * b).sum();
                let p = 1.0 / (1.0 + (-z).exp());
                let r = p - y;
                for i in 0..d {
                    grad[i] += r * x[i];
                    let s = p * (1.0 - p) * x[i];
                    for j in 0..d {
                        hess[i][j] += s * x[j];
                    }
                }
            }
            for i in 0..d {
                hess[i][i] += 1e-3;
                grad[i] += 1e-3 * w[i];
            }
            let step = solve_linear(hess, grad);
            for i in 0..d {
                w[i] -= step[i];
            }
        }
        w
    }

    /// Self-play study of the spread still to come. For every position
    /// with the opponent to move (where a playout cut off by the ply limit
    /// stops), record final spread minus (spread + leave adjustment) and
    /// report its mean and standard deviation by tiles left in the bag.
    /// Run with `cargo test --release study_spread_variance -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn study_spread_variance() {
        let dict = load_default_dictionary().unwrap();
        let games: usize = std::env::var("GAMES")
            .ok()
            .and_then(|g| g.parse().ok())
            .unwrap_or(400);
        let started = Instant::now();
        // Half the games we start, half the opponent starts.
        let traces: Vec<(bool, Vec<TracePoint>, i32)> = (0..games as u64)
            .into_par_iter()
            .map(|seed| {
                let we_start = seed % 2 == 0;
                let mut g = Playout::new_game(&dict, 1000 + seed);
                g.trace = Some(Vec::new());
                let first = if we_start { Player::Us } else { Player::Opp };
                let final_spread = g.run(first, 1000) as i32;
                (we_start, g.trace.take().unwrap(), final_spread)
            })
            .collect();
        println!("{} games in {:?}", games, started.elapsed());

        // Residuals bucketed by bag size, opponent to move next; tempo
        // offsets by who started and who is to move.
        let mut by_bag: Vec<Vec<f64>> = vec![Vec::new(); 101];
        let mut by_bag_us: Vec<Vec<f64>> = vec![Vec::new(); 101];
        let mut tempo: [Vec<f64>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut plies = 0usize;
        let mut bingos = 0usize;
        let mut final_spreads = [Vec::new(), Vec::new()];
        for (we_start, trace, final_spread) in &traces {
            final_spreads[usize::from(*we_start)].push(*final_spread as f64);
            for t in trace {
                plies += 1;
                bingos += usize::from(t.was_bingo);
                let residual = *final_spread as f64 - (t.spread as f64 + t.leave_adj as f64);
                // Only mid-game points (bag >= 8) for the tempo estimate.
                if t.bag >= 8 {
                    tempo[usize::from(*we_start) * 2 + usize::from(t.mover_is_us)].push(residual);
                }
                if t.mover_is_us {
                    by_bag[t.bag].push(residual);
                } else {
                    by_bag_us[t.bag].push(-residual);
                }
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        println!(
            "{} plies, {:.1}% bingos, mean final spread: we start {:+.1}, they start {:+.1}",
            plies,
            100.0 * bingos as f64 / plies as f64,
            mean(&final_spreads[1]),
            mean(&final_spreads[0])
        );
        println!("tempo (mean residual, bag >= 8):");
        println!(
            "  they started, we are to move:   {:+.1} (n={})",
            mean(&tempo[0]),
            tempo[0].len()
        );
        println!(
            "  they started, they are to move: {:+.1} (n={})",
            mean(&tempo[1]),
            tempo[1].len()
        );
        println!(
            "  we started, we are to move:     {:+.1} (n={})",
            mean(&tempo[2]),
            tempo[2].len()
        );
        println!(
            "  we started, they are to move:   {:+.1} (n={})",
            mean(&tempo[3]),
            tempo[3].len()
        );
        println!("bag  n     mean   sd   (opponent to move; residual = final - spread - leaves)");
        let mut fit_x = Vec::new();
        let mut fit_y = Vec::new();
        for (bag, v) in by_bag.iter().enumerate() {
            if v.len() < 30 {
                continue;
            }
            let n = v.len() as f64;
            let mean = v.iter().sum::<f64>() / n;
            let var = v.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
            println!("{:3} {:5} {:+6.1} {:5.1}", bag, v.len(), mean, var.sqrt());
            fit_x.push(bag as f64);
            fit_y.push(var);
        }
        // Least squares for var = alpha + beta * bag.
        let n = fit_x.len() as f64;
        let mx = fit_x.iter().sum::<f64>() / n;
        let my = fit_y.iter().sum::<f64>() / n;
        let sxy: f64 = fit_x
            .iter()
            .zip(&fit_y)
            .map(|(x, y)| (x - mx) * (y - my))
            .sum();
        let sxx: f64 = fit_x.iter().map(|x| (x - mx).powi(2)).sum();
        let beta = sxy / sxx;
        let alpha = my - beta * mx;
        println!("variance fit: var(bag) = {:.1} + {:.2} * bag  (sd at 0: {:.1}, at 50: {:.1}, at 86: {:.1})",
            alpha, beta, alpha.max(0.0).sqrt(), (alpha + 50.0 * beta).sqrt(), (alpha + 86.0 * beta).sqrt());
        let mut all_us: Vec<f64> = by_bag_us.iter().flatten().copied().collect();
        all_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean_us = all_us.iter().sum::<f64>() / all_us.len().max(1) as f64;
        println!(
            "mean residual with us to move (sign flipped): {:+.2}",
            mean_us
        );
    }

    /// Self-play study of bingo probability by leave: at positions from
    /// self-play games, keep a random leave of 0-6 tiles from the mover's
    /// rack plus bag, draw the rest, and record whether a bingo exists. A
    /// logistic model on letter counts is fitted and printed; it drives the
    /// bingo-bonus correction to the Scrabble-fitted leave table.
    /// Run with `cargo test --release study_bingo_probability -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn study_bingo_probability() {
        let dict = load_default_dictionary().unwrap();
        let games: usize = std::env::var("GAMES")
            .ok()
            .and_then(|g| g.parse().ok())
            .unwrap_or(300);
        let started = Instant::now();
        // (letter counts incl. blank at 26, bingo?)
        let samples: Vec<([u8; 27], bool)> = (0..games as u64)
            .into_par_iter()
            .flat_map_iter(|seed| {
                let mut rng = Rng::with_seed(5000 + seed);
                let mut out = Vec::new();
                let mut g = Playout::new_game(&dict, 2000 + seed);
                g.run_with(Player::Us, 1000, &mut |p, to_move| {
                    for _ in 0..3 {
                        let mut pool: Vec<Tile> = p.rack(to_move).clone();
                        pool.extend_from_slice(&p.bag);
                        if pool.len() < RACK_SIZE {
                            return;
                        }
                        rng.shuffle(&mut pool);
                        let k = rng.usize(0..=6);
                        let leave = &pool[..k];
                        let rack: Vec<Tile> = pool[..RACK_SIZE].to_vec();
                        let mut counts = [0u8; 27];
                        for t in leave {
                            let i = if t.is_blank {
                                26
                            } else {
                                (t.letter as u8 - b'A') as usize
                            };
                            counts[i] += 1;
                        }
                        let moves = Solver::new(&p.board, p.dict).find_moves(&rack_from(&rack));
                        let bingo = moves.iter().any(|m| m.is_bingo);
                        out.push((counts, bingo));
                    }
                });
                out
            })
            .collect();
        println!(
            "{} samples from {} games in {:?}",
            samples.len(),
            games,
            started.elapsed()
        );

        let xs: Vec<Vec<f64>> = samples
            .iter()
            .map(|(c, _)| {
                let mut x = vec![1.0];
                x.extend(c.iter().map(|&n| n as f64));
                x
            })
            .collect();
        let ys: Vec<f64> = samples.iter().map(|(_, b)| f64::from(*b)).collect();
        let w = fit_logistic(&xs, &ys, 8);
        let p_of =
            |x: &[f64]| 1.0 / (1.0 + (-(x.iter().zip(&w).map(|(a, b)| a * b).sum::<f64>())).exp());
        let p0 = p_of(
            &xs[0]
                .iter()
                .enumerate()
                .map(|(i, _)| if i == 0 { 1.0 } else { 0.0 })
                .collect::<Vec<_>>(),
        );
        println!(
            "overall bingo rate {:.3}; p(empty leave) = {:.3}",
            ys.iter().sum::<f64>() / ys.len() as f64,
            p0
        );
        println!("bias {:+.3}", w[0]);
        println!("letter  weight  p(single)  correction = -10*(p-p0)");
        for i in 0..27 {
            let letter = if i == 26 {
                '?'
            } else {
                (b'A' + i as u8) as char
            };
            let mut x = vec![0.0; 28];
            x[0] = 1.0;
            x[i + 1] = 1.0;
            let p = p_of(&x);
            println!(
                "  {}   {:+.3}   {:.3}   {:+.2}",
                letter,
                w[i + 1],
                p,
                -10.0 * (p - p0)
            );
        }
        // Held-out check: log-loss of the model vs the constant model.
        let n = ys.len() as f64;
        let (mut ll_model, mut ll_const) = (0.0, 0.0);
        let pbar = ys.iter().sum::<f64>() / n;
        for (x, &y) in xs.iter().zip(&ys) {
            let p = p_of(x).clamp(1e-6, 1.0 - 1e-6);
            ll_model += y * p.ln() + (1.0 - y) * (1.0 - p).ln();
            ll_const += y * pbar.ln() + (1.0 - y) * (1.0 - pbar).ln();
        }
        println!(
            "log-loss model {:.4} vs constant {:.4}",
            -ll_model / n,
            -ll_const / n
        );
        println!(
            "WEIGHTS {:?}",
            w.iter()
                .map(|v| (v * 1000.0).round() / 1000.0)
                .collect::<Vec<_>>()
        );
    }
}
