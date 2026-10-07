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

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastrand::Rng;
use rayon::prelude::*;

use crate::board::Board;
use crate::dictionary::Dictionary;
#[cfg(test)]
use crate::game::full_tile_set;
use crate::game::SolverState;
use crate::game::{best_exchange, top_exchanges};
use crate::solver::{Move, MoveKind, Solver, EXCHANGE_MIN_BAG};
#[cfg(test)]
use crate::tile::Rack;
use crate::tile::{Tile, RACK_SIZE};

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
    /// Set from another thread to stop the evaluation early (checked before
    /// every playout); the results then hold whatever was simulated.
    pub cancel: Option<Arc<AtomicBool>>,
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
            cancel: None,
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
            cancel: None,
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

    #[allow(dead_code)] // library API; the app drives `sim` instead
    pub fn with_score_diff(mut self, score_diff: i32) -> Self {
        self.score_diff = score_diff;
        self
    }

    /// Stop early once `cancel` is set.
    #[allow(dead_code)] // library API; the app drives `sim` instead
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(AtomicOrdering::Relaxed))
    }
}

/// Result of Monte Carlo evaluation for a single move
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))] // used by tests and library consumers
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

/// Model of the opponent for playouts: how their hidden rack is dealt and
/// how strongly they play. The default deals uniformly and plays perfect
/// greedy, which is exactly the behaviour without a model.
#[derive(Debug, Clone)]
pub struct OpponentModel {
    /// Per-letter sampling weight for the tiles the opponent kept after
    /// their last play, indexed by letter (26 = blank). The weight is the
    /// inferred expected count relative to a uniform deal's, so a value
    /// above 1 tilts the deal toward putting that letter in their rack.
    /// `None` deals uniformly.
    kept_weights: Option<[f32; 27]>,
    /// How many tiles they kept (7 minus the tiles they just played).
    kept_size: usize,
    /// Equity spread of the opponent's simulated move choice, in points:
    /// 0 plays the best move every turn, larger values err more, roughly
    /// matching a player who gives up `temperature` points per turn.
    pub temperature: f32,
}

impl OpponentModel {
    pub fn new(temperature: f32) -> Self {
        OpponentModel {
            kept_weights: None,
            kept_size: 0,
            temperature,
        }
    }

    /// Attach per-letter marginals from rack inference: `(letter,
    /// expected)` is the inferred expected count of each letter among the
    /// `kept_size` tiles the opponent kept ('?' for the blank). Weights
    /// are clamped so a noisy or stale inference can only tilt the deal,
    /// never make a holding impossible or certain.
    ///
    /// Sequential weighted sampling without replacement undershoots the
    /// target marginals (achieved expectations land between uniform and
    /// inferred); the tilt direction is what matters, exact fidelity would
    /// need a conditional-Poisson weight solve.
    pub fn with_marginals(
        mut self,
        marginals: &[(char, f32)],
        kept_size: usize,
        pool: &TilePool,
    ) -> Self {
        // Kept tiles beyond a rack would spill past the opponent's region
        // of the deal; no real caller keeps more than 6.
        let kept_size = kept_size.min(RACK_SIZE - 1);
        if marginals.is_empty() || kept_size == 0 || pool.is_empty() {
            return self;
        }
        let mut counts = [0u32; 27];
        for tile in &pool.tiles {
            counts[Self::index(tile)] += 1;
        }
        let mut weights = [1.0f32; 27];
        for &(letter, expected) in marginals {
            let idx = if letter == '?' {
                26
            } else if letter.is_ascii_uppercase() {
                (letter as u8 - b'A') as usize
            } else {
                continue;
            };
            if counts[idx] == 0 {
                continue;
            }
            let uniform = kept_size as f32 * counts[idx] as f32 / pool.len() as f32;
            weights[idx] = (expected / uniform).clamp(0.05, 20.0);
        }
        self.kept_weights = Some(weights);
        self.kept_size = kept_size;
        self
    }

    pub fn has_marginals(&self) -> bool {
        self.kept_weights.is_some()
    }

    fn index(tile: &Tile) -> usize {
        if tile.is_blank {
            26
        } else {
            (tile.letter as u8 - b'A') as usize
        }
    }

    /// Deal the pool for one playout, laid out as `[opponent rack | bag]`
    /// like a uniform shuffle; our draw comes off the top (the end) of the
    /// bag. The opponent's kept tiles are sampled first, by weight and
    /// without replacement, and open their rack, so they can never appear
    /// in our draw or the bag; everything else is dealt uniformly (their
    /// fresh draws are exchangeable with ours). The layout does not depend
    /// on the candidate, so every candidate in an iteration faces the same
    /// opponent rack.
    fn dealt(&self, pool: &TilePool, rng: &mut Rng) -> Vec<Tile> {
        let Some(weights) = &self.kept_weights else {
            return pool.shuffled(rng);
        };
        let mut rest = pool.tiles.clone();
        let mut kept = Vec::with_capacity(self.kept_size);
        for _ in 0..self.kept_size.min(rest.len()) {
            let total: f32 = rest.iter().map(|t| weights[Self::index(t)]).sum();
            let picked = if total > 0.0 {
                let mut roll = rng.f32() * total;
                let mut picked = rest.len() - 1;
                for (i, t) in rest.iter().enumerate() {
                    roll -= weights[Self::index(t)];
                    if roll <= 0.0 {
                        picked = i;
                        break;
                    }
                }
                picked
            } else {
                rng.usize(..rest.len())
            };
            kept.push(rest.swap_remove(picked));
        }
        rng.shuffle(&mut rest);
        kept.append(&mut rest);
        kept
    }

