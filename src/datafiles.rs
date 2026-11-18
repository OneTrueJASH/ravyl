//! Locating the data files (the dictionary FST and the trained leave
//! table) and telling a real file from a Git LFS pointer stub (older
//! checkouts kept the leave table in LFS).

use std::io::Read;
use std::path::{Path, PathBuf};

/// Relative location of the dictionary FST under a data root.
pub const WORDS_FST: &str = "data/words.fst";
/// Relative location of the leave table under a data root.
pub const LEAVES: &str = "data/leaves.rlv";

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

/// Environment variable naming the data directory.
pub const DATA_DIR_ENV: &str = "RAVYL_DATA_DIR";

/// Its name before the rename to RAVYL, still honoured.
pub const LEGACY_DATA_DIR_ENV: &str = "CPSOLVER_DATA_DIR";

/// The name of the app's directories before the rename to RAVYL.
const LEGACY_NAME: &str = "cpsolver2";

/// Move the app's directories from their names before the rename to RAVYL
/// (`cpsolver2/` under the user config and data directories, holding the
/// settings and saved games) to `ravyl/`, once: only when the old one
/// exists and the new one does not. Returns what was moved.
pub fn migrate_legacy_dirs() -> Vec<(PathBuf, PathBuf)> {
    if cfg!(test) {
        return Vec::new();
    }
    let bases: Vec<PathBuf> = [dirs::config_dir(), dirs::data_dir()]
        .into_iter()
        .flatten()
        .collect();
    migrate_in(&bases)
}

/// [`migrate_legacy_dirs`] under these base directories.
fn migrate_in(bases: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let mut moved = Vec::new();
    for base in bases {
        let (old, new) = (base.join(LEGACY_NAME), base.join("ravyl"));
        if old.is_dir() && !new.exists() && !in_use(&old) && std::fs::rename(&old, &new).is_ok() {
            moved.push((old, new));
        }
    }
    moved
}

/// True while a running window of the old app holds a game in `dir`
/// open (a live process's `games/<slug>.lock`): moving the directory
/// then would pull it out from under that window.
fn in_use(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir.join("games")) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        path.extension().is_some_and(|x| x == "lock")
            && std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| t.trim().parse::<u32>().ok())
                .is_some_and(crate::games::pid_alive)
    })
}

/// The `--data-dir` command-line choice, set once at startup.
static CLI_DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Record the `--data-dir` argument (only the first call has an effect).
pub fn set_cli_data_dir(dir: PathBuf) {
    let _ = CLI_DATA_DIR.set(dir);
}

/// Directories searched for data files, in order:
///
/// 1. `--data-dir`
/// 2. `$RAVYL_DATA_DIR`
/// 3. the user data directory (`~/.local/share/ravyl`,
///    `~/Library/Application Support/ravyl`)
/// 4. `~/.ravyn`, where `ravyn fetch-data` installs them
/// 5. `<exe>/../share/ravyl` (an installed layout)
/// 6. the working directory (running from a checkout, for development)
///
/// Each holds `data/words.fst` and `data/leaves.rlv`. Test builds
/// search the crate root first so tests do not depend on the working
/// directory.
pub fn data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(test)]
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    roots.extend(ordered_roots(
        std::env::var_os(DATA_DIR_ENV)
            .or_else(|| std::env::var_os(LEGACY_DATA_DIR_ENV))
            .map(PathBuf::from),
        CLI_DATA_DIR.get().cloned(),
        dirs::data_dir(),
        dirs::home_dir(),
        std::env::current_exe().ok(),
    ));
    roots
}

