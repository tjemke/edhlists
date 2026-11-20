//! The deck database: every decklist edh-scout has fetched, parsed, in one SQLite file.
//!
//! Each row keeps the parsed deck (commanders, the 99, EDHREC's price, salt, tags and links) and,
//! gzipped, EDHREC's own deck object from the page it came from, so a parser fix can re-read
//! every deck without fetching its page again. Reading a commander's thousands of stored decks is
//! one query instead of thousands of 270 KB pages to parse. Each deck's score is kept too, with
//! what it was scored against, so decks are only scored again when that changes. The decks the
//! user marked as favorites are kept in a table of their own, so storing a deck again (a re-read)
//! never drops the mark.
//!
//! One writer at a time, readers alongside it (WAL); two runs at once wait for each other briefly
//! rather than fail. Every write is one transaction, which takes the write lock as it begins (so
//! it waits for another run's write rather than failing partway), and changes rows, never a copy
//! of them read earlier, so runs writing at once never lose each other's work. Several runs may
//! also create or upgrade the database at once: whichever is first does it, the others wait and
//! find it done. The connection sits behind a mutex, so one [`DeckDb`] can be shared between
//! threads.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use edh_data::deck::{Deck, DeckMeta};
use edh_data::edhrec::deck_url;
use edh_data::text::commander_slug;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};

mod store;
pub use store::{DeckStore, Imported, Retired};

/// The schema this code reads and writes (`PRAGMA user_version`).
pub const SCHEMA: i64 = 3;

/// Each step brings the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE deck (
        id          TEXT PRIMARY KEY,   -- EDHREC urlhash
        slug        TEXT NOT NULL,      -- commander_slug(commanders)
        commanders  TEXT NOT NULL,      -- JSON array
        cards       TEXT NOT NULL,      -- JSON {name: quantity}, the 99
        savedate    TEXT,
        price       REAL,
        salt        REAL,
        source_url  TEXT,
        tags        TEXT NOT NULL,      -- JSON array
        source      BLOB NOT NULL,      -- EDHREC's deck object from the page, gzipped JSON
        parser      INTEGER NOT NULL    -- version of the parser that filled the columns
    ) WITHOUT ROWID;
    CREATE INDEX deck_by_slug ON deck(slug);
",
    "
    CREATE TABLE score (
        id          TEXT PRIMARY KEY,   -- deck id
        epoch       INTEGER NOT NULL,   -- the scorer and card/combo databases it was scored with
        synergy_src INTEGER NOT NULL,   -- the commander data synergy was scored against
        power       REAL NOT NULL,
        bracket     INTEGER NOT NULL,
        floor       INTEGER NOT NULL,
        structure   REAL NOT NULL,
        synergy     REAL
    ) WITHOUT ROWID;
",
    "
    CREATE TABLE favorite (
        id          TEXT PRIMARY KEY,   -- deck id
        added       INTEGER NOT NULL    -- when it was marked, in seconds since 1970
    ) WITHOUT ROWID;
",
];

/// How long a run waits for another one's write before giving up.
const BUSY: Duration = Duration::from_secs(10);

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked))
}

/// A deck's score as the database keeps it, with what it was scored against: an `epoch` for the
/// scorer and the card and combo databases, and a fingerprint of the commander data synergy was
/// measured with. A score is only good while both still match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredScore {
    pub epoch: i64,
    pub synergy_src: i64,
    pub power: f64,
    pub bracket: u8,
    pub floor: u8,
    pub structure: f64,
    pub synergy: Option<f64>,
}

pub struct DeckDb {
    conn: Mutex<Connection>,
    /// Every stored id, so [`DeckDb::contains`] (asked for each row of a deck table, tens of
    /// thousands at a time) needs no query.
    ids: Mutex<HashSet<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn gzip(text: &str) -> Result<Vec<u8>> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(text.as_bytes())?;
    Ok(e.finish()?)
}

fn gunzip(bytes: &[u8]) -> Result<String> {
    let mut out = String::new();
    GzDecoder::new(bytes).read_to_string(&mut out)?;
    Ok(out)
}

/// A deck as one row's values.
struct Row {
    slug: String,
    commanders: String,
    cards: String,
    tags: String,
    source: Vec<u8>,
}

impl Row {
    fn of(deck: &Deck, source: &str) -> Result<Self> {
        Ok(Self {
            slug: commander_slug(&deck.commanders),
            commanders: serde_json::to_string(&deck.commanders)?,
            cards: serde_json::to_string(&deck.cards)?,
            tags: serde_json::to_string(&deck.meta.tags)?,
            source: gzip(source)?,
        })
    }
}

