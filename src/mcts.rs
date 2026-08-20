//! Monte Carlo Tree Search (MCTS) move evaluation
//!
//! Implements a 2-ply lookahead algorithm to evaluate candidate moves by simulating
//! random opponent responses and follow-up plays.

use std::collections::HashMap;

use fastrand::Rng;
use rayon::prelude::*;

use crate::dictionary::Dictionary;
use crate::game::SolverState;
use crate::solver::{leave_value_with_phase, Move, Solver};
use crate::tile::{Rack, Tile, RACK_SIZE};

/// Configuration for MCTS evaluation
#[derive(Debug, Clone)]
pub struct MctsConfig {
    /// Number of top candidates to evaluate (default: 10)
    pub num_candidates: usize,
    /// Simulation iterations per candidate (default: 50)
    pub iterations_per_candidate: usize,
    /// Use parallel evaluation with Rayon (default: true)
    pub parallel: bool,
}

impl Default for MctsConfig {
    fn default() -> Self {
        MctsConfig {
            num_candidates: 20,
            iterations_per_candidate: 200,
            parallel: true,
        }
    }
}

/// Result of MCTS evaluation for a single move
#[derive(Debug, Clone)]
pub struct MctsResult {
    /// The evaluated move
    pub mv: Move,
    /// MCTS equity after simulation
    pub mcts_equity: f32,
}

/// Pool of available tiles for random drawing
pub struct TilePool {
    tiles: Vec<Tile>,
}

impl TilePool {
    /// Create a tile pool from remaining tile counts
    pub fn from_remaining(remaining: &HashMap<char, u32>) -> Self {
        let mut tiles = Vec::with_capacity(100); // Pre-allocate for typical bag size
        for (&letter, &count) in remaining {
            for _ in 0..count {
                if letter == '?' {
                    tiles.push(Tile::blank(' '));
                } else {
                    tiles.push(Tile::new(letter));
                }
            }
        }
        TilePool { tiles }
    }

    /// Draw n random tiles from the pool using Fisher-Yates partial shuffle
    /// This avoids allocating a separate indices vector
    pub fn draw_random_rack(&self, n: usize, rng: &mut Rng) -> Rack {
        let mut rack = Rack::new();
        let len = self.tiles.len();
        if len == 0 || n == 0 {
            return rack;
        }

        let draw_count = n.min(len);

        // Use a local copy for shuffling (only the indices we need)
        // Fisher-Yates partial shuffle - only shuffle first `draw_count` elements
        let mut indices: Vec<usize> = (0..len).collect();
        for i in 0..draw_count {
            let j = rng.usize(i..len);
            indices.swap(i, j);
        }

        for &idx in &indices[..draw_count] {
            rack.add(self.tiles[idx]);
        }
        rack
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

    /// Remove specific tiles from the pool, returning a new pool
    /// Uses swap_remove for O(1) removal instead of O(n)
    pub fn without_tiles(&self, tiles_to_remove: &[Tile]) -> Self {
        let mut remaining = self.tiles.clone();
        for tile in tiles_to_remove {
            // Find and remove matching tile using swap_remove (O(1) instead of O(n))
            if let Some(idx) = remaining.iter().position(|t| {
                if tile.is_blank {
                    t.is_blank
                } else {
                    !t.is_blank && t.letter == tile.letter
                }
            }) {
                remaining.swap_remove(idx);
            }
        }
        TilePool { tiles: remaining }
    }
}

/// Single simulation iteration result
struct SimulationResult {
    spread: f32,
}

/// MCTS evaluator for move selection
pub struct MctsEvaluator {
    config: MctsConfig,
}

impl MctsEvaluator {
    pub fn new(config: MctsConfig) -> Self {
        MctsEvaluator { config }
    }

