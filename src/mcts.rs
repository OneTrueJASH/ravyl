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

use fastrand::Rng;
use rayon::prelude::*;

use crate::board::Board;
use crate::dictionary::Dictionary;
use crate::game::SolverState;
use crate::solver::{Move, Solver};
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
}

impl Default for MctsConfig {
    fn default() -> Self {
        MctsConfig {
            num_candidates: 20,
            iterations_per_candidate: 200,
            max_plies: 2,
            parallel: true,
            score_diff: 0,
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
}

impl<'a> Playout<'a> {
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

    /// Remove `mv`'s tiles from `p`'s rack, place them, and draw.
    fn commit(&mut self, p: Player, mv: &Move) {
        for (r, c, tile) in &mv.tiles_used {
            self.board.place_tile(*r, *c, *tile);
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
    }

    /// Highest-scoring move for `p` (None means pass).
    fn best_by_score(&self, p: Player) -> Option<Move> {
        let solver = Solver::new(&self.board, self.dict);
        let rack = rack_from(self.rack(p));
        solver.find_moves(&rack).into_iter().max_by_key(|m| m.score)
    }

    /// Best move by static equity for `p` (None means pass).
    fn best_by_equity(&self, p: Player) -> Option<Move> {
        let solver = Solver::new(&self.board, self.dict);
        let rack = rack_from(self.rack(p));
        solver.find_moves(&rack).into_iter().next()
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

    /// Play the game out from `to_move`'s turn. Returns the final spread
    /// (plus leave adjustments if the ply limit cut the game short).
    fn run(&mut self, mut to_move: Player, max_plies: usize) -> f32 {
        let mut plies = 0;
        loop {
            match self.final_turn_owner {
                Some(owner) if owner == to_move => {
                    // Final move of the game: take the points.
                    if let Some(mv) = self.best_by_score(to_move) {
                        self.commit(to_move, &mv);
                    }
                    return self.spread as f32;
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
                // Cut short: value the racks statically.
                let unseen = self.unseen_for_phase();
                let ours = self.dict.leave_value(self.rack(Player::Us), Some(unseen));
                let theirs = self.dict.leave_value(self.rack(Player::Opp), Some(unseen));
                return self.spread as f32 + ours - theirs;
            }

            if let Some(mv) = self.best_by_equity(to_move) {
                self.commit(to_move, &mv);
            } else if self.bag.is_empty() {
                // No bag, no move: treat as the game ending here.
                return self.spread as f32;
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
}

impl<'a> CandidateSim<'a> {
    fn new(state: &SolverState, mv: &'a Move, score_diff: i32) -> Self {
        let mut base_state = state.clone();
        base_state.apply_move(mv);
        let our_leave: Vec<Tile> = base_state.rack.tiles();

        // Unseen tiles after our move = bag + opponent's rack. While the bag
        // has tiles the opponent holds a full rack.
        let pool = TilePool::from_remaining(&base_state.unseen_tiles());
        let bag_size = pool.len().saturating_sub(RACK_SIZE);
        let draw_count = mv.tiles_used.len().min(bag_size);

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
        }
    }

    fn mean(&self) -> f32 {
        if self.iterations == 0 {
            self.mv.score as f32
        } else {
            self.total / self.iterations as f32
        }
    }

    fn record(&mut self, value: f32) {
        self.total += value;
        let final_diff = self.score_diff as f32 + value;
        if final_diff > 0.0 {
            self.wins += 1.0;
        } else if final_diff == 0.0 {
            self.wins += 0.5;
        }
        self.iterations += 1;
    }

    fn run(&mut self, dict: &Dictionary, playouts: usize, max_plies: usize) {
        if self.pool.is_empty() {
            // Opponent has nothing: our move ends the game.
            for _ in 0..playouts {
                self.record(self.mv.score as f32);
            }
            return;
        }

        for _ in 0..playouts {
            let shuffled = self.pool.shuffled(&mut self.rng);
            let (drawn, rest) = shuffled.split_at(self.draw_count);
            let (opp_rack, bag) = rest.split_at(rest.len().min(RACK_SIZE));

            let mut our_rack = self.our_leave.clone();
            our_rack.extend_from_slice(drawn);

            let mut playout = Playout {
                dict,
                board: self.board.clone(),
                racks: [our_rack, opp_rack.to_vec()],
                bag: bag.to_vec(),
                spread: self.mv.score as i32,
                final_turn_owner: None,
            };
            // If our move emptied the bag, we own the final turn: the
            // opponent gets one last turn and then we play the final move.
            if playout.bag.is_empty() && self.bag_size > 0 {
                playout.final_turn_owner = Some(Player::Us);
            }

            let value = playout.run(Player::Opp, max_plies);
            self.record(value);
        }
    }

    fn result(&self) -> MctsResult {
        MctsResult {
            mv: self.mv.clone(),
            mcts_equity: self.mean(),
            win_rate: if self.iterations == 0 {
                0.5
            } else {
                self.wins / self.iterations as f32
            },
            iterations: self.iterations,
        }
    }
}

/// Candidates trailing the leader by more than this many points after a
/// pruning round are not simulated further.
const PRUNE_MARGIN: f32 = 4.0;

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
    /// leader by more than [`PRUNE_MARGIN`] points stop (keeping their
    /// estimate so far), so the budget concentrates on the moves that can
    /// still finish first.
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

        for (round, &playouts) in rounds.iter().enumerate() {
            if playouts == 0 {
                continue;
            }
            if self.config.parallel {
                let active_set: std::collections::HashSet<usize> = active.iter().copied().collect();
                sims.par_iter_mut()
                    .enumerate()
                    .filter(|(i, _)| active_set.contains(i))
                    .for_each(|(_, sim)| sim.run(dictionary, playouts, max_plies));
            } else {
                for &i in &active {
                    sims[i].run(dictionary, playouts, max_plies);
                }
            }

            if round + 1 < rounds.len() {
                active.sort_by(|&a, &b| {
                    sims[b]
                        .mean()
                        .partial_cmp(&sims[a].mean())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let leader = sims[active[0]].mean();
                let contenders = active
                    .iter()
                    .filter(|&&i| sims[i].mean() >= leader - PRUNE_MARGIN)
                    .count();
                let keep = contenders.max(4).min(active.len());
                active.truncate(keep);
            }
        }

        let mut results: Vec<MctsResult> = sims.iter().map(|s| s.result()).collect();
        results.sort_by(|a, b| {
            b.mcts_equity
                .partial_cmp(&a.mcts_equity)
                .unwrap_or(std::cmp::Ordering::Equal)
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
        let mut playout = Playout {
            dict: &dict,
            board: board.clone(),
            racks: [
                vec![Tile::new('S'), Tile::new('E')],
                vec![Tile::new('Z'), Tile::new('A')],
            ],
            bag: vec![],
            spread: 0,
            final_turn_owner: Some(Player::Us),
        };
        let value = playout.run(Player::Opp, 10);
        // The opponent played (ZA somewhere, > 0 points) and we replied; the
        // playout stopped there even though both racks may still hold tiles.
        assert!(playout.racks[1].len() <= 2);
        assert_eq!(value, playout.spread as f32);

        // Reproduce by hand: the opponent's best last turn is the 2-ply
        // optimum, and our final move is our best score on the board after it.
        let mut check = Playout {
            dict: &dict,
            board,
            racks: [
                vec![Tile::new('S'), Tile::new('E')],
                vec![Tile::new('Z'), Tile::new('A')],
            ],
            bag: vec![],
            spread: 0,
            final_turn_owner: Some(Player::Us),
        };
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
        });
        let results = evaluator.evaluate(&state, &suggestions, &dict);
        assert_eq!(results.len(), 4.min(suggestions.len()));
        for r in &results {
            assert!(r.mcts_equity.is_finite());
            assert!(r.iterations >= 2);
        }
        assert_eq!(results[0].iterations, 10);
    }

    /// Timing harness for the pre-endgame playout. Run with
    /// `cargo test --release bench_pre_endgame -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_pre_endgame() {
        use std::time::Instant;
        let dict = load_default_dictionary().unwrap();
        const MID: &str = "14U/5HELM3ZIN/4VIG7T/6AYES3BI/6DOWER2AL/9GOOFS1/1BRA4GOWN1I1/POETIC1QI2L1N1/OX3H1U3Y3/R3FINAlS5/T4V1K1P5/A4e1EME5/L6DON5/EtHER3AD5/D2RECOAT6";
        for rack in ["AEIST", "AEST", "ES", "E"] {
            let state = SolverState::from_cgp(&format!("{} {}", MID, rack)).unwrap();
            let bag = state.unseen_tile_count() - RACK_SIZE;
            let suggestions = state.get_suggestions(&dict);
            let config = MctsConfig::pre_endgame();
            let t = Instant::now();
            let results = MctsEvaluator::new(config.clone()).evaluate(&state, &suggestions, &dict);
            let elapsed = t.elapsed();
            println!(
                "rack {:5} bag {}  {} candidates x {} playouts: {:?}",
                rack,
                bag,
                results.len(),
                config.iterations_per_candidate,
                elapsed
            );
            for r in results.iter().take(5) {
                println!(
                    "   {:8} {:>4} score {:>3} static {:>6.1} sim {:>6.1} win {:.0}%",
                    r.mv.word,
                    r.mv.coords(),
                    r.mv.score,
                    r.mv.equity,
                    r.mcts_equity,
                    r.win_rate * 100.0
                );
            }
        }
    }
}
