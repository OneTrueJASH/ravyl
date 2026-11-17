//! Sorting the suggestions list.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::board::Direction;
use crate::solver::Move;

use super::state::{App, MessageKind, SimResult, SimStatus, SortMode};

impl App {
    /// Identity of a move in the list (its word and where it goes).
    pub(super) fn move_key(mv: &Move) -> (String, usize, usize, Direction) {
        (mv.word.to_uppercase(), mv.row, mv.col, mv.direction)
    }

    /// Advance to the next sort mode, re-sort suggestions, and persist.
    pub fn cycle_sort_mode(&mut self) {
        let has_sim = self.sim_results.is_some();
        self.sort_mode = self.sort_mode.next(has_sim);
        self.apply_sort();
        self.selected_suggestion = 0;
        self.set_message(
            MessageKind::Info,
            format!("Sort: {}", self.sort_mode.label()),
        );
        self.save_settings();
    }

    /// Set a specific sort mode and re-sort suggestions.
    pub fn set_sort_mode(&mut self, mode: SortMode) {
        self.sort_mode = mode;
        self.apply_sort();
        self.selected_suggestion = 0;
    }

    /// For each suggestion, the index of its evaluation result, if any.
    /// One pass through a map keyed by borrowed strings: no allocation per
    /// move, however long the list.
    fn result_index(&self) -> Vec<Option<usize>> {
        let Some(results) = &self.sim_results else {
            return vec![None; self.suggestions.len()];
        };
        let by_key: HashMap<(&str, usize, usize, Direction), usize> = results
            .iter()
            .enumerate()
            .map(|(j, r)| ((r.word.as_str(), r.row, r.col, r.direction), j))
            .collect();
        self.suggestions
            .iter()
            .map(|m| {
                by_key
                    .get(&(m.word.as_str(), m.row, m.col, m.direction))
                    .copied()
            })
            .collect()
    }

    /// Re-sort the suggestions by the current mode and rebuild
    /// [`App::sim_index`]. Each move is paired with its result once, so
    /// the comparisons only compare numbers (with tens of thousands of
    /// moves, looking results up inside the comparator cost about 30 ms
    /// per live snapshot).
    pub(super) fn apply_sort(&mut self) {
        let index = self.result_index();
        let results: &[SimResult] = self.sim_results.as_deref().unwrap_or(&[]);
        let has_sim = self.sim_results.is_some();
        let mut items: Vec<(Move, Option<usize>)> = std::mem::take(&mut self.suggestions)
            .into_iter()
            .zip(index)
            .collect();
        let by_equity = |a: &(Move, Option<usize>), b: &(Move, Option<usize>)| {
            b.0.equity
                .partial_cmp(&a.0.equity)
                .unwrap_or(Ordering::Equal)
        };
        match self.sort_mode {
            SortMode::Equity => items.sort_by(by_equity),
            SortMode::MctsEquity | SortMode::WinPct if has_sim => {
                // Candidates pruned early carry a short-run estimate;
                // rank them after those that received the full budget,
                // and any that never got a playout after both.
                let by_win = self.sort_mode == SortMode::WinPct;
                let key = |j: usize| {
                    let r = &results[j];
                    (
                        Self::sim_tier(r),
                        if by_win { r.win_pct } else { r.sim_equity },
                    )
                };
                items.sort_by(|a, b| match (a.1, b.1) {
                    (Some(ja), Some(jb)) => {
                        key(jb).partial_cmp(&key(ja)).unwrap_or(Ordering::Equal)
                    }
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => by_equity(a, b),
                });
            }
            // No simulation results: fall back to equity.
            SortMode::MctsEquity | SortMode::WinPct => items.sort_by(by_equity),
            SortMode::Score => items.sort_by_key(|(m, _)| std::cmp::Reverse(m.score)),
            SortMode::Leave => items.sort_by(|a, b| {
                let leave = |m: &Move| m.equity - m.score as f32;
                leave(&b.0)
                    .partial_cmp(&leave(&a.0))
                    .unwrap_or(Ordering::Equal)
            }),
            SortMode::WordLength => items.sort_by(|a, b| {
                b.0.word
                    .len()
                    .cmp(&a.0.word.len())
                    .then_with(|| by_equity(a, b))
            }),
            SortMode::Alpha => items.sort_by(|a, b| a.0.word.cmp(&b.0.word)),
            SortMode::Playouts | SortMode::Bingo => {
                let by_n = self.sort_mode == SortMode::Playouts;
                let key = |j: Option<usize>| {
                    j.map_or((0.0, 0.0), |j| {
                        let r = &results[j];
                        let v = if by_n {
                            r.iterations as f32
                        } else {
                            r.ply2_bingo_pct
                        };
                        (Self::sim_tier(r), v)
                    })
                };
                items.sort_by(|a, b| {
                    key(b.1)
                        .partial_cmp(&key(a.1))
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| by_equity(a, b))
                });
            }
        }
        (self.suggestions, self.sim_index) = items.into_iter().unzip();
    }

    /// Sort tier of an evaluated candidate: exact or with its playout
    /// floor first, then those still filling up, then unsimulated ones
    /// (whose numbers are placeholders).
    pub(super) fn sim_tier(r: &SimResult) -> f32 {
        if r.is_endgame || (r.iterations > 0 && r.status != SimStatus::Pending) {
            2.0
        } else if r.iterations > 0 {
            1.0
        } else {
            0.0
        }
    }
}