    /// Hash of the model's parameters, for keying cached simulations.
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fnv::new();
        h.write_u64(self.temperature.to_bits() as u64);
        h.write_u64(self.kept_size as u64);
        if let Some(w) = &self.kept_weights {
            for x in w {
                h.write_u64(x.to_bits() as u64);
            }
        }
        h.finish()
    }
}

/// FNV-1a, for small deterministic fingerprints (position and model keys,
/// playout seeds). Not for hash tables.
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) fn write_u64(&mut self, x: u64) {
        for b in x.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub(crate) fn finish(&self) -> u64 {
        self.0
    }
}

/// Fingerprint of a position: board tiles (with blank flags) and our rack.
/// Playout seeds derive from it, so a position replays the same deals in
/// every run, which lets a stopped simulation resume where it left off.
pub fn position_fingerprint(state: &SolverState) -> u64 {
    let mut h = Fnv::new();
    for r in 0..crate::board::BOARD_SIZE {
        for c in 0..crate::board::BOARD_SIZE {
            let code = match state.board.get_tile(r, c) {
                Some(t) => (t.letter as u64) << 1 | t.is_blank as u64,
                None => 0,
            };
            h.write_u64(code);
        }
    }
    let mut rack: Vec<(bool, char)> = state
        .rack
        .tiles()
        .iter()
        .map(|t| (t.is_blank, t.letter))
        .collect();
    rack.sort_unstable();
    for (b, l) in rack {
        h.write_u64((l as u64) << 1 | b as u64);
    }
    h.finish()
}

#[cfg(test)]
fn rack_from(tiles: &[Tile]) -> Rack {
    let mut rack = Rack::new();
    for t in tiles {
        rack.add(*t);
    }
    rack
}

/// Moves the softmax opponent chooses among (the best by equity, within a
/// window of the best).
const SOFTMAX_MOVES: usize = 16;

