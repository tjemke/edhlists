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
//! # Defaults for the command-line options of the same names; the options override them.
//! brackets = "2-3"          # scout, favorites, check
//! power = "5.5-7"           # default: derived from the brackets; ignored with -b
//! rank = "fit"              # scout, favorites
//! weights = "fit=0.5,salt=0.2,structure=0.15,synergy=0.15"
//! fetch = 60
//! commanders = 30
//! max_price = 300
//! max_salt = 40
//! declared_only = false
//! since = 365               # days, or "YYYY-MM-DD"
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
//! # Forge's card scripts to read (its res/cardsfolder) instead of the ones `sync` downloads;
//! # EDH_SCOUT_FORGE_CARDS overrides it.
//! forge_cards = "/path/to/forge/res/cardsfolder"
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use edh_data::deck::BracketRange;
use edh_data::text::since_date;
use serde::Deserialize;

use crate::rank::{RankMode, Weights, parse_range};

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
    /// Forge's card scripts to read instead of the download ([`edh_data::forge::ForgeSource`]).
    pub forge_cards: Option<PathBuf>,
}

/// Enough games that a share moves by about a point at most from one run of games to another.
pub const GOLDFISH_GAMES: u32 = 5000;

impl Default for GoldfishConfig {
    fn default() -> Self {
        Self { games: GOLDFISH_GAMES, forge_cards: None }
    }
}

/// Defaults for `scout`'s options (and `favorites`' and `check`'s, where they have them), each
/// overridden by its command-line option.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// Target brackets (`--brackets`), e.g. "3" or "2-3".
    pub brackets: String,
    /// Target EDHPowerLevel band (`--power`), e.g. "5.5-7". `None`: derived from the brackets.
    /// Kept only while `--brackets` isn't given, since it goes with these brackets.
    pub power: Option<String>,
    /// How to rank decks (`--rank`).
    pub rank: RankMode,
    /// Weights for the combined ranking (`--weights`). `None`: [`Weights::default`].
    pub weights: Option<String>,
    /// New decklists to fetch per run (`--fetch`).
    pub fetch: usize,
    /// Searching across commanders: deck tables to scan first (`--commanders`).
    pub commanders: usize,
    /// Skip decks priced above this (`--max-price`).
    pub max_price: Option<f64>,
    /// Skip decks with a salt score above this (`--max-salt`).
    pub max_salt: Option<f64>,
    /// Only decks declared in the target brackets (`--declared-only`; `--no-declared-only` undoes it).
    pub declared_only: bool,
    /// Only decks saved since then (`--since`).
    pub since: Option<Since>,
    /// Searching across commanders: how many of a commander's decks may fail the `-T` filters,
    /// none passing, before it counts as a miss (see [`crate::plan`]). `scout --miss-after`
    /// overrides it.
    pub miss_after: usize,
    /// Searching across commanders: decks fetched between one widening of the search and the
    /// next; 0 never widens. `scout --widen-every` overrides it.
    pub widen_every: usize,
}

pub const BRACKETS: &str = "2-3";
pub const FETCH: usize = 60;
pub const COMMANDERS: usize = 30;

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            brackets: BRACKETS.into(),
            power: None,
            rank: RankMode::default(),
            weights: None,
            fetch: FETCH,
            commanders: COMMANDERS,
            max_price: None,
            max_salt: None,
            declared_only: false,
            since: None,
            miss_after: crate::plan::MISS_AFTER,
            widen_every: crate::plan::WIDEN_EVERY,
        }
    }
}

/// `since` in the config file: a number of days back, or a date.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Since {
    Days(u64),
    Date(String),
}

impl Since {
    /// As `--since` would take it.
    pub fn arg(&self) -> String {
        match self {
            Since::Days(n) => n.to_string(),
            Since::Date(d) => d.clone(),
        }
    }
}