    /// Evaluate candidates and return MCTS results sorted by MCTS equity
    pub fn evaluate(
        &self,
        state: &SolverState,
        candidates: &[Move],
        dictionary: &Dictionary,
    ) -> Vec<MctsResult> {
        let num_to_evaluate = candidates.len().min(self.config.num_candidates);
        let candidates_to_evaluate = &candidates[..num_to_evaluate];

        let results: Vec<MctsResult> = if self.config.parallel {
            candidates_to_evaluate
                .par_iter()
                .map(|mv| self.evaluate_candidate(state, mv, dictionary))
                .collect()
        } else {
            candidates_to_evaluate
                .iter()
                .map(|mv| self.evaluate_candidate(state, mv, dictionary))
                .collect()
        };

        // Sort by MCTS equity descending
        let mut sorted = results;
        sorted.sort_by(|a, b| {
            b.mcts_equity
                .partial_cmp(&a.mcts_equity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted
    }

    /// Evaluate a single candidate move
    fn evaluate_candidate(
        &self,
        state: &SolverState,
        mv: &Move,
        dictionary: &Dictionary,
    ) -> MctsResult {
        let mut rng = Rng::new(); // fastrand RNG - much faster than rand

        // Pre-compute state after our move (done once per candidate)
        let mut base_state = state.clone();
        base_state.apply_move(mv);

        // Pre-compute the tile pool from unseen tiles (bag + opponent's rack)
        let unseen = base_state.unseen_tiles();
        let unseen_count = unseen.values().sum::<u32>() as usize;
        let base_pool = TilePool::from_remaining(&unseen);

        // Early exit for endgame
        if base_pool.is_empty() {
            let spread = leave_value_with_phase(&mv.leave, Some(unseen_count));
            return MctsResult {
                mv: mv.clone(),
                mcts_equity: mv.score as f32 + spread,
            };
        }

        let mut total_spread = 0.0f32;
        let mut iterations_completed = 0;

        for _ in 0..self.config.iterations_per_candidate {
            if let Some(result) = self.simulate_iteration(
                &base_state,
                mv,
                &base_pool,
                unseen_count,
                dictionary,
                &mut rng,
            ) {
                total_spread += result.spread;
                iterations_completed += 1;
            }
        }

        let avg_spread = if iterations_completed > 0 {
            total_spread / iterations_completed as f32
        } else {
            0.0
        };

        // MCTS equity = Points + Spread_Difference + Leftover_Difference
        let mcts_equity = mv.score as f32 + avg_spread;

        MctsResult {
            mv: mv.clone(),
            mcts_equity,
        }
    }

    /// Run a single simulation iteration
    /// Takes pre-computed base_state (after our move) and base_pool
    fn simulate_iteration(
        &self,
        base_state: &SolverState,
        mv: &Move,
        base_pool: &TilePool,
        unseen_count: usize,
        dictionary: &Dictionary,
        rng: &mut Rng,
    ) -> Option<SimulationResult> {
        // 1. Draw random opponent rack (RACK_SIZE tiles or what's available)
        let opp_rack_size = RACK_SIZE.min(base_pool.len());
        let opp_rack = base_pool.draw_random_rack(opp_rack_size, rng);

        // 2. Find opponent's best response (greedy by equity)
        let opp_solver = Solver::new(&base_state.board, dictionary);
        let opp_moves = opp_solver.find_moves(&opp_rack);

        // 3. Get opponent's best move and update state
        let mut sim_state = base_state.clone();
        let (opp_score, opp_leave_equity) = if let Some(best_opp) = opp_moves.first() {
            sim_state.apply_move(best_opp);
            (
                best_opp.score,
                leave_value_with_phase(&best_opp.leave, Some(unseen_count)),
            )
        } else {
            (0, 0.0)
        };

        // 4. Remove opponent's tiles from pool, draw our follow-up tiles
        let pool_after_opp = base_pool.without_tiles(&opp_rack.tiles());
        let our_leave = &mv.leave;
        let tiles_needed = RACK_SIZE - our_leave.len();
        let draw_count = tiles_needed.min(pool_after_opp.len());

        // Build our follow-up rack from leave + new draws
        let mut followup_rack = Rack::new();
        for tile in our_leave {
            followup_rack.add(*tile);
        }
        if draw_count > 0 {
            let new_tiles = pool_after_opp.draw_random_rack(draw_count, rng);
            for tile in new_tiles.tiles() {
                followup_rack.add(tile);
            }
        }

        // 5. Find our best follow-up move
        let followup_solver = Solver::new(&sim_state.board, dictionary);
        let followup_moves = followup_solver.find_moves(&followup_rack);

        let (followup_score, our_leave_equity) = if let Some(best_followup) = followup_moves.first()
        {
            (
                best_followup.score,
                leave_value_with_phase(&best_followup.leave, Some(unseen_count)),
            )
        } else {
            (
                0,
                leave_value_with_phase(&followup_rack.tiles(), Some(unseen_count)),
            )
        };

        // 6. Calculate spread and leave differences
        let spread_diff = mv.score as f32 - opp_score as f32 + followup_score as f32;
        let leave_diff = our_leave_equity - opp_leave_equity;
        let total_spread = spread_diff + leave_diff;

        Some(SimulationResult {
            spread: total_spread - mv.score as f32, // Remove initial score (added back in mcts_equity)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::load_default_dictionary;

    #[test]
    fn test_tile_pool_creation() {
        let mut remaining = HashMap::new();
        remaining.insert('A', 3);
        remaining.insert('B', 2);
        remaining.insert('?', 1);

        let pool = TilePool::from_remaining(&remaining);
        assert_eq!(pool.len(), 6);
    }

    #[test]
    fn test_tile_pool_draw() {
        let mut remaining = HashMap::new();
        remaining.insert('A', 5);
        remaining.insert('E', 5);

        let pool = TilePool::from_remaining(&remaining);
        let mut rng = Rng::new();
        let rack = pool.draw_random_rack(RACK_SIZE, &mut rng);

        assert_eq!(rack.len(), RACK_SIZE);
    }

    #[test]
    fn test_tile_pool_without_tiles() {
        let mut remaining = HashMap::new();
        remaining.insert('A', 3);
        remaining.insert('B', 2);

        let pool = TilePool::from_remaining(&remaining);
        let tiles_to_remove = vec![Tile::new('A'), Tile::new('B')];
        let new_pool = pool.without_tiles(&tiles_to_remove);

        assert_eq!(new_pool.len(), 3); // 3A + 2B - 1A - 1B = 3
    }

    #[test]
    fn test_mcts_evaluator_basic() {
        let dict = load_default_dictionary().unwrap();
        let state = SolverState::new();

        // Create a simple rack
        let mut test_state = state.clone();
        test_state.add_to_rack('C');
        test_state.add_to_rack('A');
        test_state.add_to_rack('T');
        test_state.add_to_rack('S');
        test_state.add_to_rack('E');
        test_state.add_to_rack('R');
        test_state.add_to_rack('N');

        // Get suggestions
        let suggestions = test_state.get_suggestions(&dict);
        if suggestions.is_empty() {
            return; // Skip test if no moves found
        }

        // Run MCTS with minimal iterations for testing
        let config = MctsConfig {
            num_candidates: 3,
            iterations_per_candidate: 5,
            parallel: false,
        };

        let evaluator = MctsEvaluator::new(config);
        let results = evaluator.evaluate(&test_state, &suggestions, &dict);

        assert!(!results.is_empty());
        assert!(results.len() <= 3);

        // Check that results have valid equity values
        for result in &results {
            assert!(result.mcts_equity.is_finite());
        }
    }
}
