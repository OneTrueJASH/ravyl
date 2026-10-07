//! Locating the data files (dictionary FST, RefEngine lexica) and telling a
//! real file from a Git LFS pointer stub.
//!
//! The `external/refengine/data/lexica/NWL23.*` files are stored in Git LFS.
//! A checkout without `git lfs pull` leaves ~130-byte text pointers in
//! their place; handing one of those to RefEngine or the leave parser fails
//! with an unhelpful error, so callers check here first.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Relative location of the dictionary FST under a data root.
pub const WORDS_FST: &str = "data/words.fst";
/// Relative location of RefEngine's data directory under a data root.
pub const REF_ENGINE_DATA: &str = "external/refengine/data";
/// Relative location of the leave table under a data root.
pub const LEAVES_KLV2: &str = "external/refengine/data/lexica/NWL23.klv2";
/// Relative location of RefEngine's lexicon under a data root.
pub const LEXICON_KWG: &str = "external/refengine/data/lexica/NWL23.kwg";

/// Hint shown whenever an LFS pointer stub is found instead of data.
pub const LFS_HINT: &str = "run `git lfs install && git lfs pull`";

const LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs";

/// True when `path` is a Git LFS pointer stub rather than the real file.
pub fn is_lfs_pointer(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    // Pointer files are tiny; anything larger is real data.
    if meta.len() > 1024 {
        return false;
    }
    let mut head = [0u8; LFS_POINTER_PREFIX.len()];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map(|_| head == LFS_POINTER_PREFIX)
        .unwrap_or(false)
}

/// State of a data file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Present,
    Missing,
    LfsPointer,
}

/// Classify the file at `path`.
pub fn file_status(path: &Path) -> FileStatus {
    if !path.is_file() {
        FileStatus::Missing
    } else if is_lfs_pointer(path) {
        FileStatus::LfsPointer
    } else {
        FileStatus::Present
    }
}

/// Directories searched for data files, in order: next to the executable,
/// one level up from it, and the working directory. Test builds search the
/// crate root first so tests do not depend on the working directory.
pub fn data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(test)]
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let exe_path = std::env::current_exe().ok();
    if let Some(dir) = exe_path.as_ref().and_then(|p| p.parent()) {
        roots.push(dir.to_path_buf());
        roots.push(dir.join(".."));
    }
    roots.push(PathBuf::from("."));
    roots
}

/// First data root holding `relative` as a real file (not an LFS stub).
pub fn find_data_file(relative: &str) -> Option<PathBuf> {
    data_roots()
        .into_iter()
        .map(|r| r.join(relative))
        .find(|p| file_status(p) == FileStatus::Present)
}

/// RefEngine's data directory, if one with a usable lexicon exists. On failure
/// the error says why (missing, or LFS pointer stubs).
pub fn refengine_data_dir() -> Result<PathBuf, String> {
    let mut saw_pointer = false;
    for root in data_roots() {
        let kwg = root.join(LEXICON_KWG);
        match file_status(&kwg) {
            FileStatus::Present => return Ok(root.join(REF_ENGINE_DATA)),
            FileStatus::LfsPointer => saw_pointer = true,
            FileStatus::Missing => {}
        }
    }
    if saw_pointer {
        Err(format!(
            "RefEngine lexicon files are Git LFS pointers; {}",
            LFS_HINT
        ))
    } else {
        Err(format!("RefEngine lexicon not found ({})", LEXICON_KWG))
    }
}

/// Helpers for tests that need the checked-in data files. Each returns
/// `None` (after printing why) when the files are missing or are LFS
/// pointer stubs, so the calling test can skip instead of failing.
#[cfg(test)]
#[allow(dead_code)] // shared by the lib and bin test builds; each uses a subset
pub(crate) mod test_data {
    use super::*;
    use crate::dictionary::Dictionary;
    use std::sync::{Mutex, MutexGuard};

    /// RefEngine's C library has global state that is not thread-safe, and an
    /// `App` may start RefEngine inference in the background. Tests that create
    /// a RefEngine handle or an `App` hold this lock while it lives.
    static REF_ENGINE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Take the RefEngine test lock, ignoring poisoning from a failed test.
    pub fn refengine_lock() -> MutexGuard<'static, ()> {
        REF_ENGINE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The crate root.
    pub fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    /// Path to a data file under the crate root, if it is real data.
    pub fn file(relative: &str) -> Option<PathBuf> {
        let path = root().join(relative);
        match file_status(&path) {
            FileStatus::Present => Some(path),
            FileStatus::Missing => {
                eprintln!("skipping: {} is missing", path.display());
                None
            }
            FileStatus::LfsPointer => {
                eprintln!(
                    "skipping: {} is a Git LFS pointer; {}",
                    path.display(),
                    LFS_HINT
                );
                None
            }
        }
    }

    /// The dictionary with the leave table attached when available.
    pub fn dictionary() -> Option<Dictionary> {
        file(WORDS_FST)?;
        match crate::dictionary::load_dictionary_from(root()) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("skipping: dictionary failed to load: {e:#}");
                None
            }
        }
    }

    /// RefEngine's data directory as a string, when the lexicon is real data.
    pub fn refengine_data() -> Option<String> {
        file(LEXICON_KWG)?;
        file(LEAVES_KLV2)?;
        Some(root().join(REF_ENGINE_DATA).to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lfs_pointers() {
        let dir = std::env::temp_dir().join(format!("cpsolver2-lfs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("stub.kwg");
        std::fs::write(
            &stub,
            "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 4719596\n",
        )
        .unwrap();
        let real = dir.join("real.kwg");
        std::fs::write(&real, [0u8, 1, 2, 3, 4, 5, 6, 7]).unwrap();

        assert!(is_lfs_pointer(&stub));
        assert_eq!(file_status(&stub), FileStatus::LfsPointer);
        assert!(!is_lfs_pointer(&real));
        assert_eq!(file_status(&real), FileStatus::Present);
        assert_eq!(file_status(&dir.join("absent")), FileStatus::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