/// [`data_roots`] from its inputs (the environment variable, the option,
/// the user data directory, the home directory and the executable's path).
fn ordered_roots(
    env: Option<PathBuf>,
    cli: Option<PathBuf>,
    user_data: Option<PathBuf>,
    home: Option<PathBuf>,
    exe: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(cli);
    roots.extend(env.filter(|p| !p.as_os_str().is_empty()));
    roots.extend(user_data.map(|d| d.join("ravyl")));
    roots.extend(home.map(|h| h.join(".ravyn")));
    roots.extend(
        exe.as_deref()
            .and_then(Path::parent)
            .map(|d| d.join("..").join("share").join("ravyl")),
    );
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

/// A user-facing warning when the leave table is a Git LFS pointer stub
/// (so leave values fall back to the heuristic), or `None` when the real
/// file is present or nothing is there at all.
pub fn lfs_warning() -> Option<String> {
    let stubbed = [LEAVES].iter().any(|rel| {
        find_data_file(rel).is_none()
            && data_roots()
                .iter()
                .any(|root| is_lfs_pointer(&root.join(rel)))
    });
    stubbed.then(|| {
        format!(
            "The leave table is a Git LFS pointer (heuristic leave values): {}",
            LFS_HINT
        )
    })
}

/// Helpers for tests that need the checked-in data files. Each returns
/// `None` (after printing why) when the files are missing or are LFS
/// pointer stubs, so the calling test can skip instead of failing.
#[cfg(test)]
pub(crate) mod test_data {
    use super::*;
    use crate::dictionary::Dictionary;
    use std::sync::{Mutex, MutexGuard};

    /// App tests that start background evaluations (simulations with a
    /// time budget, inference, exact solvers) hold this lock, so they run
    /// one at a time: run together, they starve each other's time budgets.
    static APP_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Take the app test lock, ignoring poisoning from a failed test.
    pub fn app_lock() -> MutexGuard<'static, ()> {
        APP_TEST_LOCK
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_dirs_move_once() {
        let base = std::env::temp_dir().join(format!("ravyl-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("cpsolver2/games")).unwrap();
        std::fs::write(base.join("cpsolver2/games/g.toml"), "x").unwrap();
        let moved = migrate_in(std::slice::from_ref(&base));
        assert_eq!(moved.len(), 1);
        assert!(base.join("ravyl/games/g.toml").is_file());
        assert!(!base.join("cpsolver2").exists());
        // Never over an existing ravyl/ directory.
        std::fs::create_dir_all(base.join("cpsolver2")).unwrap();
        assert!(migrate_in(std::slice::from_ref(&base)).is_empty());
        assert!(base.join("cpsolver2").is_dir());
        // Not while a live process holds a game open there (this test's
        // own pid stands in for an old window).
        std::fs::remove_dir_all(base.join("ravyl")).unwrap();
        std::fs::create_dir_all(base.join("cpsolver2/games")).unwrap();
        std::fs::write(
            base.join("cpsolver2/games/g.lock"),
            std::process::id().to_string(),
        )
        .unwrap();
        assert!(migrate_in(std::slice::from_ref(&base)).is_empty());
        // A stale lock (no such process) does not hold it back.
        std::fs::write(base.join("cpsolver2/games/g.lock"), "999999999").unwrap();
        assert_eq!(migrate_in(std::slice::from_ref(&base)).len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn data_roots_are_searched_in_order() {
        let roots = ordered_roots(
            Some("/env".into()),
            Some("/cli".into()),
            Some("/home/u/.local/share".into()),
            Some("/home/u".into()),
            Some("/opt/cp/bin/ravyl".into()),
        );
        let expect: Vec<PathBuf> = vec![
            "/cli".into(),
            "/env".into(),
            "/home/u/.local/share/ravyl".into(),
            "/home/u/.ravyn".into(),
            "/opt/cp/bin/../share/ravyl".into(),
            ".".into(),
        ];
        assert_eq!(roots, expect);
        // Unset or empty inputs are skipped.
        let roots = ordered_roots(Some("".into()), None, None, None, None);
        assert_eq!(roots, vec![PathBuf::from(".")]);
    }

    #[test]
    fn detects_lfs_pointers() {
        let dir = std::env::temp_dir().join(format!("ravyl-lfs-{}", std::process::id()));
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
