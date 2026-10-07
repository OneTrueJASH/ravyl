//! Saved games: one TOML file per game in the user data directory
//! (`cpsolver2/games/<slug>.toml`), a lock file per open game so two
//! windows never edit the same game, the game list for the picker and
//! `--list`, and the one-time import of the old single `session.cgp`.
//!
//! Everything here takes the games directory as an argument; only
//! [`default_dir`] knows where the user's real one is, and it returns
//! `None` under `cfg(test)` so tests never touch the user's files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Name given to the game imported from the old `session.cgp`.
pub const RESTORED_NAME: &str = "Restored session";

/// Prefix of the automatic name a game gets before the user names it.
pub const UNTITLED_PREFIX: &str = "Untitled ";

/// The opponent-strength measurement of a game (see
/// `App::opp_temperature`): cumulative equity their observed moves gave up
/// and how many moves were measured.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OppLosses {
    pub total: f32,
    pub moves: u32,
}

/// One saved game, as stored on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedGame {
    /// Display name (free text, usually the opponent's name).
    pub name: String,
    /// The position from `App::position_cgp` (board, rack, scores and the
    /// final-turn opcodes).
    pub cgp: String,
    /// When it was last saved, in Unix seconds.
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub opp_losses: OppLosses,
}

/// A saved game in a listing, with what the picker shows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct GameEntry {
    pub slug: String,
    pub name: String,
    pub updated: u64,
    /// (ours, theirs), when the CGP carries scores.
    pub scores: Option<(u32, u32)>,
    /// Tiles left in the bag, when the CGP parses.
    pub bag: Option<usize>,
    /// Open in another window (a live process holds its lock).
    pub locked: bool,
}

impl GameEntry {
    /// `337–205`, or `-` without scores.
    pub fn score_text(&self) -> String {
        match self.scores {
            Some((a, b)) => format!("{}\u{2013}{}", a, b),
            None => "-".to_string(),
        }
    }
}

/// Where the user's games live (`None` in tests, which must not touch the
/// user's files, or when there is no data directory).
pub fn default_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    dirs::data_dir().map(|d| d.join("cpsolver2").join("games"))
}

/// The file name stem for a game name: lowercase ASCII letters and digits
/// kept, every run of anything else turned into one `-`, no `-` at either
/// end; `game` if nothing is left.
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "game".to_string()
    } else {
        trimmed.to_string()
    }
}

/// True for the automatic `Untitled …` names.
pub fn is_automatic_name(name: &str) -> bool {
    name.starts_with(UNTITLED_PREFIX)
}

pub fn game_path(dir: &Path, slug: &str) -> PathBuf {
    dir.join(format!("{slug}.toml"))
}

pub fn lock_path(dir: &Path, slug: &str) -> PathBuf {
    dir.join(format!("{slug}.lock"))
}

/// The current time in Unix seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// True if a saved game with this slug exists.
pub fn exists(dir: &Path, slug: &str) -> bool {
    game_path(dir, slug).is_file()
}

/// Write a game atomically (temporary file in the same directory, then a
/// rename), so a crash or a closed terminal mid-write never leaves a torn
/// file.
pub fn save(dir: &Path, game: &SavedGame) -> io::Result<()> {
    // One writer at a time: the temporary file's name is per process, and
    // the signal watchdog may save while the event loop is stuck.
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    fs::create_dir_all(dir)?;
    let path = game_path(dir, &slug(&game.name));
    let text = toml::to_string_pretty(game).map_err(io::Error::other)?;
    let tmp = path.with_extension(format!("toml.tmp{}", std::process::id()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Read the game stored under `slug`.
pub fn load(dir: &Path, slug: &str) -> io::Result<SavedGame> {
    let text = fs::read_to_string(game_path(dir, slug))?;
    toml::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.message()))
}

/// Delete a saved game (and a stale lock left beside it).
pub fn delete(dir: &Path, slug: &str) -> io::Result<()> {
    let _ = fs::remove_file(lock_path(dir, slug));
    fs::remove_file(game_path(dir, slug))
}

/// Scores (`ours/theirs`, the third CGP field) and the bag size of a
/// saved position.
pub fn summary(cgp: &str) -> (Option<(u32, u32)>, Option<usize>) {
    let scores = cgp.split_whitespace().nth(2).and_then(|f| {
        let (a, b) = f.split_once('/')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    });
    let bag = crate::game::SolverState::from_cgp(cgp)
        .ok()
        .map(|s| s.bag_size());
    (scores, bag)
}

/// Every readable saved game in `dir`, most recently played first.
/// Unreadable files are skipped (and logged).
pub fn list(dir: &Path) -> Vec<GameEntry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut games: Vec<GameEntry> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "toml" {
                return None;
            }
            let slug = path.file_stem()?.to_str()?.to_string();
            match load(dir, &slug) {
                Ok(game) => {
                    let (scores, bag) = summary(&game.cgp);
                    Some(GameEntry {
                        locked: lock_holder(dir, &slug).is_some(),
                        slug,
                        name: game.name,
                        updated: game.updated,
                        scores,
                        bag,
                    })
                }
                Err(e) => {
                    crate::logging::log(&format!(
                        "Skipped unreadable game {}: {}",
                        path.display(),
                        e
                    ));
                    None
                }
            }
        })
        .collect();
    games.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.name.cmp(&b.name)));
    games
}

