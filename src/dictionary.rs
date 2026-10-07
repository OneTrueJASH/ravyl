use std::path::Path;

use anyhow::{Context, Result};
use fst::raw::Node;
use fst::Set;

use crate::datafiles;
use crate::leaves::{bingo_bonus_correction, LeaveValues};
use crate::solver::{leave_value_with_phase, BINGO_BONUS};
use crate::tile::Tile;

/// Dictionary loaded from a pre-compiled FST for fast lookups
pub struct Dictionary {
    /// FST set for O(1) word validation and prefix matching
    fst: Set<Vec<u8>>,
    word_count: usize,
    /// Leave values from RefEngine's table, when the file was found.
    leaves: Option<LeaveValues>,
}

impl Dictionary {
    /// Load dictionary from a pre-compiled FST file
    pub fn load_from_fst<P: AsRef<Path>>(path: P) -> Result<Self> {
        let fst_bytes = std::fs::read(path.as_ref()).context("Failed to read FST file")?;

        // Count words by iterating once
        let fst = Set::new(fst_bytes).context("Failed to parse FST data")?;

        use fst::Streamer;
        let mut count = 0;
        let mut stream = fst.stream();
        while stream.next().is_some() {
            count += 1;
        }

        Ok(Dictionary {
            fst,
            word_count: count,
            leaves: None,
        })
    }

    /// Attach a leave-value table; static equity uses it instead of the
    /// per-letter heuristic.
    pub fn with_leaves(mut self, leaves: LeaveValues) -> Self {
        self.leaves = Some(leaves);
        self
    }

    /// True when leave values come from a fitted table rather than the
    /// per-letter heuristic.
    #[cfg_attr(not(test), allow(dead_code))] // used by tests and library consumers
    pub fn has_leave_table(&self) -> bool {
        self.leaves.is_some()
    }

    /// Strategic value of keeping `leave` on the rack, scaled by game phase
    /// (`unseen` = bag + opponent rack; `None` means full weight).
    ///
    /// Uses the fitted leave table when loaded and the leave is in it (the
    /// table covers 1–6 tiles), corrected from Scrabble's 50-point bingo
    /// bonus to Crossplay's 40 (see [`bingo_bonus_correction`]); otherwise
    /// the per-letter heuristic.
    ///
    /// A full 7-tile rack (the racks at a playout cut-off) is valued on the
    /// same scale as the mean of its seven 6-tile sub-leaves, each looked up
    /// in the table, so the cut-off valuation and static equity agree.
    pub fn leave_value(&self, leave: &[Tile], unseen: Option<usize>) -> f32 {
        if leave.is_empty() {
            return 0.0;
        }
        if let Some(table) = &self.leaves {
            if let Some(v) = table.value(leave) {
                return (v + bingo_bonus_correction(leave, BINGO_BONUS)) * phase_scale(unseen);
            }
            if leave.len() == crate::tile::RACK_SIZE {
                let mut total = 0.0;
                let mut found = 0;
                for skip in 0..leave.len() {
                    let sub: Vec<Tile> = leave
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != skip)
                        .map(|(_, t)| *t)
                        .collect();
                    if let Some(v) = table.value(&sub) {
                        total += v + bingo_bonus_correction(&sub, BINGO_BONUS);
                        found += 1;
                    }
                }
                if found == leave.len() {
                    return total / found as f32 * phase_scale(unseen);
                }
            }
        }
        leave_value_with_phase(leave, unseen)
    }

    /// Check if a word is valid
    #[cfg_attr(not(test), allow(dead_code))] // used by tests and library consumers
    pub fn is_valid(&self, word: &str) -> bool {
        let upper = word.to_ascii_uppercase();
        self.fst.contains(upper.as_bytes())
    }

    /// Check if a prefix could lead to a valid word
    #[cfg_attr(not(test), allow(dead_code))] // used by tests and library consumers
    pub fn has_prefix(&self, prefix: &str) -> bool {
        if prefix.is_empty() {
            return true;
        }

        let upper = prefix.to_ascii_uppercase();
        let bytes = upper.as_bytes();

        // Use FST range query to check for prefix
        use fst::{IntoStreamer, Streamer};
        let mut stream = self.fst.range().ge(bytes).into_stream();

        if let Some(word) = stream.next() {
            word.starts_with(bytes)
        } else {
            false
        }
    }

    /// Root node of the underlying FST, for incremental prefix walking.
    ///
    /// The move generator descends one letter at a time with [`Self::child`]
    /// instead of re-validating whole prefixes, so each step is a single
    /// transition lookup rather than a range query.
    #[inline]
    pub fn root(&self) -> Node<'_> {
        self.fst.as_fst().root()
    }

    /// Follow the transition for `byte` (an uppercase ASCII letter) out of
    /// `node`, returning the child node if the extended prefix exists.
    #[inline]
    pub fn child(&self, node: Node<'_>, byte: u8) -> Option<Node<'_>> {
        node.find_input(byte)
            .map(|i| self.fst.as_fst().node(node.transition(i).addr))
    }

    /// Node reached by following a transition's address (used when iterating
    /// a node's transitions directly).
    #[inline]
    pub fn node_at(&self, addr: fst::raw::CompiledAddr) -> Node<'_> {
        self.fst.as_fst().node(addr)
    }

    /// Get the total number of words in the dictionary
    pub fn len(&self) -> usize {
        self.word_count
    }

    /// Returns true if the dictionary contains no words
    #[allow(dead_code)] // Required by public API (clippy::len_without_is_empty)
    pub fn is_empty(&self) -> bool {
        self.word_count == 0
    }
}

