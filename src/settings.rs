//! Application settings with TOML persistence.
//!
//! Settings are stored in `~/.config/cpsolver2/settings.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::SortMode;

/// The lexicon used by Crossplay (fixed, not user-configurable).
pub const LEXICON: &str = "NWL23";

/// Display preferences. Missing keys take their defaults, so a file
/// written by an older version still loads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Default sort mode for suggestions
    pub default_sort: DefaultSort,
    /// Show sidebar by default
    pub sidebar_visible: bool,
}

/// Sort mode options for settings (maps to app::SortMode)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DefaultSort {
    #[default]
    Equity,
    Mcts,
    WinPct,
    Score,
    Leave,
    Length,
    Alpha,
}

impl DefaultSort {
    pub fn to_sort_mode(self) -> SortMode {
        match self {
            DefaultSort::Equity => SortMode::Equity,
            DefaultSort::Mcts => SortMode::MctsEquity,
            DefaultSort::WinPct => SortMode::WinPct,
            DefaultSort::Score => SortMode::Score,
            DefaultSort::Leave => SortMode::Leave,
            DefaultSort::Length => SortMode::WordLength,
            DefaultSort::Alpha => SortMode::Alpha,
        }
    }

    pub fn from_sort_mode(mode: SortMode) -> Self {
        match mode {
            SortMode::Equity => DefaultSort::Equity,
            SortMode::MctsEquity => DefaultSort::Mcts,
            SortMode::WinPct => DefaultSort::WinPct,
            SortMode::Score => DefaultSort::Score,
            SortMode::Leave => DefaultSort::Leave,
            SortMode::WordLength => DefaultSort::Length,
            SortMode::Alpha => DefaultSort::Alpha,
        }
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            default_sort: DefaultSort::default(),
            sidebar_visible: true,
        }
    }
}

/// All application settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub display: DisplaySettings,
}

impl Settings {
    /// Get the path to the settings file
    pub fn config_path() -> Option<PathBuf> {
        // Tests must never read or overwrite the user's real settings.
        if cfg!(test) {
            return None;
        }
        dirs::config_dir().map(|p| p.join("cpsolver2").join("settings.toml"))
    }

    /// Load settings from file, or return defaults if file doesn't exist
    pub fn load() -> Self {
        Self::load_or_recover().0
    }

    /// Load settings, with a message for the user when the file could not
    /// be parsed: it is then moved aside to `settings.toml.bak` and the
    /// defaults are used.
    pub fn load_or_recover() -> (Self, Option<String>) {
        match Self::config_path() {
            Some(path) => Self::load_from(&path),
            None => (Self::default(), None),
        }
    }

    fn load_from(path: &Path) -> (Self, Option<String>) {
        let Ok(content) = fs::read_to_string(path) else {
            return (Self::default(), None);
        };
        match toml::from_str(&content) {
            Ok(settings) => (settings, None),
            Err(e) => {
                let backup = path.with_extension("toml.bak");
                let note = match fs::rename(path, &backup) {
                    Ok(()) => format!(
                        "Settings file was unreadable ({}); saved it as {} and used defaults",
                        e.message(),
                        backup.display()
                    ),
                    Err(_) => format!(
                        "Settings file was unreadable ({}); using defaults",
                        e.message()
                    ),
                };
                (Self::default(), Some(note))
            }
        }
    }

    /// Save settings to file
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path().ok_or("Could not determine config directory")?;
        self.save_to(&path)
    }

    /// Write atomically: a temporary file in the same directory, renamed
    /// over the old one, so a crash mid-write cannot leave a torn file.
    fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension(format!("toml.tmp{}", std::process::id()));
        fs::write(&tmp, content).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            e.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = Settings::default();
        assert_eq!(s.display.default_sort, DefaultSort::Equity);
        assert!(s.display.sidebar_visible);
    }

    #[test]
    fn test_settings_toml_roundtrip() {
        let settings = Settings::default();
        let toml_str = toml::to_string(&settings).unwrap();
        let parsed: Settings = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.display.default_sort, settings.display.default_sort);
        assert_eq!(
            parsed.display.sidebar_visible,
            settings.display.sidebar_visible
        );
    }

    #[test]
    fn test_sort_mode_conversion() {
        assert_eq!(DefaultSort::Equity.to_sort_mode(), SortMode::Equity);
        assert_eq!(DefaultSort::Mcts.to_sort_mode(), SortMode::MctsEquity);
        assert_eq!(DefaultSort::Score.to_sort_mode(), SortMode::Score);
        assert_eq!(DefaultSort::Leave.to_sort_mode(), SortMode::Leave);
        assert_eq!(DefaultSort::Length.to_sort_mode(), SortMode::WordLength);
        assert_eq!(DefaultSort::Alpha.to_sort_mode(), SortMode::Alpha);
    }

    fn temp_settings_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cpsolver2-settings-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("settings.toml")
    }

    /// Keys added in later versions take their defaults when absent.
    #[test]
    fn missing_keys_use_defaults() {
        let parsed: Settings = toml::from_str("[display]\nsidebar_visible = false\n").unwrap();
        assert!(!parsed.display.sidebar_visible);
        assert_eq!(parsed.display.default_sort, DefaultSort::Equity);
        let empty: Settings = toml::from_str("").unwrap();
        assert!(empty.display.sidebar_visible);
    }

    #[test]
    fn unreadable_file_is_backed_up() {
        let path = temp_settings_path("bad");
        fs::write(&path, "display = [not valid").unwrap();
        let (settings, note) = Settings::load_from(&path);
        assert!(settings.display.sidebar_visible);
        let note = note.expect("the user is told");
        assert!(note.contains("settings.toml.bak"), "{note}");
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(path.with_extension("toml.bak")).unwrap(),
            "display = [not valid"
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_round_trips_atomically() {
        let path = temp_settings_path("save");
        let mut settings = Settings::default();
        settings.display.default_sort = DefaultSort::Score;
        settings.save_to(&path).unwrap();
        let (loaded, note) = Settings::load_from(&path);
        assert!(note.is_none());
        assert_eq!(loaded.display.default_sort, DefaultSort::Score);
        // No temporary file is left behind.
        let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_lexicon_constant() {
        assert_eq!(LEXICON, "NWL23");
    }
}