const COLUMNS: &str = "d.id, d.commanders, d.cards, d.savedate, d.price, d.salt, d.source_url, d.tags";

/// A deck from the columns in [`COLUMNS`] order, starting at `at`. The outer error is SQLite's,
/// the inner one a row whose JSON doesn't parse.
fn deck_at(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Result<Deck>> {
    let id: String = r.get(at)?;
    let commanders: String = r.get(at + 1)?;
    let cards: String = r.get(at + 2)?;
    let meta = DeckMeta {
        edhrec_url: Some(deck_url(&id)),
        savedate: r.get(at + 3)?,
        price: r.get(at + 4)?,
        salt: r.get(at + 5)?,
        source_url: r.get(at + 6)?,
        tags: Vec::new(),
        declared_bracket: None,
    };
    let tags: String = r.get(at + 7)?;
    Ok(decode(id, &commanders, &cards, &tags, meta))
}

fn decode(id: String, commanders: &str, cards: &str, tags: &str, mut meta: DeckMeta) -> Result<Deck> {
    let context = || format!("stored deck {id} doesn't parse");
    meta.tags = serde_json::from_str(tags).with_context(context)?;
    Ok(Deck {
        commanders: serde_json::from_str(commanders).with_context(context)?,
        cards: serde_json::from_str::<BTreeMap<String, u32>>(cards).with_context(context)?,
        meta,
        id,
    })
}

/// The database's schema version, refusing one newer than this code reads.
fn schema(conn: &Connection) -> Result<i64> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA {
        bail!("the deck database was written by a newer edh-scout (schema {version}, this one reads {SCHEMA})");
    }
    Ok(version)
}

/// Switch the database to WAL (which sticks to the file). SQLite doesn't wait out a busy database
/// for this, as it does for other statements: when several runs create the database at once, try
/// again until the others are done.
fn use_wal(conn: &Connection) -> Result<()> {
    let deadline = Instant::now() + BUSY;
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Err(e) if is_busy(&e) && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            r => return Ok(r?),
        }
    }
}

