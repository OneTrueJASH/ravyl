//! Per-instance logging.
//!
//! Each process writes `cpsolver2-<pid>.log` under the user's state
//! directory (`~/.local/state/cpsolver2` on Linux; the cache directory,
//! `~/Library/Caches/cpsolver2`, on macOS where there is no state
//! directory). The file is created fresh with mode 0600 and never through a
//! symlink. Logs older than a week are pruned at startup, keeping at most
//! [`MAX_KEPT_LOGS`] files.
//!
//! The last [`RING_CAPACITY`] lines are also kept in memory for the log
//! viewer, so drawing a frame never touches the file system.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lines kept in memory for the log viewer.
pub const RING_CAPACITY: usize = 10_000;
/// Most log files kept in the log directory after pruning.
const MAX_KEPT_LOGS: usize = 20;
/// Log files older than this are pruned at startup.
const MAX_LOG_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const LOG_PREFIX: &str = "cpsolver2-";

struct LogState {
    file: Option<File>,
    path: Option<PathBuf>,
    lines: VecDeque<String>,
    /// Lines dropped from the front of `lines` so far.
    dropped: u64,
    /// Absolute index of the first line of the current section.
    section_start: u64,
}

impl LogState {
    fn new(file: Option<File>, path: Option<PathBuf>) -> Self {
        LogState {
            file,
            path,
            lines: VecDeque::new(),
            dropped: 0,
            section_start: 0,
        }
    }

    fn push(&mut self, line: String) {
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{}", line);
        }
        if self.lines.len() == RING_CAPACITY {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }

    fn start_section(&mut self) {
        self.section_start = self.dropped + self.lines.len() as u64;
    }

    fn section(&mut self) -> &[String] {
        let skip = self.section_start.saturating_sub(self.dropped) as usize;
        let lines = self.lines.make_contiguous();
        &lines[skip.min(lines.len())..]
    }
}

fn state() -> MutexGuard<'static, LogState> {
    static STATE: OnceLock<Mutex<LogState>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            let (file, path) = open_log_file();
            Mutex::new(LogState::new(file, path))
        })
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Directory for this program's log files.
pub fn log_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::cache_dir)
        .map(|d| d.join("cpsolver2"))
}

/// Create this process's log file. Tests keep their logs in memory only.
fn open_log_file() -> (Option<File>, Option<PathBuf>) {
    if cfg!(test) {
        return (None, None);
    }
    let Some(dir) = log_dir() else {
        return (None, None);
    };
    if create_private_dir(&dir).is_err() {
        return (None, None);
    }
    prune_old_logs(&dir, SystemTime::now());
    let path = dir.join(format!("{}{}.log", LOG_PREFIX, std::process::id()));
    match create_private_file(&path) {
        Ok(file) => (Some(file), Some(path)),
        Err(_) => (None, None),
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Create `path` as a new file readable only by the user. `create_new`
/// refuses an existing file or a symlink in its place (O_EXCL), and
/// O_NOFOLLOW guards the open itself.
fn create_private_file(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// Delete log files older than [`MAX_LOG_AGE`], then all but the newest
/// [`MAX_KEPT_LOGS`]. Only regular files named like our logs are touched.
fn prune_old_logs(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(LOG_PREFIX) && name.ends_with(".log")
        })
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file()
                .then(|| (meta.modified().unwrap_or(UNIX_EPOCH), e.path()))
        })
        .collect();
    logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (i, (modified, path)) in logs.iter().enumerate() {
        let too_old = now
            .duration_since(*modified)
            .is_ok_and(|age| age > MAX_LOG_AGE);
        if too_old || i >= MAX_KEPT_LOGS {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Write a log message with a timestamp. Multi-line messages become
/// several lines.
pub fn log(message: &str) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut st = state();
    for (i, line) in message.split('\n').enumerate() {
        let line = if i == 0 {
            format!("[{}] {}", timestamp, line)
        } else {
            line.to_string()
        };
        st.push(line);
    }
}

/// Forwards records from the `log` facade (which the RAVYN engine logs
/// through) into this log, so they reach the file and the log viewer.
struct Bridge;

impl ::log::Log for Bridge {
    fn enabled(&self, metadata: &::log::Metadata) -> bool {
        metadata.level() <= ::log::Level::Info
    }

    fn log(&self, record: &::log::Record) {
        if self.enabled(record.metadata()) {
            log(&record.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// Route the engine's log records into this log. Only the first call
/// installs the bridge; later calls do nothing.
pub fn install_engine_bridge() {
    static BRIDGE: Bridge = Bridge;
    if ::log::set_logger(&BRIDGE).is_ok() {
        ::log::set_max_level(::log::LevelFilter::Info);
    }
}

/// Start a new section: the log viewer shows only lines logged from here
/// on (the file keeps everything).
pub fn start_section() {
    state().start_section();
}

/// Call `f` with the current section's lines (oldest first).
pub fn with_section_lines<R>(f: impl FnOnce(&[String]) -> R) -> R {
    f(state().section())
}

/// The log file path for display ("(in memory)" when there is no file).
pub fn log_path_str() -> String {
    state()
        .path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "(in memory)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prunes_old_and_excess_logs() {
        let dir = std::env::temp_dir().join(format!("cpsolver2-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..(MAX_KEPT_LOGS + 3) {
            std::fs::write(dir.join(format!("{}{}.log", LOG_PREFIX, i)), "x").unwrap();
        }
        std::fs::write(dir.join("unrelated.txt"), "keep").unwrap();

        // Nothing is old yet: only the excess beyond the cap goes.
        prune_old_logs(&dir, SystemTime::now());
        let count = |dir: &Path| {
            std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(LOG_PREFIX))
                .count()
        };
        assert_eq!(count(&dir), MAX_KEPT_LOGS);

        // A week later every log is stale; other files are untouched.
        prune_old_logs(&dir, SystemTime::now() + MAX_LOG_AGE * 2);
        assert_eq!(count(&dir), 0);
        assert!(dir.join("unrelated.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn log_file_is_private_and_refuses_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cpsolver2-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("a.log");
        let file = create_private_file(&path).unwrap();
        drop(file);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // A planted symlink is not followed.
        let target = dir.join("target.txt");
        std::fs::write(&target, "secret").unwrap();
        let link = dir.join("b.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(create_private_file(&link).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sections_and_ring_overflow() {
        let mut st = LogState::new(None, None);
        st.push("before".to_string());
        st.start_section();
        st.push("inside".to_string());
        assert_eq!(st.section(), ["inside".to_string()]);

        // Overflowing the ring drops the oldest lines; the section start
        // stays anchored to the same logical line until it scrolls away.
        for i in 0..RING_CAPACITY {
            st.push(format!("line {i}"));
        }
        assert_eq!(st.lines.len(), RING_CAPACITY);
        assert_eq!(st.section().len(), RING_CAPACITY);
        assert_eq!(st.section()[0], "line 0");
        st.start_section();
        assert!(st.section().is_empty());
    }
}