/// Which player is to move in a playout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Softmax spread of the opponent's greedy-phase move choice, in
    /// equity points; 0 (the default) plays the best move every turn.
    opp_temperature: f32,
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
            opp_temperature: 0.0,
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
        }
        if let MoveKind::Exchange(returned) = &mv.kind {
            for &tile in returned {
                let at = self.rng.usize(..=self.bag.len());
                self.bag.insert(at, tile);
            }
        }
        // Only a play can draw the last tile: an exchange puts back as many
        // as it took, so the bag is checked after the return.
        if need > 0 && self.bag.is_empty() {
            self.final_turn_owner = Some(p);
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
        Solver::new(&self.board, self.dict).best_move_by_score(self.rack(p))
    }

    /// Best move by static equity for `p`, including exchanges when the bag
    /// allows them (None means pass).
    fn best_by_equity(&self, p: Player) -> Option<Move> {
        let solver = Solver::new(&self.board, self.dict);
        let best_play = solver.best_move_by_equity(self.rack(p));
        if self.bag.len() < EXCHANGE_MIN_BAG {
            return best_play;
        }
        let unseen = self.unseen_for_phase();
        let best_exchange = best_exchange(self.rack(p), self.dict, Some(unseen));
        match (best_play, best_exchange) {
            (Some(play), Some(exch)) => Some(if exch.equity > play.equity {
                exch
            } else {
                play
            }),
            (play, exch) => play.or(exch),
        }
    }

    /// Move choice in the greedy phase: exact best for us, and for the
    /// opponent a softmax sample over near-best moves when the model says
    /// they err (`opp_temperature` > 0). The final two turns after the bag
    /// empties stay exact for both players regardless.
    fn policy_move(&mut self, p: Player) -> Option<Move> {
        if p != Player::Opp || self.opp_temperature <= 0.0 {
            return self.best_by_equity(p);
        }
        let tau = self.opp_temperature;
        // Only the best SOFTMAX_MOVES of plays and of exchanges can make the
        // cut below, so only those are built.
        let mut moves = {
            let solver = Solver::new(&self.board, self.dict);
            let mut moves = solver.top_moves_by_equity(self.rack(p), SOFTMAX_MOVES);
            if self.bag.len() >= EXCHANGE_MIN_BAG {
                let unseen = self.unseen_for_phase();
                moves.extend(top_exchanges(
                    self.rack(p),
                    self.dict,
                    Some(unseen),
                    SOFTMAX_MOVES,
                ));
                moves.sort_by(|a, b| {
                    b.equity
                        .partial_cmp(&a.equity)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            moves
        };
        let best_equity = moves.first()?.equity;
        // Errors far below the best are not plausible even for a weak
        // player; without the window the long tail of junk moves would
        // dominate the sample by sheer count.
        moves.retain(|m| m.equity >= best_equity - 3.0 * tau);
        moves.truncate(SOFTMAX_MOVES);
        let weights: Vec<f32> = moves
            .iter()
            .map(|m| ((m.equity - best_equity) / tau).exp())
            .collect();
        let total: f32 = weights.iter().sum();
        let mut roll = self.rng.f32() * total;
        for (i, w) in weights.iter().enumerate() {
            roll -= w;
            if roll <= 0.0 {
                return Some(moves.swap_remove(i));
            }
        }
        moves.pop()
    }

    /// `p`'s last turn with `p.other()` holding the final move: choose the
    /// play maximising score minus the other's best reply, or pass.
    fn best_last_turn(&self, p: Player) -> (Option<Move>, i32) {
        let solver = Solver::new(&self.board, self.dict);
        let other_rack = self.rack(p.other());
        let mut moves = solver.find_moves_for(self.rack(p));
        moves.sort_by_key(|m| std::cmp::Reverse(m.score));

        // Only the reply's score matters, so no reply moves are built.
        let reply_score =
            |board: &Board| -> i32 { Solver::new(board, self.dict).best_score(other_rack) as i32 };

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
            if let Some(mv) = self.policy_move(to_move) {
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
pub(crate) struct CandidateSim<'a> {
    mv: &'a Move,
    board: Board,
    our_leave: Vec<Tile>,
    pool: TilePool,
    bag_size: usize,
    draw_count: usize,
    score_diff: i32,
    model: Option<&'a OpponentModel>,
    /// Base seed of this evaluation's playouts (see `run_batch`).
    seed: u64,
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
    pub(crate) fn new(
        state: &SolverState,
        mv: &'a Move,
        score_diff: i32,
        model: Option<&'a OpponentModel>,
    ) -> Self {
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
            model,
            seed: iteration_seed(PLAYOUT_SEED, position_fingerprint(state)),
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
        self.wins += win_value(self.score_diff, &outcome);
        self.iterations += 1;
    }

    /// Run playouts `start..start + count` (iteration numbers) and return
    /// what they found, without touching the accumulators, so batches of
    /// one candidate can run in parallel. Stops early at the deadline or on
    /// cancellation; the returned batch is then shorter.
    ///
    /// Playout `i` draws from its own generator seeded by `i` alone, the
    /// same for every candidate: common random numbers. Each iteration
    /// faces the same deal whichever candidate is played, which tightens
    /// comparisons between candidates, makes runs reproducible, and does
    /// not depend on how the work is split between threads.
    pub(crate) fn run_batch(
        &self,
        dict: &Dictionary,
        start: usize,
        count: usize,
        max_plies: usize,
        deadline: Option<Instant>,
        cancel: Option<&AtomicBool>,
    ) -> Vec<PlayoutRecord> {
        let mut out = Vec::with_capacity(count);
        if self.pool.is_empty() {
            // Opponent has nothing: our move ends the game.
            out.resize(
                count,
                PlayoutRecord {
                    outcome: PlayoutOutcome {
                        value: self.mv.score as f32,
                        finished: true,
                        bag: 0,
                    },
                    opp_bingo: false,
                    our_bingo: false,
                },
            );
            return out;
        }

        for i in start..start + count {
            if deadline.is_some_and(|d| Instant::now() >= d)
                || cancel.is_some_and(|c| c.load(AtomicOrdering::Relaxed))
            {
                break;
            }
            // Common random numbers: everything random in iteration i comes
            // from this generator, and it is consumed the same way for
            // every candidate (the deal, then two seeds), so all candidates
            // face the same opponent rack and the same bag order.
            let mut rng = Rng::with_seed(iteration_seed(self.seed, i as u64));
            let dealt = match self.model {
                Some(m) if m.has_marginals() => m.dealt(&self.pool, &mut rng),
                _ => self.pool.shuffled(&mut rng),
            };
            let playout_seed = rng.u64(..);
            let mut exchange_rng = Rng::with_seed(rng.u64(..));

            let split = dealt.len().min(RACK_SIZE);
            let opp_rack = dealt[..split].to_vec();
            let mut bag = dealt[split..].to_vec();
            // Our draw comes off the top of the bag, where every later
            // draw comes from too.
            let drawn = bag.split_off(bag.len() - self.draw_count.min(bag.len()));
            let mut our_rack = self.our_leave.clone();
            our_rack.extend_from_slice(&drawn);
            if let MoveKind::Exchange(returned) = &self.mv.kind {
                // Exchanged tiles go back after we draw and the bag is
                // reshuffled, with the iteration's own generator.
                bag.extend_from_slice(returned);
                exchange_rng.shuffle(&mut bag);
            }

            let seed = playout_seed;
            let mut playout =
                Playout::new(dict, self.board.clone(), [our_rack, opp_rack], bag, seed);
            playout.spread = self.mv.score as i32;
            playout.opp_temperature = self.model.map(|m| m.temperature).unwrap_or(0.0);
            // If our move emptied the bag, we own the final turn: the
            // opponent gets one last turn and then we play the final move.
            if playout.bag.is_empty() && self.bag_size > 0 {
                playout.final_turn_owner = Some(Player::Us);
            }

            let outcome = playout.run_with(Player::Opp, max_plies, &mut |_, _| {});
            out.push(PlayoutRecord {
                outcome,
                opp_bingo: playout.first_move_bingo[Player::Opp as usize] == Some(true),
                our_bingo: playout.first_move_bingo[Player::Us as usize] == Some(true),
            });
        }
        out
    }

    /// Add a batch's playouts to the accumulators, in iteration order.
    fn absorb(&mut self, records: &[PlayoutRecord]) {
        for r in records {
            self.opp_bingos += u32::from(r.opp_bingo);
            self.our_bingos += u32::from(r.our_bingo);
            self.record(r.outcome);
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
/// 1000 games, 2026-08-20, with racks valued through the leave table): with
/// the bag holding 8+ tiles, the final spread minus (spread so far + leave
/// values) averaged +18.5 / +22.7 with us to move and -22.5 / -18.7 with the
/// opponent to move, whichever side had started the game (earlier runs of
/// 400-800 games gave 17.6-23.2). The same offset applies to every
/// candidate, so it changes win estimates, not rankings.
const TEMPO: f32 = 21.0;

/// Standard deviation of the spread still to come from a mid-game
/// position, by tiles left in the bag. Same study: the variance of the
/// residual grows linearly with the bag, slope 79.4-79.8 per tile across
/// runs and an intercept between 92 and 213 (few samples at small bags,
/// where playouts usually finish exactly anyway): var = 150 + 79.8 * bag
/// (sd 12 with an empty bag, 64 with 50 tiles, 84 at the start).
fn remaining_spread_sd(bag: usize) -> f32 {
    (150.0 + 79.8 * bag as f32).sqrt()
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

/// What one playout reports back to its candidate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlayoutRecord {
    pub(crate) outcome: PlayoutOutcome,
    pub(crate) opp_bingo: bool,
    pub(crate) our_bingo: bool,
}

/// A playout's contribution to the win rate: 1, 0 or 1/2 for a finished
/// game (counting the score difference before the move), and for a game
/// cut off mid-way the probability that the remaining spread swing leaves
/// us ahead.
pub(crate) fn win_value(score_diff: i32, outcome: &PlayoutOutcome) -> f32 {
    let final_diff = score_diff as f32 + outcome.value;
    if outcome.finished {
        match final_diff.partial_cmp(&0.0) {
            Some(std::cmp::Ordering::Greater) => 1.0,
            Some(std::cmp::Ordering::Less) => 0.0,
            _ => 0.5,
        }
    } else {
        normal_cdf(final_diff / remaining_spread_sd(outcome.bag))
    }
}

/// Base seed of the playout generators (see [`CandidateSim::run_batch`]).
const PLAYOUT_SEED: u64 = 0x5EED_C0DE;

/// Playouts per scheduled job: small enough to spread a few candidates
/// over every core, large enough to amortise scheduling.
const PLAYOUT_BATCH: usize = 8;

/// SplitMix64 finaliser: a well-mixed seed for iteration `i`.
fn iteration_seed(base: u64, i: u64) -> u64 {
    let mut z = base ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Result of one playout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlayoutOutcome {
    /// Spread change from our point of view (exact if `finished`, otherwise
    /// spread so far plus leave values plus the tempo of the side to move).
    pub(crate) value: f32,
    /// The game reached its end inside the ply limit.
    pub(crate) finished: bool,
    /// Tiles left in the bag where the playout stopped.
    pub(crate) bag: usize,
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
    #[cfg_attr(not(test), allow(dead_code))] // used by tests and library consumers
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
    #[cfg_attr(not(test), allow(dead_code))] // the model-free API, kept for tests and callers
    pub fn evaluate(
        &self,
        state: &SolverState,
        candidates: &[Move],
        dictionary: &Dictionary,
    ) -> Vec<MctsResult> {
        self.evaluate_with_model(state, candidates, dictionary, None)
    }

    /// As [`Self::evaluate`], with an opponent model biasing how their
    /// rack is dealt and how strongly they play in the playouts.
    pub fn evaluate_with_model(
        &self,
        state: &SolverState,
        candidates: &[Move],
        dictionary: &Dictionary,
        model: Option<&OpponentModel>,
    ) -> Vec<MctsResult> {
        let num_to_evaluate = candidates.len().min(self.config.num_candidates);
        let mut sims: Vec<CandidateSim> = candidates[..num_to_evaluate]
            .iter()
            .map(|mv| CandidateSim::new(state, mv, self.config.score_diff, model))
            .collect();

        // Quarter, quarter, half: two pruning points.
        let total = self.config.iterations_per_candidate;
        let rounds = [total / 4, total / 4, total - 2 * (total / 4)];
        let mut active: Vec<usize> = (0..sims.len()).collect();
        let max_plies = self.config.max_plies;
        let deadline = self.config.time_budget.map(|b| Instant::now() + b);
        let cancel = self.config.cancel.as_deref();

        for (round, &playouts) in rounds.iter().enumerate() {
            if playouts == 0 {
                continue;
            }
            // One job per (candidate, batch of playouts), so every core
            // stays busy even when only a few candidates are left.
            let mut jobs: Vec<(usize, usize, usize)> = Vec::new();
            for &ci in &active {
                let first = sims[ci].iterations;
                let mut start = first;
                while start < first + playouts {
                    let n = PLAYOUT_BATCH.min(first + playouts - start);
                    jobs.push((ci, start, n));
                    start += n;
                }
            }
            let run = |&(ci, start, n): &(usize, usize, usize)| {
                let records = sims[ci].run_batch(dictionary, start, n, max_plies, deadline, cancel);
                (ci, start, n, records)
            };
            let mut batches: Vec<(usize, usize, usize, Vec<PlayoutRecord>)> =
                if self.config.parallel {
                    jobs.par_iter().map(run).collect()
                } else {
                    jobs.iter().map(run).collect()
                };
            // Keep each candidate's playouts contiguous in iteration order:
            // after a batch cut short (deadline or cancel), later batches of
            // that candidate are dropped, so iteration i means the same
            // deal for every candidate in the paired comparisons.
            batches.sort_by_key(|&(ci, start, _, _)| (ci, start));
            let mut cut = vec![false; sims.len()];
            for (ci, _, n, records) in &batches {
                if cut[*ci] {
                    continue;
                }
                sims[*ci].absorb(records);
                if records.len() < *n {
                    cut[*ci] = true;
                }
            }
            if deadline.is_some_and(|d| Instant::now() >= d) || self.config.is_cancelled() {
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
        // Fully simulated first, then candidates cut short, then any that
        // never got a playout (their numbers are placeholders).
        let tier = |r: &MctsResult| match r.iterations {
            0 => 0,
            n if n == full => 2,
            _ => 1,
        };
        results.sort_by(|a, b| {
            tier(b).cmp(&tier(a)).then(
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

    /// A marginal saying the opponent kept Z more often than a uniform
    /// deal would must raise Z's frequency in their dealt rack, without
    /// changing which tiles are dealt overall.
    #[test]
    fn opponent_model_biases_the_deal() {
        let mut remaining = std::collections::HashMap::new();
        remaining.insert('A', 5);
        remaining.insert('E', 5);
        remaining.insert('B', 8);
        remaining.insert('Z', 2);
        let pool = TilePool::from_remaining(&remaining);
        assert_eq!(pool.len(), 20);

        let kept_size = 6;
        // Uniform expectation of Z among 6 kept tiles is 6 * 2/20 = 0.6;
        // the marginal says 1.5.
        let model = OpponentModel::new(0.0).with_marginals(&[('Z', 1.5)], kept_size, &pool);
        assert!(model.has_marginals());

        let mut rng = Rng::with_seed(7);
        let deals = 2000;
        let mut z_in_opp = 0usize;
        for i in 0..deals {
            let dealt = model.dealt(&pool, &mut rng);
            assert_eq!(dealt.len(), pool.len());
            if i == 0 {
                let mut sorted: Vec<char> = dealt.iter().map(|t| t.letter).collect();
                sorted.sort_unstable();
                let mut expected: Vec<char> = pool.tiles.iter().map(|t| t.letter).collect();
                expected.sort_unstable();
                assert_eq!(sorted, expected, "a deal must be a permutation of the pool");
            }
            z_in_opp += dealt[..RACK_SIZE]
                .iter()
                .filter(|t| t.letter == 'Z')
                .count();
        }
        // A uniform deal puts 7 * 2/20 = 0.7 Z's in their rack on average;
        // the tilt should push it well above that.
        let mean = z_in_opp as f32 / deals as f32;
        assert!(mean > 0.95, "expected biased Z mean above 0.95, got {mean}");
    }

    /// With a temperature the simulated opponent errs sometimes but still
    /// prefers better moves; our own side stays exact.
    #[test]
    fn opponent_policy_errs_at_temperature() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let mut playout = Playout::new_game(&dict, 42);

        let best = playout.best_by_equity(Player::Opp).unwrap();
        let pick = playout.policy_move(Player::Opp).unwrap();
        assert_eq!(
            (pick.word.clone(), pick.score),
            (best.word.clone(), best.score),
            "zero temperature must play the greedy move"
        );

        playout.opp_temperature = 8.0;
        let samples = 300;
        let mut erred = 0usize;
        let mut best_picked = 0usize;
        let mut total_gap = 0.0f32;
        for _ in 0..samples {
            let mv = playout.policy_move(Player::Opp).unwrap();
            let gap = best.equity - mv.equity;
            assert!(gap >= -1e-4, "sampled move cannot beat the best");
            assert!(gap <= 3.0 * 8.0 + 1e-4, "window bounds the error");
            total_gap += gap.max(0.0);
            if gap > 1e-6 {
                erred += 1;
            } else {
                best_picked += 1;
            }
        }
        assert!(erred > 0, "a warm opponent must err sometimes");
        assert!(
            best_picked > samples / 16,
            "the best move should stay the most likely pick"
        );
        assert!(
            total_gap / samples as f32 <= 2.0 * 8.0,
            "mean error should be on the order of the temperature"
        );

        let our_best = playout.best_by_equity(Player::Us).unwrap();
        let our_pick = playout.policy_move(Player::Us).unwrap();
        assert_eq!(
            (our_pick.word, our_pick.score),
            (our_best.word, our_best.score),
            "our own playout policy stays exact"
        );
    }

    /// `evaluate` and `evaluate_with_model(None)` are the same computation.
    #[test]
    fn evaluate_without_model_is_unchanged() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let mut state = SolverState::new();
        for ch in ['C', 'A', 'T', 'S', 'E', 'R', 'N'] {
            state.add_to_rack(ch);
        }
        let suggestions = state.get_suggestions(&dict);
        let config = MctsConfig {
            num_candidates: 3,
            iterations_per_candidate: 6,
            max_plies: 2,
            parallel: false,
            score_diff: 0,
            time_budget: None,
            cancel: None,
        };
        let evaluator = MctsEvaluator::new(config);
        let plain = evaluator.evaluate(&state, &suggestions, &dict);
        let with_none = evaluator.evaluate_with_model(&state, &suggestions, &dict, None);
        let key = |r: &MctsResult| (r.mv.word.clone(), r.mcts_equity, r.win_rate);
        assert_eq!(
            plain.iter().map(key).collect::<Vec<_>>(),
            with_none.iter().map(key).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_mcts_evaluator_basic() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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
            cancel: None,
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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
            cancel: None,
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
        assert!(remaining_spread_sd(0) > 11.0 && remaining_spread_sd(0) < 13.0);
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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
            cancel: None,
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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
            cancel: None,
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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

    /// Calibration of the observed-move-loss estimator: self-play games
    /// where the opponent plays at a known softmax temperature, recording
    /// their true equity loss per move (their rack is known here) next to
    /// what `observed_move_loss` measures blind from sampled completions.
    /// The linear fit measured = a + b*tau justifies (or corrects) how the
    /// app maps a measured loss to a playout temperature.
    /// Run with `cargo test --release study_loss_calibration -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn study_loss_calibration() {
        use crate::game::observed_move_loss;

        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let games: usize = std::env::var("GAMES")
            .ok()
            .and_then(|g| g.parse().ok())
            .unwrap_or(24);
        let taus = [0.0f32, 4.0, 8.0, 12.0];
        let started = Instant::now();

        let full_counts = {
            let mut counts = std::collections::HashMap::new();
            for t in full_tile_set() {
                let c = if t.is_blank { '?' } else { t.letter };
                *counts.entry(c).or_insert(0u32) += 1;
            }
            counts
        };
        // The unseen pool from our side of the table after the opponent's
        // move: everything minus the board and our own rack.
        let pool_after = |g: &Playout| {
            let mut counts = full_counts.clone();
            for r in 0..crate::board::BOARD_SIZE {
                for c in 0..crate::board::BOARD_SIZE {
                    if let Some(t) = g.board.get_tile(r, c) {
                        let key = if t.is_blank { '?' } else { t.letter };
                        if let Some(n) = counts.get_mut(&key) {
                            *n = n.saturating_sub(1);
                        }
                    }
                }
            }
            for t in g.rack(Player::Us) {
                let key = if t.is_blank { '?' } else { t.letter };
                if let Some(n) = counts.get_mut(&key) {
                    *n = n.saturating_sub(1);
                }
            }
            counts
        };

        let mut fit_points = Vec::new();
        for &tau in &taus {
            let per_game: Vec<(Vec<f32>, Vec<f32>)> = (0..games as u64)
                .into_par_iter()
                .map(|seed| {
                    let mut g = Playout::new_game(&dict, 9_000 + seed);
                    g.opp_temperature = tau;
                    let mut to_move = if seed % 2 == 0 {
                        Player::Us
                    } else {
                        Player::Opp
                    };
                    let mut true_losses = Vec::new();
                    let mut measured = Vec::new();
                    for ply in 0..40u64 {
                        if g.final_turn_owner.is_some() || g.bag.is_empty() {
                            break;
                        }
                        if to_move == Player::Opp && g.rack(Player::Opp).len() == RACK_SIZE {
                            let best = g.best_by_equity(Player::Opp);
                            let prev_board = g.board.clone();
                            let chosen = g.policy_move(Player::Opp);
                            let (Some(best), Some(chosen)) = (best, chosen) else {
                                break;
                            };
                            true_losses.push((best.equity - chosen.equity).max(0.0));
                            let is_play = matches!(chosen.kind, MoveKind::Play);
                            g.commit(Player::Opp, &chosen);
                            if is_play {
                                if let Some(m) = observed_move_loss(
                                    &prev_board,
                                    &chosen.tiles_used,
                                    chosen.score,
                                    &pool_after(&g),
                                    &dict,
                                    10,
                                    seed * 101 + ply,
                                ) {
                                    measured.push(m);
                                }
                            }
                        } else if let Some(mv) = g.policy_move(to_move) {
                            g.commit(to_move, &mv);
                        } else {
                            break;
                        }
                        to_move = to_move.other();
                    }
                    (true_losses, measured)
                })
                .collect();

            let true_all: Vec<f32> = per_game.iter().flat_map(|(t, _)| t.clone()).collect();
            let meas_all: Vec<f32> = per_game.iter().flat_map(|(_, m)| m.clone()).collect();
            let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
            println!(
                "tau {:>4.1}: {} opp moves, true loss {:>5.2}, measured {:>5.2} ({} measured)",
                tau,
                true_all.len(),
                mean(&true_all),
                mean(&meas_all),
                meas_all.len(),
            );
            fit_points.push((tau as f64, mean(&meas_all) as f64));
        }

        // Least-squares measured = a + b*tau.
        let n = fit_points.len() as f64;
        let sx: f64 = fit_points.iter().map(|(x, _)| x).sum();
        let sy: f64 = fit_points.iter().map(|(_, y)| y).sum();
        let sxx: f64 = fit_points.iter().map(|(x, _)| x * x).sum();
        let sxy: f64 = fit_points.iter().map(|(x, y)| x * y).sum();
        let b = (n * sxy - sx * sy) / (n * sxx - sx * sx);
        let a = (sy - b * sx) / n;
        println!(
            "fit: measured = {:.2} + {:.3}*tau  =>  temperature = (measured - {:.2}) / {:.3}",
            a, b, a, b
        );
        println!("total {:?}", started.elapsed());
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
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
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

    fn tiles_of(s: &str) -> Vec<Tile> {
        s.chars()
            .map(|c| {
                if c == '?' {
                    Tile::blank(' ')
                } else {
                    Tile::new(c)
                }
            })
            .collect()
    }
    fn multiset(v: &[Tile]) -> Vec<(bool, char)> {
        let mut k: Vec<(bool, char)> = v
            .iter()
            .map(|x| (x.is_blank, if x.is_blank { '?' } else { x.letter }))
            .collect();
        k.sort();
        k
    }

    /// An exchange in a playout: the exchanged tiles leave the rack, as many
    /// are drawn, the bag keeps its size, nothing is lost, and the bag is
    /// never "emptied" by it.
    #[test]
    fn playout_exchange_conserves_tiles_and_bag_size() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let bag = tiles_of("EEEEEEEEEE");
        let mut p = Playout::new(
            &dict,
            Board::new(),
            [tiles_of("QVWXZ?A"), tiles_of("AAAAAAA")],
            bag,
            7,
        );
        let before: Vec<Tile> = p.racks[0]
            .iter()
            .chain(&p.racks[1])
            .chain(&p.bag)
            .copied()
            .collect();
        let mv = Move::exchange(&p.racks[0], tiles_of("QV?"), 0.0);
        p.commit(Player::Us, &mv);
        assert_eq!(p.racks[0].len(), 7);
        assert_eq!(p.bag.len(), 10);
        assert!(p.final_turn_owner.is_none());
        assert_eq!(p.spread, 0);
        // Drew three E's (the bag was all E's), returned Q V ?.
        assert_eq!(multiset(&p.racks[0]), multiset(&tiles_of("WXZAEEE")));
        let after: Vec<Tile> = p.racks[0]
            .iter()
            .chain(&p.racks[1])
            .chain(&p.bag)
            .copied()
            .collect();
        assert_eq!(multiset(&before), multiset(&after));
        assert!(p.bag.iter().any(|t| t.is_blank) && p.bag.iter().any(|t| t.letter == 'Q'));
    }

    /// A seven-tile exchange with exactly seven tiles in the bag must not
    /// count as drawing the last tile (regression: the emptiness check ran
    /// before the exchanged tiles went back).
    #[test]
    fn full_exchange_with_bag_of_seven_does_not_claim_final_turn() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let mut p = Playout::new(
            &dict,
            Board::new(),
            [tiles_of("UUVVWWI"), tiles_of("AAAAAAA")],
            tiles_of("EEEEEEE"),
            9,
        );
        let mv = Move::exchange(&p.racks[0], tiles_of("UUVVWWI"), 0.0);
        p.commit(Player::Us, &mv);
        assert_eq!(p.bag.len(), 7);
        assert!(p.final_turn_owner.is_none());
        assert_eq!(multiset(&p.racks[0]), multiset(&tiles_of("EEEEEEE")));
    }

    /// The three ways the bag can empty, each followed by exactly one more
    /// move for the other side and then the emptier's final move.
    #[test]
    fn playout_endings_follow_the_equal_turn_sequence() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let board = Board::new();
        let mut b = board.clone();
        b.place_tile(7, 6, Tile::new('C'));
        b.place_tile(7, 7, Tile::new('A'));
        b.place_tile(7, 8, Tile::new('T'));

        // (a) Our candidate emptied the bag: owner = Us from the start.
        let mut p = Playout::new(
            &dict,
            b.clone(),
            [tiles_of("RE"), tiles_of("SOD")],
            vec![],
            1,
        );
        p.final_turn_owner = Some(Player::Us);
        p.trace = Some(vec![]);
        let out = p.run_with(Player::Opp, 10, &mut |_, _| {});
        assert!(out.finished);
        let tr = p.trace.take().unwrap();
        assert_eq!(tr.len(), 2, "{:?}", tr);
        assert!(!tr[0].mover_is_us && tr[1].mover_is_us);

        // (b) The opponent's first move empties a one-tile bag: they own the
        // final turn; we get one last turn, then they play, then it ends.
        let mut p = Playout::new(
            &dict,
            b.clone(),
            [tiles_of("RE"), tiles_of("SOD")],
            tiles_of("E"),
            2,
        );
        p.trace = Some(vec![]);
        let out = p.run_with(Player::Opp, 20, &mut |_, _| {});
        assert!(out.finished);
        assert_eq!(p.final_turn_owner, Some(Player::Opp));
        let tr = p.trace.take().unwrap();
        assert_eq!(tr.len(), 3, "{:?}", tr);
        assert!(!tr[0].mover_is_us && tr[1].mover_is_us && !tr[2].mover_is_us);
        assert_eq!(tr[0].bag, 0);

        // (c) The opponent passes (no tiles), our move empties the bag: we
        // own it; the opponent (still no tiles) passes their last turn and
        // we play the final move.
        let mut p = Playout::new(
            &dict,
            b.clone(),
            [tiles_of("RES"), vec![]],
            tiles_of("D"),
            3,
        );
        p.trace = Some(vec![]);
        let out = p.run_with(Player::Opp, 20, &mut |_, _| {});
        assert!(out.finished);
        assert_eq!(p.final_turn_owner, Some(Player::Us));
        let tr = p.trace.take().unwrap();
        // Our emptying move, then our final move; the opponent's turns are
        // passes (recorded in the greedy phase, silent on their last turn)
        // that never change the spread.
        assert_eq!(tr.iter().filter(|t| t.mover_is_us).count(), 2, "{:?}", tr);
        for w in tr.windows(2) {
            if !w[1].mover_is_us {
                assert_eq!(w[0].spread, w[1].spread);
            }
        }
    }

    /// Exchange candidates draw from the pre-move pool and the exchanged
    /// tiles end up back in the bag, so every playout conserves tiles; the
    /// evaluator handles them alongside plays.
    #[test]
    fn exchange_candidates_evaluate() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };
        let state =
            SolverState::from_cgp("15/15/15/15/15/15/15/3BEECH7/7I7/7K7/7E7/7r7/15/15/15 QVWXZ?A")
                .unwrap();
        let candidates = state.get_suggestions(&dict);
        let results = MctsEvaluator::new(MctsConfig {
            num_candidates: 12,
            iterations_per_candidate: 8,
            max_plies: 2,
            parallel: false,
            score_diff: 0,
            time_budget: None,
            cancel: None,
        })
        .evaluate(&state, &candidates, &dict);
        assert!(results
            .iter()
            .any(|r| matches!(r.mv.kind, MoveKind::Exchange(_))));
        for r in &results {
            assert!(r.mcts_equity.is_finite());
            assert!((0.0..=1.0).contains(&r.win_rate));
            assert!((0.0..=1.0).contains(&r.opp_bingo_rate));
        }
        // Direct check of the dealing for an exchange candidate.
        let ex = candidates
            .iter()
            .find(|m| matches!(m.kind, MoveKind::Exchange(_)))
            .unwrap();
        let sim = CandidateSim::new(&state, ex, 0, None);
        let MoveKind::Exchange(exchanged) = &ex.kind else {
            unreachable!()
        };
        assert_eq!(sim.draw_count, exchanged.len());
        // Pool = unseen before the move = 100 - 9 board - 7 rack = 84.
        assert_eq!(sim.pool.len(), 84);
        assert_eq!(sim.bag_size, 77);
    }
}