/// True if process `pid` is running (it may belong to another user).
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists and may be
    // signalled; nothing is sent.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Without a portable liveness check every lock counts as stale, so a
/// crashed window can never lock a game forever.
#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    false
}

/// The pid of the live process holding the game's lock, if any. A lock
/// whose process is gone (or which cannot be read) is stale and holds
/// nothing.
pub fn lock_holder(dir: &Path, slug: &str) -> Option<u32> {
    let text = fs::read_to_string(lock_path(dir, slug)).ok()?;
    let pid: u32 = text.trim().parse().ok()?;
    pid_alive(pid).then_some(pid)
}

/// Why a game could not be locked.
#[derive(Debug)]
pub enum LockError {
    /// Another live process holds it.
    Held(u32),
    Io(io::Error),
}

/// Take the lock for `slug` for this process: create `<slug>.lock` holding
/// our pid, taking over a stale one. Refused while a live process (this
/// one included: two windows of the same process are still two windows)
/// holds it.
pub fn lock(dir: &Path, slug: &str) -> Result<(), LockError> {
    fs::create_dir_all(dir).map_err(LockError::Io)?;
    let path = lock_path(dir, slug);
    for _ in 0..2 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                return writeln!(file, "{}", std::process::id()).map_err(LockError::Io);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if let Some(pid) = lock_holder(dir, slug) {
                    return Err(LockError::Held(pid));
                }
                // Stale: remove it and try once more.
                crate::logging::log(&format!("Took over the stale lock {}", path.display()));
                let _ = fs::remove_file(&path);
            }
            Err(e) => return Err(LockError::Io(e)),
        }
    }
    Err(LockError::Io(io::Error::other(
        "lock file keeps reappearing",
    )))
}

/// Release our lock on `slug` (only if it is ours).
pub fn unlock(dir: &Path, slug: &str) {
    let path = lock_path(dir, slug);
    let ours = fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        == Some(std::process::id());
    if ours {
        let _ = fs::remove_file(&path);
    }
}

/// `Untitled YYYY-MM-DD HH:MM` for the Unix time `secs`, in local time
/// where the platform can tell (UTC otherwise).
pub fn untitled_name(secs: u64) -> String {
    let (y, mo, d, h, mi) = local_civil(secs);
    format!("{UNTITLED_PREFIX}{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

#[cfg(unix)]
fn local_civil(secs: u64) -> (i64, u32, u32, u32, u32) {
    let t = secs as libc::time_t;
    // SAFETY: `tm` is plain data, and localtime_r only writes into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return utc_civil(secs);
    }
    (
        tm.tm_year as i64 + 1900,
        tm.tm_mon as u32 + 1,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
    )
}

#[cfg(not(unix))]
fn local_civil(secs: u64) -> (i64, u32, u32, u32, u32) {
    utc_civil(secs)
}

/// UTC date and time for Unix seconds (Howard Hinnant's days-to-civil).
fn utc_civil(secs: u64) -> (i64, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32)
}

