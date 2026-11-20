//! The optional config file (`$XDG_CONFIG_HOME/edh-scout/config.toml`): settings that are a matter
//! of taste rather than of one run. A missing file means all defaults.
//!
//! ```toml
//! [tui]
//! # Keep fetching eligible decks past the -n budget for as long as the browser is open.
//! keep_fetching = true
//! # Deck list beside the details ("side-by-side"), above them ("stacked"), or "auto".
//! layout = "auto"
//!
//! [search]
//! # Searching across commanders: decks failing the -T filters, none passing, before a commander
//! # counts as a miss (and too many misses widen the search).
//! miss_after = 3
//! # Decks fetched between one widening and the next; 0 never widens.
//! widen_every = 5
//!
//! [performance]
//! # Threads for bulk work (reading, parsing and scoring stored decks); default: half the CPUs.
//! threads = 4
//!
//! [goldfish]
//! # Games the mana goldfish plays per deck for the details pane, `check` and the report;
//! # 0 turns it off.
//! games = 5000
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub tui: TuiConfig,
    pub search: SearchConfig,
    pub performance: PerformanceConfig,
    pub goldfish: GoldfishConfig,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GoldfishConfig {
    /// Games the mana goldfish ([`crate::mana`]) plays per deck; 0 turns it off.
    pub games: u32,
}

/// Enough games that a share moves by about a point at most from one run of games to another.
pub const GOLDFISH_GAMES: u32 = 5000;

impl Default for GoldfishConfig {
    fn default() -> Self {
        Self { games: GOLDFISH_GAMES }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// Searching across commanders: how many of a commander's decks may fail the `-T` filters,
    /// none passing, before it counts as a miss (see [`crate::plan`]). `scout --miss-after`
    /// overrides it.
    pub miss_after: usize,
    /// Searching across commanders: decks fetched between one widening of the search and the
    /// next; 0 never widens. `scout --widen-every` overrides it.
    pub widen_every: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { miss_after: crate::plan::MISS_AFTER, widen_every: crate::plan::WIDEN_EVERY }
    }
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PerformanceConfig {
    /// Threads for bulk work: reading, parsing and scoring stored decks. `None`: half the CPUs.
    pub threads: Option<usize>,
}

impl PerformanceConfig {
    /// How many threads bulk work may use: the setting, or half the CPUs the system reports (at
    /// least one), so a long scoring run doesn't take over the whole machine.
    pub fn threads(&self) -> usize {
        self.threads.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1).max(1))
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    /// Once the run's fetch budget is spent, keep fetching the remaining eligible decks (in the
    /// same order, at the same pace) until the browser is closed.
    pub keep_fetching: bool,
    /// Where the browser puts the deck list and the details pane.
    pub layout: PaneLayout,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { keep_fetching: true, layout: PaneLayout::Auto }
    }
}

/// The browser's deck list and details pane: side by side, or one above the other.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneLayout {
    /// Side by side in a landscape terminal at least [`SIDE_BY_SIDE_MIN_WIDTH`] columns wide,
    /// stacked otherwise (so a portrait terminal stacks them however wide it is).
    #[default]
    Auto,
    /// Side by side whenever the terminal is wide enough, whatever its shape.
    SideBySide,
    /// Always stacked.
    Stacked,
}

/// Narrower than this, the deck list and the details don't fit side by side.
pub const SIDE_BY_SIDE_MIN_WIDTH: u16 = 110;

/// A character cell is about twice as tall as it is wide, so a terminal is portrait (taller than
/// wide on screen) when it has more than half as many rows as columns.
const CELL_ASPECT: u32 = 2;

impl PaneLayout {
    /// Put the panes side by side in a `width` × `height` terminal (in cells)?
    pub fn side_by_side(self, width: u16, height: u16) -> bool {
        let wide_enough = width >= SIDE_BY_SIDE_MIN_WIDTH;
        match self {
            PaneLayout::Auto => wide_enough && u32::from(height) * CELL_ASPECT <= u32::from(width),
            PaneLayout::SideBySide => wide_enough,
            PaneLayout::Stacked => false,
        }
    }
}