/// Leave weight by game phase: leaves matter less as redraws run out.
pub fn phase_scale(unseen: Option<usize>) -> f32 {
    match unseen {
        Some(n) if n <= 7 => 0.25, // Endgame: no redraws; the 2-ply solver decides
        Some(n) if n <= 14 => 0.6, // Pre-endgame: one draw left at most
        _ => 1.0,
    }
}

/// Load the dictionary and leave table from one data root: `data/words.fst`
/// plus `external/refengine/data/lexica/NWL23.klv2` when it is real data. A
/// missing table or an LFS pointer stub falls back to heuristic leaves.
pub fn load_dictionary_from(root: &Path) -> Result<Dictionary> {
    let fst_path = root.join(datafiles::WORDS_FST);
    if datafiles::is_lfs_pointer(&fst_path) {
        anyhow::bail!(
            "{} is a Git LFS pointer; {}",
            fst_path.display(),
            datafiles::LFS_HINT
        );
    }
    let dict = Dictionary::load_from_fst(&fst_path)
        .with_context(|| format!("loading {}", fst_path.display()))?;
    let leaves_path = root.join(datafiles::LEAVES_KLV2);
    match datafiles::file_status(&leaves_path) {
        datafiles::FileStatus::Present => {}
        datafiles::FileStatus::LfsPointer => {
            crate::logging::log(&format!(
                "Leave table is a Git LFS pointer ({}); using heuristic leave values",
                datafiles::LFS_HINT
            ));
            return Ok(dict);
        }
        datafiles::FileStatus::Missing => return Ok(dict),
    }
    match LeaveValues::load(&leaves_path) {
        Ok(table) => Ok(dict.with_leaves(table)),
        Err(e) => {
            crate::logging::log(&format!(
                "Leave table unusable, using heuristic leave values: {e:#}"
            ));
            Ok(dict)
        }
    }
}

/// Load the default dictionary (`data/words.fst`) from the first data root
/// that has it, attaching RefEngine's leave table (`NWL23.klv2`) when present.
pub fn load_default_dictionary() -> Result<Dictionary> {
    let fst_path = datafiles::find_data_file(datafiles::WORDS_FST).context(
        "Could not find the dictionary (data/words.fst). Run cpsolver2 from \
         the repository root, or put the data files next to the binary.",
    )?;
    let root = fst_path
        .parent()
        .and_then(Path::parent)
        .context("dictionary path has no data root")?;
    load_dictionary_from(root)
}

#[cfg(test)]
mod tests {
    use crate::datafiles::{test_data, LEAVES_KLV2};

    #[test]
    fn test_load_dictionary() {
        let Some(dict) = test_data::dictionary() else {
            return;
        };
        assert!(dict.len() > 100000);
        // The leave table is attached whenever the real file is present
        // (an LFS pointer stub falls back to heuristic leaves).
        let klv_real = test_data::file(LEAVES_KLV2).is_some();
        assert_eq!(dict.has_leave_table(), klv_real);
    }

    #[test]
    fn test_word_validation() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };

        // Valid words
        assert!(dict.is_valid("WORD"));
        assert!(dict.is_valid("word"));
        assert!(dict.is_valid("QUARTZ"));
        assert!(dict.is_valid("AA")); // Valid 2-letter word

        // Invalid words
        assert!(!dict.is_valid("XYZZY"));
        assert!(!dict.is_valid("ASDFGH"));
    }

    #[test]
    fn test_prefix_matching() {
        let Some(dict) = crate::datafiles::test_data::dictionary() else {
            return;
        };

        assert!(dict.has_prefix("QU"));
        assert!(dict.has_prefix("QUAR"));
        assert!(dict.has_prefix(""));

        // QX is not a valid prefix
        assert!(!dict.has_prefix("QX"));
    }
}