/// A name based on `base` that no saved game or lock in `dir` uses yet:
/// `base`, then `base (2)`, `base (3)`, …
pub fn unused_name(dir: &Path, base: &str) -> String {
    let taken = |name: &str| {
        let s = slug(name);
        exists(dir, &s) || lock_path(dir, &s).exists()
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|n| !taken(n))
        .expect("some suffix is free")
}

/// "just now", "5m ago", "2h ago", "3d ago", "4mo ago" for a time `then`
/// as seen at `now` (both Unix seconds).
pub fn relative_time(then: u64, now: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".to_string(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ if s < 60 * 86_400 => format!("{}d ago", s / 86_400),
        _ => format!("{}mo ago", s / (30 * 86_400)),
    }
}

/// Import the old single-file session: when `<games dir>/..` holds
/// `session.cgp`, save it as the game [`RESTORED_NAME`] (`… (2)` and so on
/// if that name is taken) and rename the old file to
/// `session.cgp.imported`. Returns the imported game's slug.
///
/// Not only the first time: a window still running an older version
/// writes `session.cgp` when it exits, after the games directory exists,
/// and that game must not be lost either.
pub fn migrate_session(dir: &Path) -> Option<String> {
    let session = dir.parent()?.join("session.cgp");
    if !session.is_file() {
        return None;
    }
    let cgp = fs::read_to_string(&session).ok()?;
    let cgp = cgp.trim();
    if crate::game::SolverState::from_cgp(cgp).is_err() {
        crate::logging::log("Old session.cgp is unreadable; not imported");
        return None;
    }
    let updated = fs::metadata(&session)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or_else(now_secs, |d| d.as_secs());
    let game = SavedGame {
        name: unused_name(dir, RESTORED_NAME),
        cgp: cgp.to_string(),
        updated,
        opp_losses: OppLosses::default(),
    };
    if let Err(e) = save(dir, &game) {
        crate::logging::log(&format!("Importing session.cgp failed: {}", e));
        return None;
    }
    let _ = fs::rename(&session, session.with_extension("cgp.imported"));
    crate::logging::log(&format!("Imported session.cgp as \"{}\"", game.name));
    Some(slug(&game.name))
}

/// A fresh, empty directory for a test (under the system temp dir, never
/// the user's data directory).
#[cfg(test)]
pub(crate) fn test_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cpsolver2-games-{}-{}-{}",
        tag,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pid that no longer runs: a child that has exited and been reaped.
