//! Centralized logging for crossplay-solver
//!
//! Each instance gets its own log file based on process ID.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Get the instance-specific log file path
pub fn log_path() -> &'static PathBuf {
    static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
    LOG_PATH.get_or_init(|| {
        let pid = std::process::id();
        PathBuf::from(format!("/tmp/crossplay-{}.log", pid))
    })
}

/// Write a log message with timestamp
pub fn log(message: &str) {
    let path = log_path();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{}] {}", timestamp, message);
    }
}

/// Clear the log file
pub fn clear_log() {
    let path = log_path();
    let _ = std::fs::write(path, "");
}

/// Read all log lines from the file
pub fn read_log_lines() -> Vec<String> {
    let path = log_path();
    match File::open(path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            reader.lines().filter_map(|l| l.ok()).collect()
        }
        Err(_) => Vec::new(),
    }
}

/// Get the log file path as a string (for display)
pub fn log_path_str() -> String {
    log_path().to_string_lossy().to_string()
}