impl Config {
    /// The config file to read: `explicit`, else `$EDH_SCOUT_CONFIG`, else the platform default.
    pub fn path(explicit: Option<PathBuf>) -> Option<PathBuf> {
        explicit
            .or_else(|| std::env::var_os("EDH_SCOUT_CONFIG").map(PathBuf::from))
            .or_else(|| directories::ProjectDirs::from("", "", "edh-scout").map(|d| d.config_dir().join("config.toml")))
    }

    /// Read the config file. A missing default file is fine; a missing explicit one is an error.
    pub fn load(explicit: Option<PathBuf>) -> Result<Self> {
        let required = explicit.is_some() || std::env::var_os("EDH_SCOUT_CONFIG").is_some();
        let Some(path) = Self::path(explicit) else { return Ok(Self::default()) };
        if !required && !path.exists() {
            return Ok(Self::default());
        }
        Self::read(&path)
    }

    fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        if config.performance.threads == Some(0) {
            anyhow::bail!("[performance] threads must be at least 1");
        }
        if config.search.miss_after == 0 {
            anyhow::bail!("[search] miss_after must be at least 1");
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        assert!(Config::parse("").unwrap().tui.keep_fetching);
        assert!(!Config::parse("[tui]\nkeep_fetching = false\n").unwrap().tui.keep_fetching);
        assert_eq!(Config::parse("").unwrap().tui.layout, PaneLayout::Auto);
        assert_eq!(Config::parse("[tui]\nlayout = \"stacked\"\n").unwrap().tui.layout, PaneLayout::Stacked);
        assert_eq!(Config::parse("[tui]\nlayout = \"side-by-side\"\n").unwrap().tui.layout, PaneLayout::SideBySide);
        assert!(Config::parse("[tui]\nlayout = \"sideways\"\n").is_err());
    }

    #[test]
    fn layouts() {
        use PaneLayout::*;
        // Landscape: 240×67 (a 1080p screen at 8×16 cells).
        assert!(Auto.side_by_side(240, 67));
        assert!(SideBySide.side_by_side(240, 67));
        assert!(!Stacked.side_by_side(240, 67));
        // Portrait but wide enough: 135×120 (a 1080×1920 screen).
        assert!(!Auto.side_by_side(135, 120));
        assert!(SideBySide.side_by_side(135, 120));
        // Too narrow either way.
        assert!(!Auto.side_by_side(100, 30));
        assert!(!SideBySide.side_by_side(100, 30));
    }

    #[test]
    fn threads() {
        assert_eq!(Config::parse("[performance]\nthreads = 3\n").unwrap().performance.threads(), 3);
        assert!(Config::parse("").unwrap().performance.threads() >= 1);
        assert!(Config::parse("[performance]\nthreads = 0\n").is_err());
    }

    #[test]
    fn miss_after() {
        assert_eq!(Config::parse("").unwrap().search.miss_after, crate::plan::MISS_AFTER);
        assert_eq!(Config::parse("[search]\nmiss_after = 8\n").unwrap().search.miss_after, 8);
        assert!(Config::parse("[search]\nmiss_after = 0\n").is_err(), "0 would make every commander a miss");
    }

    #[test]
    fn widen_every() {
        assert_eq!(Config::parse("").unwrap().search.widen_every, crate::plan::WIDEN_EVERY);
        assert_eq!(Config::parse("[search]\nwiden_every = 20\n").unwrap().search.widen_every, 20);
        assert_eq!(Config::parse("[search]\nwiden_every = 0\n").unwrap().search.widen_every, 0, "0 turns it off");
    }

    #[test]
    fn goldfish_games() {
        assert_eq!(Config::parse("").unwrap().goldfish.games, GOLDFISH_GAMES);
        assert_eq!(Config::parse("[goldfish]\ngames = 20000\n").unwrap().goldfish.games, 20000);
        assert_eq!(Config::parse("[goldfish]\ngames = 0\n").unwrap().goldfish.games, 0, "0 turns it off");
        assert!(Config::parse("[goldfish]\ngames = -1\n").is_err());
    }

    #[test]
    fn typos_are_errors() {
        assert!(Config::parse("[tui]\nkeep_fetchin = false\n").is_err());
        assert!(Config::parse("[tuii]\n").is_err());
    }
}
