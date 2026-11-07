//! The optional config file (`$XDG_CONFIG_HOME/edh-scout/config.toml`): settings that are a matter
//! of taste rather than of one run. A missing file means all defaults.
//!
//! ```toml
//! [tui]
//! # Keep fetching eligible decks past the -n budget for as long as the browser is open.
//! keep_fetching = true
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub tui: TuiConfig,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    /// Once the run's fetch budget is spent, keep fetching the remaining eligible decks (in the
    /// same order, at the same pace) until the browser is closed.
    pub keep_fetching: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { keep_fetching: true }
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
        Ok(toml::from_str(text)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        assert!(Config::parse("").unwrap().tui.keep_fetching);
        assert!(!Config::parse("[tui]\nkeep_fetching = false\n").unwrap().tui.keep_fetching);
    }

    #[test]
    fn typos_are_errors() {
        assert!(Config::parse("[tui]\nkeep_fetchin = false\n").is_err());
        assert!(Config::parse("[tuii]\n").is_err());
    }
}