impl SearchConfig {
    /// Read every setting the way its option would, so a bad one is reported once, on loading.
    fn check(&self) -> Result<()> {
        BracketRange::parse(&self.brackets).context("brackets")?;
        if let Some(p) = &self.power {
            parse_range::<f64>(p).context("power")?;
        }
        if let Some(w) = &self.weights {
            w.parse::<Weights>().context("weights")?;
        }
        if let Some(s) = &self.since
            && since_date(&s.arg()).is_none()
        {
            anyhow::bail!("since takes a number of days or \"YYYY-MM-DD\", got \"{}\"", s.arg());
        }
        if self.miss_after == 0 {
            anyhow::bail!("miss_after must be at least 1");
        }
        Ok(())
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
        config.search.check().context("[search]")?;
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
    fn search_defaults_match_the_options_without_a_config() {
        let search = Config::parse("").unwrap().search;
        assert_eq!(search.brackets, BRACKETS);
        assert_eq!(search.power, None);
        assert_eq!(search.rank, RankMode::Fit);
        assert_eq!(search.weights, None);
        assert_eq!((search.fetch, search.commanders), (FETCH, COMMANDERS));
        assert_eq!((search.max_price, search.max_salt, search.declared_only), (None, None, false));
        assert_eq!(search.since, None);
    }

    #[test]
    fn search_defaults_can_be_set() {
        let search = Config::parse(
            "[search]\nbrackets = \"3\"\npower = \"6-7.5\"\nrank = \"combined\"\nweights = \"fit=0.4,salt=0.6\"\n\
             fetch = 100\ncommanders = 50\nmax_price = 300\nmax_salt = 40.5\ndeclared_only = true\nsince = 365\n",
        )
        .unwrap()
        .search;
        assert_eq!(search.brackets, "3");
        assert_eq!(search.power.as_deref(), Some("6-7.5"));
        assert_eq!(search.rank, RankMode::Combined);
        assert_eq!(search.weights.as_deref(), Some("fit=0.4,salt=0.6"));
        assert_eq!((search.fetch, search.commanders), (100, 50));
        assert_eq!((search.max_price, search.max_salt), (Some(300.0), Some(40.5)), "whole numbers are prices too");
        assert!(search.declared_only);
        assert_eq!(search.since, Some(Since::Days(365)));
        let dated = Config::parse("[search]\nsince = \"2025-06-01\"\n").unwrap().search;
        assert_eq!(dated.since, Some(Since::Date("2025-06-01".into())));
    }

    #[test]
    fn bad_search_defaults_are_reported_on_loading() {
        for bad in [
            "brackets = \"2-9\"",
            "brackets = \"two\"",
            "power = \"high\"",
            "rank = \"best\"",
            "weights = \"fun=1\"",
            "since = \"last year\"",
            "since = -3",
            "fetch = -1",
        ] {
            let err = Config::parse(&format!("[search]\n{bad}\n"));
            assert!(err.is_err(), "[search] {bad} should be an error");
        }
    }

    #[test]
    fn goldfish_games() {
        assert_eq!(Config::parse("").unwrap().goldfish.games, GOLDFISH_GAMES);
        assert_eq!(Config::parse("[goldfish]\ngames = 20000\n").unwrap().goldfish.games, 20000);
        assert_eq!(Config::parse("[goldfish]\ngames = 0\n").unwrap().goldfish.games, 0, "0 turns it off");
        assert!(Config::parse("[goldfish]\ngames = -1\n").is_err());
        let forge = Config::parse("[goldfish]\nforge_cards = \"/opt/forge/res/cardsfolder\"\n").unwrap();
        assert_eq!(forge.goldfish.forge_cards.as_deref(), Some(Path::new("/opt/forge/res/cardsfolder")));
    }

    #[test]
    fn typos_are_errors() {
        assert!(Config::parse("[tui]\nkeep_fetchin = false\n").is_err());
        assert!(Config::parse("[tuii]\n").is_err());
    }
}
