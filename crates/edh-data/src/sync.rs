//! Downloading and (re)building the local card and combo databases.
//!
//! Three bulk downloads (Scryfall card printings and oracle tags, Commander Spellbook variants)
//! replace what would otherwise be thousands of per-card API calls, and a fourth brings Forge's card
//! scripts for the mana goldfish. The raw files are kept, so a schema change in this tool only
//! rebuilds — it never re-downloads.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::cards::{self, CARDS_SCHEMA, CardDb};
use crate::combos::{self, COMBOS_SCHEMA, ComboDb};
use crate::config::{DB_STALE_DAYS, Paths, SCRYFALL_BULK_CARDS, SCRYFALL_BULK_TAGS, SCRYFALL_SETS, SPELLBOOK_DUMP};
use crate::forge::{FORGE_LATEST_RELEASE, FORGE_SCHEMA, ForgeDb, ForgeRaw, forge_tarball, read_tarball};
use crate::http::{Http, ensure_online};
use crate::progress::Progress;
use crate::storage::{read_json_gz, write_json_gz};
use crate::tags::load_role_tags;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default)]
pub struct SyncWhat {
    pub cards: bool,
    pub combos: bool,
    /// Forge's card scripts, from its latest release (unless a directory of them is configured:
    /// see [`crate::forge::ForgeSource`]).
    pub forge: bool,
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
fn download_bulk(http: &Http, meta_url: &str, what: &str, dest: &Path, progress: &dyn Progress) -> Result<()> {
    ensure_online(http, &format!("downloading {what}"))?;
    let meta: BulkMeta = serde_json::from_slice(&http.get_bytes(meta_url)?).context("Scryfall bulk-data metadata")?;
    let url = meta
        .jsonl_download_uri
        .or(meta.download_uri)
        .context("Scryfall's bulk-data entry has no download URI — did their API change?")?;
    let mb = meta.compressed_size.or(meta.size).unwrap_or(0) as f64 / 1e6;
    progress.step(&format!("downloading {what} (~{mb:.0} MB)"));
    http.download(&url, dest)?;
    Ok(())
}

pub fn sync(http: &Http, paths: &Paths, what: SyncWhat, progress: &dyn Progress) -> Result<()> {
    let mut card_db = None;
    if what.cards {
        let raw = paths.scryfall_raw();
        let tags_raw = paths.scryfall_tags_raw();
        if !(what.use_existing_raw && raw.exists()) {
            download_bulk(http, SCRYFALL_BULK_CARDS, "Scryfall card printings", &raw, progress)?;
        }
        if !(what.use_existing_raw && tags_raw.exists()) {
            download_bulk(http, SCRYFALL_BULK_TAGS, "Scryfall Tagger oracle tags", &tags_raw, progress)?;
        }
        let sets_raw = paths.scryfall_sets_raw();
        if !(what.use_existing_raw && sets_raw.exists()) {
            progress.step("downloading Scryfall's set list (~0.6 MB)");
            http.download(SCRYFALL_SETS, &sets_raw)?;
        }
        progress.step("building card database");
        let tags = load_role_tags(&tags_raw)?;
        // Kept downloads from before the set list was fetched build without it (no `b:`, `g:`).
        let db = cards::build_from_bulk(&raw, Some(&tags), sets_raw.exists().then_some(sets_raw.as_path()))?;
        let gcs = db.cards.iter().filter(|c| c.game_changer).count();
        db.save(&paths.cards_db())?;
        progress.note(&format!(
            "  {} cards, {gcs} Game Changers, {} with Tagger roles",
            db.cards.len(),
            db.cards.iter().filter(|c| c.tagged).count()
        ));
        card_db = Some(db);
    }
    if what.combos {
        if card_db.is_none() && paths.cards_db().exists() {
            card_db = Some(CardDb::load(&paths.cards_db())?);
        }
        let raw = paths.spellbook_raw();
        if !(what.use_existing_raw && raw.exists()) {
            ensure_online(http, "downloading Commander Spellbook combos")?;
            progress.step("downloading Commander Spellbook combo dump (~32 MB)");
            http.download(SPELLBOOK_DUMP, &raw)?;
        }
        progress.step("building combo database");
        let db = combos::build_from_dump(&raw, card_db.as_ref())?;
        db.save(&paths.combos_db())?;
        progress.note(&format!("  {} commander-legal combos", db.combos.len()));
    }
    if what.forge {
        sync_forge(http, paths, what.use_existing_raw, progress)?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// Download Forge's latest release's scripts, unless they're the ones already here, and build
/// their database.
fn sync_forge(http: &Http, paths: &Paths, use_existing_raw: bool, progress: &dyn Progress) -> Result<()> {
    let raw_path = paths.forge_raw();
    let raw = if use_existing_raw && raw_path.exists() {
        read_json_gz::<ForgeRaw>(&raw_path)?
    } else {
        ensure_online(http, "downloading Forge's card scripts")?;
        let latest: Release = serde_json::from_slice(&http.get_bytes(FORGE_LATEST_RELEASE)?)
            .context("Forge's latest release on GitHub")?;
        let have = ForgeDb::load(&paths.forge_db()).ok();
        if have.as_ref().is_some_and(|db| db.schema == FORGE_SCHEMA && db.source == latest.tag_name) {
            progress.note(&format!("  Forge's card scripts are already {}'s", latest.tag_name));
            return Ok(());
        }
        // The release is the one kept, and only the database is of an older schema.
        let kept = raw_path.exists().then(|| read_json_gz::<ForgeRaw>(&raw_path).ok()).flatten();
        match kept.filter(|raw| raw.source == latest.tag_name) {
            Some(raw) => raw,
            None => {
                progress.step(&format!(
                    "downloading Forge {}'s card scripts (~250 MB with the rest of its source)",
                    latest.tag_name
                ));
                let files = http.stream(&forge_tarball(&latest.tag_name), |r| read_tarball(r))?;
                let raw = ForgeRaw { source: latest.tag_name, files };
                write_json_gz(&raw_path, &raw)?;
                raw
            }
        }
    };
    progress.step("building Forge card script database");
    let db = ForgeDb::from_raw(raw);
    db.save(&paths.forge_db())?;
    progress.note(&format!("  {} card scripts from {}", db.len(), db.source));
    Ok(())
}

/// Load both databases, syncing whatever is missing or from an older schema first. Each file is
/// read once, both at the same time: a current one is kept as loaded, not loaded again after the
/// schema check.
pub fn ensure_databases(http: &Http, paths: &Paths, progress: &dyn Progress) -> Result<(CardDb, ComboDb)> {
    progress.step("loading card and combo databases");
    // Side by side: the combo database takes the longer, and nothing needs one before the other.
    let (cards, combos) = std::thread::scope(|s| {
        let combos = s.spawn(|| current(&paths.combos_db(), ComboDb::load, |d| d.schema == COMBOS_SCHEMA));
        let cards = current(&paths.cards_db(), CardDb::load, |d| d.schema == CARDS_SCHEMA);
        (cards, combos.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
    });
    if cards.is_none() || combos.is_none() {
        let missing_raw = (cards.is_none() && !(paths.scryfall_raw().exists() && paths.scryfall_tags_raw().exists()))
            || (combos.is_none() && !paths.spellbook_raw().exists());
        if http.offline() && missing_raw {
            bail!("no local card/combo database yet; run `edh-scout sync` once while online");
        }
        progress.note("local databases missing or outdated — building them first");
        let what = SyncWhat { cards: cards.is_none(), combos: combos.is_none(), forge: false, use_existing_raw: true };
        sync(http, paths, what, progress)?;
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
        progress.note(&format!(
            "note: card data is {} days old — `edh-scout sync` refreshes prices and Game Changers",
            cards.age_days()
        ));
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