impl DeckDb {
    /// Open (or create) the database at `path`, bringing its schema up to date.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::with(conn).with_context(|| format!("in {}", path.display()))
    }

    fn with(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY)?;
        use_wal(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        if schema(&conn)? != SCHEMA {
            // Another run may be bringing it up to date too: with the write lock held, look again.
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let version = schema(&tx)?;
            for (i, step) in MIGRATIONS.iter().enumerate().skip(version as usize) {
                tx.execute_batch(step)?;
                tx.pragma_update(None, "user_version", i as i64 + 1)?;
            }
            tx.commit()?;
        }
        let ids = {
            let mut q = conn.prepare("SELECT id FROM deck")?;
            q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<HashSet<String>>>()?
        };
        Ok(Self { conn: Mutex::new(conn), ids: Mutex::new(ids) })
    }

    pub fn contains(&self, id: &str) -> bool {
        lock(&self.ids).contains(id)
    }

    pub fn len(&self) -> usize {
        lock(&self.ids).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every stored id.
    pub fn ids(&self) -> HashSet<String> {
        lock(&self.ids).clone()
    }

    /// Store a deck, with EDHREC's deck object it was parsed from (replacing any earlier row).
    /// `parser` is the version of the parser that read it.
    pub fn put(&self, deck: &Deck, source: &str, parser: u32) -> Result<()> {
        self.put_many(&[(deck, source)], parser)
    }

    /// [`DeckDb::put`] for many decks, in one transaction.
    pub fn put_many(&self, decks: &[(&Deck, &str)], parser: u32) -> Result<()> {
        let rows: Vec<Row> = decks.iter().map(|(d, s)| Row::of(d, s)).collect::<Result<_>>()?;
        let mut conn = lock(&self.conn);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO deck
                 (id, slug, commanders, cards, savedate, price, salt, source_url, tags, source, parser)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            // A deck read again may score differently.
            let mut forget = tx.prepare_cached("DELETE FROM score WHERE id = ?1")?;
            for ((deck, _), row) in decks.iter().zip(&rows) {
                forget.execute([&deck.id])?;
                let m = &deck.meta;
                ins.execute(params![
                    deck.id,
                    row.slug,
                    row.commanders,
                    row.cards,
                    m.savedate,
                    m.price,
                    m.salt,
                    m.source_url,
                    row.tags,
                    row.source,
                    parser
                ])?;
            }
        }
        tx.commit()?;
        drop(conn);
        lock(&self.ids).extend(decks.iter().map(|(d, _)| d.id.clone()));
        Ok(())
    }

    /// One stored deck.
    pub fn get(&self, id: &str) -> Result<Option<Deck>> {
        let conn = lock(&self.conn);
        let mut q = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM deck d WHERE d.id = ?1"))?;
        q.query_row([id], |r| deck_at(r, 0)).optional()?.transpose()
    }

    /// The stored decks with these ids, in the same order; `None` where a deck isn't stored.
    pub fn get_many(&self, ids: &[&str]) -> Result<Vec<Option<Deck>>> {
        let rows =
            self.by_ids(ids, &format!("SELECT w.pos, {COLUMNS} FROM temp.wanted w JOIN deck d ON d.id = w.id"), |r| {
                deck_at(r, 1)
            })?;
        rows.into_iter().map(|d| d.transpose()).collect()
    }

    /// The stored scores of these decks, in the same order; `None` where a deck has none.
    pub fn scores(&self, ids: &[&str]) -> Result<Vec<Option<StoredScore>>> {
        self.by_ids(
            ids,
            "SELECT w.pos, s.epoch, s.synergy_src, s.power, s.bracket, s.floor, s.structure, s.synergy
             FROM temp.wanted w JOIN score s ON s.id = w.id",
            |r| {
                Ok(StoredScore {
                    epoch: r.get(1)?,
                    synergy_src: r.get(2)?,
                    power: r.get(3)?,
                    bracket: r.get(4)?,
                    floor: r.get(5)?,
                    structure: r.get(6)?,
                    synergy: r.get(7)?,
                })
            },
        )
    }

    /// Keep these decks' scores (replacing earlier ones), in one transaction.
    pub fn put_scores(&self, scores: &[(&str, StoredScore)]) -> Result<()> {
        let mut conn = lock(&self.conn);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO score (id, epoch, synergy_src, power, bracket, floor, structure, synergy)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (id, s) in scores {
                ins.execute(params![id, s.epoch, s.synergy_src, s.power, s.bracket, s.floor, s.structure, s.synergy])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Run `select` (which reads `temp.wanted (pos, id)` and returns `pos` first) for these ids,
    /// and map its rows into the positions asked for; `None` where nothing came back.
    fn by_ids<T>(
        &self,
        ids: &[&str],
        select: &str,
        mut row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<Option<T>>> {
        let mut out: Vec<Option<T>> = (0..ids.len()).map(|_| None).collect();
        let mut conn = lock(&self.conn);
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS wanted (pos INTEGER PRIMARY KEY, id TEXT NOT NULL);
             DELETE FROM temp.wanted;",
        )?;
        {
            let mut ins = tx.prepare_cached("INSERT INTO temp.wanted (pos, id) VALUES (?1, ?2)")?;
            for (pos, id) in ids.iter().enumerate() {
                ins.execute(params![pos as i64, id])?;
            }
            let mut q = tx.prepare(select)?;
            let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, row(r)?)))?;
            for r in rows {
                let (pos, value) = r?;
                out[pos as usize] = Some(value);
            }
        }
        tx.execute_batch("DELETE FROM temp.wanted;")?;
        tx.commit()?;
        Ok(out)
    }

    /// EDHREC's deck object a stored deck was parsed from, as JSON.
    pub fn source(&self, id: &str) -> Result<Option<String>> {
        let conn = lock(&self.conn);
        let bytes: Option<Vec<u8>> =
            conn.query_row("SELECT source FROM deck WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        bytes.map(|b| gunzip(&b)).transpose()
    }

    /// The ids of the decks marked as favorites, oldest mark first.
    pub fn favorites(&self) -> Result<Vec<String>> {
        let conn = lock(&self.conn);
        let mut q = conn.prepare_cached("SELECT id FROM favorite ORDER BY added, id")?;
        Ok(q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Mark a deck as a favorite, or clear the mark. Marking it again keeps when it was first
    /// marked.
    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        let conn = lock(&self.conn);
        if favorite {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            conn.execute("INSERT OR IGNORE INTO favorite (id, added) VALUES (?1, ?2)", params![id, now as i64])?;
        } else {
            conn.execute("DELETE FROM favorite WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    /// Ids of the decks a parser older than `parser` filled in.
    pub fn parsed_before(&self, parser: u32) -> Result<Vec<String>> {
        let conn = lock(&self.conn);
        let mut q = conn.prepare("SELECT id FROM deck WHERE parser < ?1")?;
        Ok(q.query_map([parser], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(id: &str, commander: &str, card: &str) -> Deck {
        Deck {
            id: id.into(),
            commanders: vec![commander.into()],
            cards: [(card.to_string(), 1), ("Sol Ring".to_string(), 1)].into(),
            meta: DeckMeta {
                edhrec_url: Some(deck_url(id)),
                source_url: Some(format!("https://archidekt.com/decks/{id}")),
                savedate: Some("2026-11-01".into()),
                price: Some(123.5),
                salt: Some(30.25),
                tags: vec!["Legends".into()],
                declared_bracket: None,
            },
        }
    }

    #[test]
    fn decks_round_trip_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("decks.sqlite");
        let a = deck("aaa", "Jodah, the Unifier", "Bonesplitter");
        let b = deck("bbb", "Krenko, Mob Boss", "Goblin Bombardment");
        {
            let db = DeckDb::open(&path).unwrap();
            assert!(db.is_empty());
            db.put_many(&[(&a, r#"{"recs":[1]}"#), (&b, r#"{"recs":[2]}"#)], 1).unwrap();
            assert!(db.contains("aaa") && !db.contains("ccc"));
        }
        let db = DeckDb::open(&path).unwrap();
        assert_eq!(db.len(), 2);
        let got = db.get("aaa").unwrap().unwrap();
        assert_eq!((got.commanders.clone(), got.cards.clone()), (a.commanders.clone(), a.cards.clone()));
        assert_eq!((got.meta.price, got.meta.salt, &got.meta.tags), (Some(123.5), Some(30.25), &a.meta.tags));
        assert_eq!(got.meta.edhrec_url.as_deref(), Some(deck_url("aaa").as_str()));
        assert!(db.get("zzz").unwrap().is_none());
        assert_eq!(db.source("bbb").unwrap().as_deref(), Some(r#"{"recs":[2]}"#));
        // In the order asked for, with a gap for a deck that isn't stored.
        let many = db.get_many(&["bbb", "nope", "aaa"]).unwrap();
        let ids: Vec<Option<&str>> = many.iter().map(|d| d.as_ref().map(|d| d.id.as_str())).collect();
        assert_eq!(ids, vec![Some("bbb"), None, Some("aaa")]);
        // Asking again works the same (the temporary table is emptied in between).
        assert_eq!(db.get_many(&["aaa"]).unwrap()[0].as_ref().unwrap().id, "aaa");
    }

    #[test]
    fn replacing_and_parser_versions() {
        let dir = tempfile::tempdir().unwrap();
        let db = DeckDb::open(&dir.path().join("d.sqlite")).unwrap();
        db.put(&deck("aaa", "Jodah, the Unifier", "Bonesplitter"), "{}", 1).unwrap();
        db.put(&deck("bbb", "Jodah, the Unifier", "Bonesplitter"), "{}", 2).unwrap();
        db.put(&deck("aaa", "Jodah, the Unifier", "Swords to Plowshares"), "{}", 2).unwrap();
        assert_eq!(db.len(), 2);
        assert!(db.get("aaa").unwrap().unwrap().cards.contains_key("Swords to Plowshares"));
        assert!(db.parsed_before(2).unwrap().is_empty());
        db.put(&deck("ccc", "Jodah, the Unifier", "Bonesplitter"), "{}", 1).unwrap();
        assert_eq!(db.parsed_before(2).unwrap(), vec!["ccc".to_string()]);
    }

    #[test]
    fn scores_are_kept_and_forgotten_when_the_deck_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = DeckDb::open(&dir.path().join("d.sqlite")).unwrap();
        db.put_many(
            &[
                (&deck("aaa", "Jodah, the Unifier", "Bonesplitter"), "{}"),
                (&deck("bbb", "Jodah, the Unifier", "Ponder"), "{}"),
            ],
            1,
        )
        .unwrap();
        let s =
            StoredScore { epoch: 7, synergy_src: 3, power: 6.25, bracket: 3, floor: 2, structure: 0.9, synergy: None };
        db.put_scores(&[("aaa", s), ("bbb", StoredScore { synergy: Some(0.1), ..s })]).unwrap();
        let got = db.scores(&["bbb", "zzz", "aaa"]).unwrap();
        assert_eq!(got, vec![Some(StoredScore { synergy: Some(0.1), ..s }), None, Some(s)]);
        // Storing a deck again (a re-read) drops its score.
        db.put(&deck("aaa", "Jodah, the Unifier", "Swords to Plowshares"), "{}", 1).unwrap();
        assert_eq!(db.scores(&["aaa", "bbb"]).unwrap()[0], None);
    }

    #[test]
    fn favorites_persist_and_survive_a_deck_being_stored_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.sqlite");
        {
            let db = DeckDb::open(&path).unwrap();
            assert!(db.favorites().unwrap().is_empty());
            db.put(&deck("aaa", "Jodah, the Unifier", "Bonesplitter"), "{}", 1).unwrap();
            db.set_favorite("bbb", true).unwrap();
            db.set_favorite("aaa", true).unwrap();
            // Marking twice is one mark; clearing one that isn't marked is fine.
            db.set_favorite("aaa", true).unwrap();
            db.set_favorite("zzz", false).unwrap();
        }
        let db = DeckDb::open(&path).unwrap();
        let mut got = db.favorites().unwrap();
        got.sort();
        assert_eq!(got, vec!["aaa", "bbb"]);
        // A re-read deck keeps its mark.
        db.put(&deck("aaa", "Jodah, the Unifier", "Swords to Plowshares"), "{}", 2).unwrap();
        db.set_favorite("bbb", false).unwrap();
        assert_eq!(db.favorites().unwrap(), vec!["aaa"]);
    }

    #[test]
    fn an_older_database_gains_favorites_and_keeps_its_decks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.sqlite");
        {
            // A database as schema 2 left it.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch(MIGRATIONS[1]).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            let db = DeckDb { conn: Mutex::new(conn), ids: Mutex::new(HashSet::new()) };
            db.put(&deck("aaa", "Jodah, the Unifier", "Bonesplitter"), "{}", 1).unwrap();
        }
        let db = DeckDb::open(&path).unwrap();
        assert!(db.contains("aaa"));
        db.set_favorite("aaa", true).unwrap();
        assert_eq!(db.favorites().unwrap(), vec!["aaa"]);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), SCHEMA);
    }

    /// Open the database at `path` from `runs` threads at once, each with its own connection
    /// (as separate edh-scout processes would have), and run `each` on each.
    fn at_once(path: &Path, runs: usize, each: impl Fn(usize, DeckDb) + Sync) {
        let start = std::sync::Barrier::new(runs);
        std::thread::scope(|s| {
            for i in 0..runs {
                let (start, each) = (&start, &each);
                s.spawn(move || {
                    start.wait();
                    let db = DeckDb::open(path).unwrap_or_else(|e| panic!("run {i} couldn't open the database: {e:#}"));
                    each(i, db);
                });
            }
        });
    }

    #[test]
    fn several_runs_can_create_the_database_at_once() {
        for round in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("d.sqlite");
            at_once(&path, 6, |_, db| assert!(db.is_empty(), "round {round}: a new database has decks"));
            let conn = Connection::open(&path).unwrap();
            let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
            assert_eq!(version, SCHEMA, "round {round}: the schema wasn't brought up to date once");
        }
    }

    #[test]
    fn several_runs_writing_at_once_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.sqlite");
        DeckDb::open(&path).unwrap();
        let (runs, decks) = (4, 60);
        let s =
            StoredScore { epoch: 1, synergy_src: 1, power: 5.0, bracket: 3, floor: 2, structure: 0.5, synergy: None };
        at_once(&path, runs, |run, db| {
            for n in 0..decks {
                let id = format!("r{run}d{n}");
                let d = deck(&id, "Jodah, the Unifier", "Bonesplitter");
                if n % 2 == 0 {
                    db.put(&d, "{}", 1).unwrap();
                } else {
                    db.put_many(&[(&d, "{}")], 1).unwrap();
                }
                db.put_scores(&[(id.as_str(), StoredScore { power: n as f64, ..s })]).unwrap();
                if n % 3 == 0 {
                    db.set_favorite(&id, true).unwrap();
                }
                // Reads from each run go on alongside the others' writes.
                assert_eq!(db.get_many(&[id.as_str()]).unwrap()[0].as_ref().map(|d| d.id.as_str()), Some(id.as_str()));
            }
        });
        let db = DeckDb::open(&path).unwrap();
        assert_eq!(db.len(), runs * decks, "decks were lost");
        let ids: Vec<String> = (0..runs).flat_map(|r| (0..decks).map(move |n| format!("r{r}d{n}"))).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let lost = db.scores(&ids).unwrap().iter().filter(|s| s.is_none()).count();
        assert_eq!(lost, 0, "{lost} scores were lost");
        assert_eq!(db.favorites().unwrap().len(), runs * decks.div_ceil(3), "favorites were lost");
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.sqlite");
        DeckDb::open(&path).unwrap();
        Connection::open(&path).unwrap().pragma_update(None, "user_version", SCHEMA + 1).unwrap();
        let err = DeckDb::open(&path).err().unwrap();
        assert!(format!("{err:#}").contains("newer edh-scout"), "{err:#}");
    }
}
