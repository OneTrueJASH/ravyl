//! Application settings with TOML persistence.
//!
//! Settings are stored in `~/.config/cpsolver2/settings.toml`.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::SortMode;

/// The lexicon used by Crossplay (fixed, not user-configurable).
pub const LEXICON: &str = "NWL23";

/// Display preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct Settings {
    pub display: DisplaySettings,
}

impl Settings {
    /// Get the path to the settings file
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("cpsolver2").join("settings.toml"))
    }

    /// Load settings from file, or return defaults if file doesn't exist
    pub fn load() -> Self {
        Self::config_path()
            .and_then(|path| fs::read_to_string(&path).ok())
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
    }

    /// Save settings to file
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path().ok_or("Could not determine config directory")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, content).map_err(|e| e.to_string())
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

    #[test]
    fn test_lexicon_constant() {
        assert_eq!(LEXICON, "NWL23");
    }
}
