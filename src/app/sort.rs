//! Sorting the suggestions list.

use super::*;

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

    pub(super) fn apply_sort(&mut self) {
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
                    let sim_map: std::collections::HashMap<_, _> = results
                        .iter()
                        .map(|r| {
                            let key = (r.word.to_uppercase(), r.row, r.col, r.direction);
                            (
                                key,
                                (
                                    Self::sim_tier(r),
                                    if by_win { r.win_pct } else { r.sim_equity },
                                ),
                            )
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
            SortMode::Playouts | SortMode::Bingo => {
                let by_n = self.sort_mode == SortMode::Playouts;
                let key_of: std::collections::HashMap<_, _> = self
                    .sim_results
                    .iter()
                    .flatten()
                    .map(|r| {
                        let key = (r.word.to_uppercase(), r.row, r.col, r.direction);
                        let v = if by_n {
                            r.iterations as f32
                        } else {
                            r.ply2_bingo_pct
                        };
                        (key, (Self::sim_tier(r), v))
                    })
                    .collect();
                self.suggestions.sort_by(|a, b| {
                    let ka = key_of
                        .get(&Self::move_key(a))
                        .copied()
                        .unwrap_or((0.0, 0.0));
                    let kb = key_of
                        .get(&Self::move_key(b))
                        .copied()
                        .unwrap_or((0.0, 0.0));
                    kb.partial_cmp(&ka)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(
                            b.equity
                                .partial_cmp(&a.equity)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        )
                });
            }
        }
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
