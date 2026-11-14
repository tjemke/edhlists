//! Decks by EDHREC id: from the deck database, or fetched (one request), parsed and stored.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use edh_data::deck::Deck;
use edh_data::edhrec::{DECK_PARSER_VERSION, deck_from_source, fetch_deck_page, is_deck_id, read_deck_page};
use edh_data::http::Http;
use edh_data::progress::Progress;
use edh_data::storage::{atomic_write, open_maybe_gz};
use rayon::prelude::*;

use crate::DeckDb;

/// Pages parsed per batch while importing; each batch is stored in one transaction.
const IMPORT_BATCH: usize = 512;

/// The decklists we have fetched. Decks are snapshots: once stored, a deck never costs a request
/// again. Each fetched page is also kept, gzipped, in `pages`: a cache, since the database keeps
/// what a parser needs to read the deck again.
#[derive(Clone)]
pub struct DeckStore {
    db: Arc<DeckDb>,
    pages: PathBuf,
}

/// What [`DeckStore::retire_old_pages`] did.
#[derive(Debug, PartialEq)]
pub enum Retired {
    Nothing,
    Moved,
    /// Some pages couldn't move (another filesystem, or already in the cache): this many bytes.
    LeftInPlace {
        bytes: u64,
    },
}

/// What [`DeckStore::import`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub decks: usize,
    /// Pages that couldn't be read, and the first reason.
    pub failed: usize,
    pub first_error: Option<String>,
}

impl DeckStore {
    pub fn open(db: &Path, pages: PathBuf) -> Result<Self> {
        Ok(Self { db: Arc::new(DeckDb::open(db)?), pages })
    }

    pub fn db(&self) -> &DeckDb {
        &self.db
    }

    pub fn contains(&self, urlhash: &str) -> bool {
        self.db.contains(urlhash)
    }

    /// The deck from the database, or fetched (one request), stored, and its page kept.
    pub fn fetch(&self, http: &Http, urlhash: &str) -> Result<Deck> {
        if !is_deck_id(urlhash) {
            return Err(anyhow!("'{urlhash}' is not an EDHREC deck id"));
        }
        if let Some(deck) = self.db.get(urlhash)? {
            return Ok(deck);
        }
        let html = fetch_deck_page(http, urlhash)?;
        // Parse before storing, so a page we can't read is never kept as "fetched".
        let page = read_deck_page(&html, urlhash)?;
        self.db.put(&page.deck, &page.source, DECK_PARSER_VERSION)?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(html.as_bytes())?;
        atomic_write(&self.pages.join(format!("{urlhash}.html.gz")), &gz.finish()?)?;
        Ok(page.deck)
    }

    /// Where fetched pages are kept.
    pub fn pages(&self) -> &Path {
        &self.pages
    }

    /// Move a directory of deck pages already read into the database (where they were kept
    /// before it existed) into the page cache. A rename when both are on one filesystem;
    /// otherwise the pages stay where they are.
    pub fn retire_old_pages(&self, old: &Path) -> Result<Retired> {
        if !old.exists() {
            return Ok(Retired::Nothing);
        }
        if let Some(parent) = self.pages.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !self.pages.exists() && std::fs::rename(old, &self.pages).is_ok() {
            return Ok(Retired::Moved);
        }
        // The cache already has pages (or the whole directory couldn't move): one by one.
        std::fs::create_dir_all(&self.pages)?;
        let mut left = 0u64;
        for entry in std::fs::read_dir(old)?.filter_map(|e| e.ok()) {
            let dest = self.pages.join(entry.file_name());
            if dest.exists() || std::fs::rename(entry.path(), &dest).is_err() {
                left += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        if left == 0 && std::fs::remove_dir(old).is_ok() {
            return Ok(Retired::Moved);
        }
        Ok(Retired::LeftInPlace { bytes: left })
    }

    /// The stored decks with these ids, in order; `None` where a deck isn't stored.
    pub fn get_many(&self, ids: &[&str]) -> Result<Vec<Option<Deck>>> {
        self.db.get_many(ids)
    }

    /// Store the decks on the pages in `dir` (`<urlhash>.html`, or `.html.gz`) that the database
    /// doesn't have yet, parsing them on the thread pool. This is how decks fetched before the
    /// database existed get into it, and how it's rebuilt if deleted.
    pub fn import(&self, dir: &Path, progress: &dyn Progress) -> Result<Imported> {
        let mut done = Imported::default();
        let Ok(entries) = std::fs::read_dir(dir) else { return Ok(done) };
        let todo: Vec<(String, PathBuf)> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?;
                let id = name.strip_suffix(".html.gz").or_else(|| name.strip_suffix(".html"))?.to_string();
                (is_deck_id(&id) && !self.db.contains(&id)).then_some((id, p))
            })
            .collect();
        for (n, batch) in todo.chunks(IMPORT_BATCH).enumerate() {
            progress.step(&format!(
                "reading stored deck pages into the deck database: {} of {}",
                n * IMPORT_BATCH,
                todo.len()
            ));
            let read: Vec<Result<(Deck, String)>> = batch
                .par_iter()
                .map(|(id, path)| {
                    let mut html = String::new();
                    open_maybe_gz(path)?
                        .read_to_string(&mut html)
                        .with_context(|| format!("reading {}", path.display()))?;
                    let page = read_deck_page(&html, id)?;
                    Ok((page.deck, page.source))
                })
                .collect();
            let mut ok = Vec::with_capacity(read.len());
            for r in read {
                match r {
                    Ok(d) => ok.push(d),
                    Err(e) => {
                        done.failed += 1;
                        done.first_error.get_or_insert_with(|| format!("{e:#}"));
                    }
                }
            }
            let rows: Vec<(&Deck, &str)> = ok.iter().map(|(d, s)| (d, s.as_str())).collect();
            self.db.put_many(&rows, DECK_PARSER_VERSION)?;
            done.decks += rows.len();
        }
        Ok(done)
    }

