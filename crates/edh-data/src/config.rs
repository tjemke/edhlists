//! Paths, endpoints and politeness settings shared across the crate.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const EDHREC_SITE: &str = "https://edhrec.com";
pub const EDHREC_JSON: &str = "https://json.edhrec.com/pages";
/// Every printing (we price cards at their cheapest printing, as EDHPowerLevel does).
pub const SCRYFALL_BULK_CARDS: &str = "https://api.scryfall.com/bulk-data/default-cards";
/// Scryfall Tagger oracle tags (what each card does: ramp, removal, ...).
pub const SCRYFALL_BULK_TAGS: &str = "https://api.scryfall.com/bulk-data/oracle-tags";
pub const SPELLBOOK_DUMP: &str = "https://json.commanderspellbook.com/variants.json.gz";

/// How long fetched pages stay fresh before we ask again. Decklists are snapshots and never expire.
pub const TTL_DECK_TABLE: Duration = Duration::from_secs(24 * 3600);
pub const TTL_COMMANDER_PAGE: Duration = Duration::from_secs(7 * 24 * 3600);
pub const TTL_SALT: Duration = Duration::from_secs(14 * 24 * 3600);

/// Card and combo databases older than this get a "consider `sync`" note.
pub const DB_STALE_DAYS: u64 = 21;

/// Minimum spacing between two requests to the same host. Everything is sequential: there is never
/// more than one request in flight, so these are hard upper bounds on our request rate.
pub fn host_gap(host: &str) -> Duration {
    match host {
        // Server-rendered deck pages: the expensive thing we ask EDHREC for, so be gentle.
        "edhrec.com" | "www.edhrec.com" => Duration::from_millis(2000),
        // Static JSON on a CDN.
        "json.edhrec.com" => Duration::from_millis(1000),
        // Scryfall asks for 50-100 ms between requests.
        h if h.ends_with("scryfall.com") || h.ends_with("scryfall.io") => Duration::from_millis(150),
        _ => Duration::from_millis(1000),
    }
}

pub fn user_agent() -> String {
    format!("edh-scout/{VERSION} (personal deck-browsing tool; sequential, cached requests)")
}

/// Where everything we download lives. Data on disk is cheap; requests are not.
#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(override_dir: Option<PathBuf>) -> Result<Self> {
        let root = match override_dir.or_else(|| std::env::var_os("EDH_SCOUT_CACHE").map(PathBuf::from)) {
            Some(p) => p,
            None => directories::ProjectDirs::from("", "", "edh-scout")
                .context("could not determine a cache directory; pass --cache-dir")?
                .cache_dir()
                .to_path_buf(),
        };
        Ok(Self { root })
    }

    pub fn http(&self) -> PathBuf {
        self.root.join("http")
    }
    pub fn raw(&self) -> PathBuf {
        self.root.join("raw")
    }
    pub fn cards_db(&self) -> PathBuf {
        self.root.join("cards.json.gz")
    }
    pub fn combos_db(&self) -> PathBuf {
        self.root.join("combos.json.gz")
    }
    pub fn scryfall_raw(&self) -> PathBuf {
        self.raw().join("scryfall-default-cards.jsonl.gz")
    }
    pub fn scryfall_tags_raw(&self) -> PathBuf {
        self.raw().join("scryfall-oracle-tags.jsonl.gz")
    }
    pub fn spellbook_raw(&self) -> PathBuf {
        self.raw().join("spellbook-variants.json.gz")
    }
}