#[cfg(test)]
pub(crate) fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_rules() {
        assert_eq!(slug("Alice"), "alice");
        assert_eq!(slug("  Bob  Smith!! "), "bob-smith");
        assert_eq!(
            slug("Untitled 2026-10-07 14:03"),
            "untitled-2026-10-07-14-03"
        );
        assert_eq!(slug("--x__y--"), "x-y");
        assert_eq!(slug("José"), "jos");
        assert_eq!(slug("R2-D2"), "r2-d2");
        assert_eq!(slug(""), "game");
        assert_eq!(slug("!!!"), "game");
        assert_eq!(slug("ÄÖÜ"), "game");
    }

    #[test]
    fn save_load_round_trip() {
        let dir = test_dir("roundtrip");
        let game = SavedGame {
            name: "Alice Smith".to_string(),
            cgp: "15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 AEINRST/ 120/95 0 \
                  cpsolver.final us; cpsolver.final_known 1;"
                .to_string(),
            updated: 1_700_000_000,
            opp_losses: OppLosses {
                total: 31.5,
                moves: 3,
            },
        };
        save(&dir, &game).unwrap();
        assert!(game_path(&dir, "alice-smith").is_file());
        assert_eq!(load(&dir, "alice-smith").unwrap(), game);
        // No temporary file is left behind.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let entries = list(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].scores, Some((120, 95)));
        assert_eq!(entries[0].score_text(), "120\u{2013}95");
        // 100 - 3 on the board - 7 in the rack - 7 for their rack.
        assert_eq!(entries[0].bag, Some(83));
        assert!(!entries[0].locked);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_is_newest_first_and_skips_junk() {
        let dir = test_dir("list");
        for (name, updated) in [("Old", 10), ("Newest", 30), ("Middle", 20)] {
            let game = SavedGame {
                name: name.to_string(),
                cgp: "15/15/15/15/15/15/15/15/15/15/15/15/15/15/15 / 0/0 0".to_string(),
                updated,
                opp_losses: OppLosses::default(),
            };
            save(&dir, &game).unwrap();
        }
        fs::write(dir.join("broken.toml"), "name = [").unwrap();
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        let names: Vec<String> = list(&dir).into_iter().map(|g| g.name).collect();
        assert_eq!(names, ["Newest", "Middle", "Old"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn locks_refuse_live_holders_and_take_over_stale_ones() {
        let dir = test_dir("locks");
        lock(&dir, "alice").unwrap();
        assert_eq!(lock_holder(&dir, "alice"), Some(std::process::id()));
        assert!(matches!(lock(&dir, "alice"), Err(LockError::Held(_))));
        unlock(&dir, "alice");
        assert!(!lock_path(&dir, "alice").exists());

        // A lock left by a process that is gone is taken over.
        fs::write(lock_path(&dir, "bob"), format!("{}\n", dead_pid())).unwrap();
        assert_eq!(lock_holder(&dir, "bob"), None);
        lock(&dir, "bob").unwrap();
        assert_eq!(lock_holder(&dir, "bob"), Some(std::process::id()));

        // Someone else's lock is never removed by unlock.
        fs::write(lock_path(&dir, "carol"), "1\n").unwrap();
        unlock(&dir, "carol");
        assert!(lock_path(&dir, "carol").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn untitled_names_and_uniqueness() {
        let name = untitled_name(now_secs());
        assert!(is_automatic_name(&name));
        let rest = &name[UNTITLED_PREFIX.len()..];
        assert_eq!(rest.len(), "2026-10-07 14:03".len(), "{name}");
        assert_eq!(utc_civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(utc_civil(1_791_382_980), (2026, 10, 7, 14, 23));
        assert_eq!(utc_civil(951_782_400), (2000, 2, 29, 0, 0));

        let dir = test_dir("unique");
        assert_eq!(unused_name(&dir, "Bob"), "Bob");
        fs::write(game_path(&dir, "bob"), "").unwrap();
        fs::write(lock_path(&dir, "bob-2"), "").unwrap();
        assert_eq!(unused_name(&dir, "Bob"), "Bob (3)");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(100, 100), "just now");
        assert_eq!(relative_time(0, 5 * 60), "5m ago");
        assert_eq!(relative_time(0, 2 * 3600 + 10), "2h ago");
        assert_eq!(relative_time(0, 3 * 86_400), "3d ago");
        assert_eq!(relative_time(0, 90 * 86_400), "3mo ago");
        assert_eq!(relative_time(200, 100), "just now");
    }

    #[test]
    fn session_is_imported() {
        let base = test_dir("migrate");
        let dir = base.join("games");
        let cgp = "15/15/15/15/15/15/15/4CAT8/15/15/15/15/15/15/15 AEINRST/ 10/20 0 \
                   cpsolver.final opp; cpsolver.final_known 0;";
        fs::write(base.join("session.cgp"), format!("{cgp}\n")).unwrap();
        assert_eq!(migrate_session(&dir).as_deref(), Some("restored-session"));
        let game = load(&dir, "restored-session").unwrap();
        assert_eq!(game.name, RESTORED_NAME);
        assert_eq!(game.cgp, cgp);
        assert!(!base.join("session.cgp").exists());
        assert!(base.join("session.cgp.imported").exists());

        // Nothing to do without the file.
        assert_eq!(migrate_session(&dir), None);

        // One written later (by a window running an older version) is
        // imported too, under a free name.
        fs::write(base.join("session.cgp"), cgp).unwrap();
        assert_eq!(migrate_session(&dir).as_deref(), Some("restored-session-2"));
        assert_eq!(
            load(&dir, "restored-session-2").unwrap().name,
            "Restored session (2)"
        );
        assert!(!base.join("session.cgp").exists());

        // An unreadable one is left alone.
        fs::write(base.join("session.cgp"), "garbage").unwrap();
        assert_eq!(migrate_session(&dir), None);
        assert!(base.join("session.cgp").exists());
        let _ = fs::remove_dir_all(&base);
    }
}
