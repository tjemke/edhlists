//! Downloading and (re)building the local card and combo databases.
//!
//! Three bulk downloads (Scryfall card printings and oracle tags, Commander Spellbook variants)
//! replace what would otherwise be thousands of per-card API calls. The raw files are kept, so a schema change in
//! this tool only rebuilds — it never re-downloads.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::cards::{self, CARDS_SCHEMA, CardDb};
use crate::combos::{self, COMBOS_SCHEMA, ComboDb};
use crate::config::{DB_STALE_DAYS, Paths, SCRYFALL_BULK_CARDS, SCRYFALL_BULK_TAGS, SPELLBOOK_DUMP};
use crate::http::{Http, ensure_online};
use crate::tags::load_role_tags;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default)]
pub struct SyncWhat {
    pub cards: bool,
    pub combos: bool,
    /// Reuse a raw download already on disk instead of fetching it again.
    pub use_existing_raw: bool,
}

#[derive(Deserialize)]
struct BulkMeta {
    #[serde(default)]
    download_uri: Option<String>,
    #[serde(default)]
    jsonl_download_uri: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    compressed_size: Option<u64>,
}

/// Resolve a Scryfall bulk-data entry to its current file and download it (2 requests).
fn download_bulk(http: &Http, meta_url: &str, what: &str, dest: &Path) -> Result<()> {
    ensure_online(http, &format!("downloading {what}"))?;
    let meta: BulkMeta = serde_json::from_slice(&http.get_bytes(meta_url)?).context("Scryfall bulk-data metadata")?;
    let url = meta
        .jsonl_download_uri
        .or(meta.download_uri)
        .context("Scryfall's bulk-data entry has no download URI — did their API change?")?;
    let mb = meta.compressed_size.or(meta.size).unwrap_or(0) as f64 / 1e6;
    eprintln!("downloading {what} (~{mb:.0} MB) ...");
    http.download(&url, dest)?;
    Ok(())
}

pub fn sync(http: &Http, paths: &Paths, what: SyncWhat) -> Result<()> {
    let mut card_db = None;
    if what.cards {
        let raw = paths.scryfall_raw();
        let tags_raw = paths.scryfall_tags_raw();
        if !(what.use_existing_raw && raw.exists()) {
            download_bulk(http, SCRYFALL_BULK_CARDS, "Scryfall card printings", &raw)?;
        }
        if !(what.use_existing_raw && tags_raw.exists()) {
            download_bulk(http, SCRYFALL_BULK_TAGS, "Scryfall Tagger oracle tags", &tags_raw)?;
        }
        eprintln!("building card database ...");
        let tags = load_role_tags(&tags_raw)?;
        let db = cards::build_from_bulk(&raw, Some(&tags))?;
        let gcs = db.cards.iter().filter(|c| c.game_changer).count();
        db.save(&paths.cards_db())?;
        eprintln!(
            "  {} cards, {gcs} Game Changers, {} with Tagger roles",
            db.cards.len(),
            db.cards.iter().filter(|c| c.tagged).count()
        );
        card_db = Some(db);
    }
    if what.combos {
        if card_db.is_none() && paths.cards_db().exists() {
            card_db = Some(CardDb::load(&paths.cards_db())?);
        }
        let raw = paths.spellbook_raw();
        if !(what.use_existing_raw && raw.exists()) {
            ensure_online(http, "downloading Commander Spellbook combos")?;
            eprintln!("downloading Commander Spellbook combo dump (~32 MB) ...");
            http.download(SPELLBOOK_DUMP, &raw)?;
        }
        eprintln!("building combo database ...");
        let db = combos::build_from_dump(&raw, card_db.as_ref())?;
        db.save(&paths.combos_db())?;
        eprintln!("  {} commander-legal combos", db.combos.len());
    }
    Ok(())
}

/// Load both databases, syncing whatever is missing or from an older schema first. Each file is
/// read once: a current one is kept as loaded, not loaded again after the schema check.
pub fn ensure_databases(http: &Http, paths: &Paths) -> Result<(CardDb, ComboDb)> {
    let cards = current(&paths.cards_db(), CardDb::load, |d| d.schema == CARDS_SCHEMA);
    let combos = current(&paths.combos_db(), ComboDb::load, |d| d.schema == COMBOS_SCHEMA);
    if cards.is_none() || combos.is_none() {
        let missing_raw = (cards.is_none() && !(paths.scryfall_raw().exists() && paths.scryfall_tags_raw().exists()))
            || (combos.is_none() && !paths.spellbook_raw().exists());
        if http.offline() && missing_raw {
            bail!("no local card/combo database yet; run `edh-scout sync` once while online");
        }
        eprintln!("local databases missing or outdated — building them first");
        sync(http, paths, SyncWhat { cards: cards.is_none(), combos: combos.is_none(), use_existing_raw: true })?;
    }
    let cards = match cards {
        Some(db) => db,
        None => CardDb::load(&paths.cards_db())?,
    };
    let combos = match combos {
        Some(db) => db,
        None => ComboDb::load(&paths.combos_db())?,
    };
    if cards.age_days() > DB_STALE_DAYS {
        eprintln!(
            "note: card data is {} days old — `edh-scout sync` refreshes prices and Game Changers",
            cards.age_days()
        );
    }
    Ok((cards, combos))
}

/// The database at `path`, if it exists, loads, and passes `ok` (the schema check).
fn current<T>(path: &Path, load: fn(&Path) -> Result<T>, ok: fn(&T) -> bool) -> Option<T> {
    if !path.exists() {
        return None;
    }
    load(path).ok().filter(ok)
}