    /// Read again, from the deck object kept with each, the decks an older parser filled in.
    pub fn reparse(&self, progress: &dyn Progress) -> Result<usize> {
        let ids = self.db.parsed_before(DECK_PARSER_VERSION)?;
        if !ids.is_empty() {
            progress.step(&format!("re-reading {} stored decks with the new parser", ids.len()));
        }
        for batch in ids.chunks(IMPORT_BATCH) {
            let sources: Vec<(String, String)> =
                batch.iter().filter_map(|id| Some((id.clone(), self.db.source(id).ok()??))).collect();
            let decks: Vec<(Deck, String)> = sources
                .into_par_iter()
                .filter_map(|(id, source)| deck_from_source(&source, &id).ok().map(|d| (d, source)))
                .collect();
            let rows: Vec<(&Deck, &str)> = decks.iter().map(|(d, s)| (d, s.as_str())).collect();
            self.db.put_many(&rows, DECK_PARSER_VERSION)?;
        }
        Ok(ids.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edh_data::progress::Silent;

    const PAGES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../edh-data/tests/data/edhrec/");
    const JODAH: &str = "8K2SZWMbz80NrhCJmt9DsA";
    const PARTNER: &str = "qqdG7wTXY32jPgVRdP-x5g";

    fn page(name: &str) -> String {
        std::fs::read_to_string(format!("{PAGES}{name}")).unwrap()
    }

    #[test]
    fn stored_decks_never_cost_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeckStore::open(&dir.path().join("decks.sqlite"), dir.path().join("pages")).unwrap();
        let offline = Http::new(dir.path().join("http"), true);
        let err = store.fetch(&offline, JODAH).unwrap_err();
        assert!(err.downcast_ref::<edh_data::http::OfflineMiss>().is_some());
        assert!(store.fetch(&offline, "../etc/passwd").is_err());
        let p = read_deck_page(&page("deckpreview_jodah.html"), JODAH).unwrap();
        store.db().put(&p.deck, &p.source, DECK_PARSER_VERSION).unwrap();
        assert!(store.contains(JODAH));
        assert_eq!(store.fetch(&offline, JODAH).unwrap().commanders, vec!["Jodah, the Unifier"]);
        assert!(offline.request_counts().is_empty());
    }

    #[test]
    fn importing_a_directory_of_pages() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("decks");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(format!("{JODAH}.html")), page("deckpreview_jodah.html")).unwrap();
        // A gzipped page (as newer versions keep them) and a page that isn't one.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, page("deckpreview_partner.html").as_bytes()).unwrap();
        std::fs::write(old.join(format!("{PARTNER}.html.gz")), gz.finish().unwrap()).unwrap();
        std::fs::write(old.join("broken.html"), "<html>nothing</html>").unwrap();
        std::fs::write(old.join("notes.txt"), "not a page").unwrap();
        let store = DeckStore::open(&dir.path().join("decks.sqlite"), dir.path().join("pages")).unwrap();
        let done = store.import(&old, &Silent).unwrap();
        assert_eq!((done.decks, done.failed), (2, 1));
        assert!(done.first_error.unwrap().contains("format may have changed"));
        assert!(store.contains(JODAH) && store.contains(PARTNER));
        let both = store.get_many(&[PARTNER, JODAH]).unwrap();
        assert_eq!(both[0].as_ref().unwrap().commanders, vec!["Kraum, Ludevic's Opus", "Tymna the Weaver"]);
        assert_eq!(both[1].as_ref().unwrap().card_count(), 100);
        // A second import finds nothing new.
        assert_eq!(store.import(&old, &Silent).unwrap().decks, 0);
        // A missing directory is nothing to import.
        assert_eq!(store.import(&dir.path().join("nope"), &Silent).unwrap(), Imported::default());
    }

    #[test]
    fn old_pages_move_into_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let (old, cache) = (dir.path().join("state").join("decks"), dir.path().join("cache").join("pages"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("aaa.html"), "a").unwrap();
        let store = DeckStore::open(&dir.path().join("decks.sqlite"), cache.clone()).unwrap();
        assert_eq!(store.retire_old_pages(&old).unwrap(), Retired::Moved);
        assert!(!old.exists() && cache.join("aaa.html").exists());
        assert_eq!(store.retire_old_pages(&old).unwrap(), Retired::Nothing);
        // Into a cache that already has pages: one by one; a page already there stays behind.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("bbb.html"), "b").unwrap();
        std::fs::write(old.join("aaa.html"), "a again").unwrap();
        assert_eq!(store.retire_old_pages(&old).unwrap(), Retired::LeftInPlace { bytes: 7 });
        assert!(cache.join("bbb.html").exists() && old.join("aaa.html").exists());
    }

    #[test]
    fn decks_from_an_older_parser_are_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeckStore::open(&dir.path().join("decks.sqlite"), dir.path().join("pages")).unwrap();
        let p = read_deck_page(&page("deckpreview_jodah.html"), JODAH).unwrap();
        let mut stale = p.deck.clone();
        stale.cards.clear();
        store.db().put(&stale, &p.source, DECK_PARSER_VERSION - 1).unwrap();
        assert_eq!(store.reparse(&Silent).unwrap(), 1);
        assert_eq!(store.db().get(JODAH).unwrap().unwrap().card_count(), 100);
        assert_eq!(store.reparse(&Silent).unwrap(), 0);
    }
}
