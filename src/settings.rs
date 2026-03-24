//! Application settings with TOML persistence.
//!
//! Settings are stored in `~/.config/cpsolver2/settings.toml`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::app::SortMode;

/// RefEngine simulation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefSettings {
    /// Number of plies to look ahead (1-6)
    pub plies: u8,
    /// Number of iterations per move (100-10000)
    pub iterations: u32,
    /// Number of plays to simulate (5-150)
    pub num_plays: u32,
    /// Enable adaptive mode
    pub adaptive: bool,
    /// Plies for endgame solver (4-12)
    pub endgame_plies: u8,
}

impl Default for RefSettings {
    fn default() -> Self {
        Self {
            plies: 2,
            iterations: 1000,
            num_plays: 150,
            adaptive: true,
            endgame_plies: 8,
        }
    }
}

/// MCTS evaluation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MctsSettings {
    /// Number of candidate moves to evaluate (3-20)
    pub num_candidates: u32,
    /// Iterations per candidate (10-200)
    pub iterations_per_candidate: u32,
    /// Enable parallel evaluation
    pub parallel: bool,
}

impl Default for MctsSettings {
    fn default() -> Self {
        Self {
            num_candidates: 10,
            iterations_per_candidate: 50,
            parallel: true,
        }
    }
}

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
    Score,
    Leave,
    Length,
    Alpha,
}

impl DefaultSort {
    pub fn label(&self) -> &'static str {
        match self {
            DefaultSort::Equity => "Equity",
            DefaultSort::Mcts => "RefEngine",
            DefaultSort::Score => "Score",
            DefaultSort::Leave => "Leave",
            DefaultSort::Length => "Length",
            DefaultSort::Alpha => "A-Z",
        }
    }

    pub fn to_sort_mode(self) -> SortMode {
        match self {
            DefaultSort::Equity => SortMode::Equity,
            DefaultSort::Mcts => SortMode::MctsEquity,
            DefaultSort::Score => SortMode::Score,
            DefaultSort::Leave => SortMode::Leave,
            DefaultSort::Length => SortMode::WordLength,
            DefaultSort::Alpha => SortMode::Alpha,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            DefaultSort::Equity => DefaultSort::Mcts,
            DefaultSort::Mcts => DefaultSort::Score,
            DefaultSort::Score => DefaultSort::Leave,
            DefaultSort::Leave => DefaultSort::Length,
            DefaultSort::Length => DefaultSort::Alpha,
            DefaultSort::Alpha => DefaultSort::Equity,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            DefaultSort::Equity => DefaultSort::Alpha,
            DefaultSort::Mcts => DefaultSort::Equity,
            DefaultSort::Score => DefaultSort::Mcts,
            DefaultSort::Leave => DefaultSort::Score,
            DefaultSort::Length => DefaultSort::Leave,
            DefaultSort::Alpha => DefaultSort::Length,
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

/// Game settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameSettings {
    /// Lexicon to use
    pub lexicon: Lexicon,
}

/// Available lexicons
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lexicon {
    #[default]
    NWL23,
    CSW21,
    TWL06,
    Sowpods,
}

impl Lexicon {
    pub fn label(&self) -> &'static str {
        match self {
            Lexicon::NWL23 => "NWL23",
            Lexicon::CSW21 => "CSW21",
            Lexicon::TWL06 => "TWL06",
            Lexicon::Sowpods => "SOWPODS",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Lexicon::NWL23 => Lexicon::CSW21,
            Lexicon::CSW21 => Lexicon::TWL06,
            Lexicon::TWL06 => Lexicon::Sowpods,
            Lexicon::Sowpods => Lexicon::NWL23,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Lexicon::NWL23 => Lexicon::Sowpods,
            Lexicon::CSW21 => Lexicon::NWL23,
            Lexicon::TWL06 => Lexicon::CSW21,
            Lexicon::Sowpods => Lexicon::TWL06,
        }
    }
}

/// Performance settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PerformanceSettings {
    /// Number of threads (0 = auto)
    pub threads: u32,
}

/// All application settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub refengine: RefSettings,
    pub mcts: MctsSettings,
    pub display: DisplaySettings,
    pub game: GameSettings,
    pub performance: PerformanceSettings,
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
        assert_eq!(s.refengine.plies, 2);
        assert_eq!(s.refengine.iterations, 1000);
        assert_eq!(s.refengine.num_plays, 150);
        assert!(s.refengine.adaptive);
        assert_eq!(s.refengine.endgame_plies, 8);
        assert_eq!(s.mcts.num_candidates, 10);
        assert_eq!(s.mcts.iterations_per_candidate, 50);
        assert!(s.mcts.parallel);
        assert_eq!(s.display.default_sort, DefaultSort::Equity);
        assert!(s.display.sidebar_visible);
        assert_eq!(s.game.lexicon, Lexicon::NWL23);
        assert_eq!(s.performance.threads, 0);
    }

    #[test]
    fn test_settings_toml_roundtrip() {
        let settings = Settings::default();
        let toml_str = toml::to_string(&settings).unwrap();
        let parsed: Settings = toml::from_str(&toml_str).unwrap();

        // Compare fields individually since Settings may not derive PartialEq
        assert_eq!(parsed.refengine.plies, settings.refengine.plies);
        assert_eq!(parsed.refengine.iterations, settings.refengine.iterations);
        assert_eq!(parsed.refengine.num_plays, settings.refengine.num_plays);
        assert_eq!(parsed.refengine.adaptive, settings.refengine.adaptive);
        assert_eq!(parsed.refengine.endgame_plies, settings.refengine.endgame_plies);
        assert_eq!(parsed.mcts.num_candidates, settings.mcts.num_candidates);
        assert_eq!(
            parsed.mcts.iterations_per_candidate,
            settings.mcts.iterations_per_candidate
        );
        assert_eq!(parsed.mcts.parallel, settings.mcts.parallel);
        assert_eq!(parsed.display.default_sort, settings.display.default_sort);
        assert_eq!(
            parsed.display.sidebar_visible,
            settings.display.sidebar_visible
        );
        assert_eq!(parsed.game.lexicon, settings.game.lexicon);
        assert_eq!(parsed.performance.threads, settings.performance.threads);
    }

    #[test]
    fn test_lexicon_label() {
        assert_eq!(Lexicon::NWL23.label(), "NWL23");
        assert_eq!(Lexicon::CSW21.label(), "CSW21");
        assert_eq!(Lexicon::TWL06.label(), "TWL06");
        assert_eq!(Lexicon::Sowpods.label(), "SOWPODS");
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
}
