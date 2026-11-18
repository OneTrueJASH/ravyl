//! The engine's dictionary ([`ravyn::dictionary`]), plus loading from the
//! app's data roots: the default root search, Git LFS pointer detection,
//! and falling back to heuristic leaves when the leave table is unusable.

use std::path::Path;

use anyhow::{Context, Result};

pub use ravyn::dictionary::*;
use ravyn::leaves::LeaveValues;

use crate::datafiles;

/// Load the dictionary and leave table from one data root: `data/words.fst`
/// plus the trained leave table `data/leaves.rlv` when it is real data. A
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
    let leaves_path = root.join(datafiles::LEAVES);
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
/// that has it, attaching the leave table (`data/leaves.rlv`) when present.
pub fn load_default_dictionary() -> Result<Dictionary> {
    let fst_path = datafiles::find_data_file(datafiles::WORDS_FST).context(
        "Could not find the dictionary (data/words.fst). Run `ravyn fetch-data` \
         (installs it in ~/.ravyn), point --data-dir or CPSOLVER_DATA_DIR at a \
         directory holding data/, install them under the user data directory \
         (cpsolver2/), or run from the repository root.",
    )?;
    let root = fst_path
        .parent()
        .and_then(Path::parent)
        .context("dictionary path has no data root")?;
    load_dictionary_from(root)
}
