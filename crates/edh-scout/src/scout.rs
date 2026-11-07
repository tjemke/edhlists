//! The `scout` pipeline: deck table(s) -> candidate selection -> stored decks right away, new decks
//! fetched in the background -> analysis as they arrive.
//!
//! Two shapes: one commander ([`prepare`] + [`Fetcher::spawn`]), or a search across commanders
//! picked by card-type filters ([`Fetcher::spawn_search`]), where the deck tables themselves load
//! in the background too.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use anyhow::Result;
use edh_data::cards::CardDb;
use edh_data::combos::ComboDb;
use edh_data::deck::Deck;
use edh_data::edhrec::{self, CommanderPage, DeckStore, EdhrecCombo, ListedCommander, TableRow};
use edh_data::http::{HostBlocked, Http, OfflineMiss};
use edh_data::text::commander_slug;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::analysis::{Analysis, Context, analyze};
use crate::select::{Ceiling, SelectOptions, allocate_weighted, select};

/// Per-commander side data: synergy and combo votes. Each piece is optional — a failure costs
/// that signal, not the run.
#[derive(Default)]
pub struct CommanderSources {
    pub page: Option<CommanderPage>,
    pub combos: Vec<EdhrecCombo>,
}

fn warn(http: &Http, what: &str, e: anyhow::Error) {
    if e.downcast_ref::<OfflineMiss>().is_none() {
        http.notice(format!("note: couldn't load {what} ({e:#}); continuing without it"));
    }
}

/// Synergy and combo votes for one commander (2 requests, cached for a week).
pub fn load_commander_sources(http: &Http, slug: &str) -> CommanderSources {
    let mut s = CommanderSources::default();
    match edhrec::commander_page(http, slug) {
        Ok(p) => s.page = Some(p),
        Err(e) => warn(http, "EDHREC synergy data", e),
    }
    match edhrec::combos_page(http, slug) {
        Ok(c) => s.combos = c,
        Err(e) => warn(http, "EDHREC combo votes", e),
    }
    s
}

/// EDHREC's top-salt list (1 request, cached for two weeks).
pub fn load_salt(http: &Http) -> HashMap<String, f64> {
    edhrec::top_salt(http).unwrap_or_else(|e| {
        warn(http, "EDHREC's salt list", e);
        HashMap::new()
    })
}

#[derive(Clone, Debug, Default)]
pub struct PoolStats {
    pub listed: usize,
    pub eligible: usize,
    /// Decks read from the deck store (no request).
    pub cached: usize,
    /// Decks queued for fetching this run.
    pub queued: usize,
    pub fetched: usize,
    pub failed: usize,
    /// Searching across commanders: deck tables to scan, and scanned so far.
    pub commanders_total: usize,
    pub commanders_scanned: usize,
    pub ceiling: Option<Ceiling>,
    /// Why fetching stopped before the budget was used up, if it did.
    pub stopped: Option<String>,
}

impl PoolStats {
    /// "34431 on EDHREC, 23804 eligible, 57 reused, 12 fetched"
    pub fn summary(&self) -> String {
        let mut s = String::new();
        if self.commanders_total > 0 {
            s += &format!("{} commanders, ", self.commanders_scanned);
        }
        s += &format!("{} on EDHREC, {} eligible", self.listed, self.eligible);
        if self.cached > 0 {
            s += &format!(", {} reused", self.cached);
        }
        if self.fetched > 0 {
            s += &format!(", {} fetched", self.fetched);
        }
        if self.failed > 0 {
            s += &format!(", {} failed", self.failed);
        }
        s
    }

    /// Still scanning deck tables (searching across commanders)?
    pub fn scanning(&self) -> bool {
        self.commanders_scanned < self.commanders_total
    }
}

pub struct ScoutOptions {
    pub commanders: Vec<String>,
    pub slug: String,
    pub select: SelectOptions,
}

/// The quick part of a single-commander scout: everything except fetching new decks.
pub struct Prepared {
    /// Decks already in the deck store, ready to analyse.
    pub decks: Vec<Deck>,
    pub sources: CommanderSources,
    pub stats: PoolStats,
    /// Decks to fetch, in priority order (empty when offline).
    pub to_fetch: Vec<TableRow>,
}

/// Attach what the deck table knows to a fetched deck.
fn finish_deck(mut deck: Deck, row: &TableRow, commanders: &[String]) -> Deck {
    deck.meta.declared_bracket = row.bracket;
    deck.meta.price = deck.meta.price.or(Some(row.price));
    deck.meta.salt = deck.meta.salt.or(Some(row.salt));
    if deck.commanders.is_empty() {
        deck.commanders = commanders.to_vec();
    }
    deck
}

/// Deck table, candidate selection, side pages and stored decks: a handful of (cached) requests.
pub fn prepare(http: &Http, store: &DeckStore, opts: &ScoutOptions) -> Result<Prepared> {
    let table = edhrec::deck_table(http, &opts.slug)?;
    let sel = select(&table.table, &opts.select, |h| store.contains(h));
    let mut stats = PoolStats { listed: sel.total, eligible: sel.eligible, ceiling: sel.ceiling, ..Default::default() };
    let sources = load_commander_sources(http, &opts.slug);
    let mut decks = Vec::with_capacity(sel.cached.len());
    for row in &sel.cached {
        match store.fetch(http, &row.urlhash) {
            Ok(d) => decks.push(finish_deck(d, row, &opts.commanders)),
            Err(_) => stats.failed += 1,
        }
    }
    stats.cached = decks.len();
    let to_fetch: Vec<TableRow> = if http.offline() { Vec::new() } else { sel.to_fetch.into_iter().cloned().collect() };
    stats.queued = to_fetch.len();
    Ok(Prepared { decks, sources, stats, to_fetch })
}

pub enum FetchEvent {
    Deck(Box<Deck>),
    /// A deck already on disk (no request was made for it).
    StoredDeck(Box<Deck>),
    /// Side data for a commander; always sent before that commander's first deck.
    Commander {
        slug: String,
        sources: Box<CommanderSources>,
    },
    /// A deck table was scanned (searching across commanders).
    Table {
        listed: usize,
        eligible: usize,
    },
    /// This many decks are now queued for fetching.
    Queued(usize),
    Failed {
        what: String,
        error: String,
    },
    /// The host asked us to back off; nothing more will be fetched this run.
    Stopped(String),
    Done,
}

/// Works on a background thread, strictly one request at a time (the shared [`Http`] enforces
/// pacing). Dropping it stops the thread before its next request.
pub struct Fetcher {
    rx: Receiver<FetchEvent>,
    cancel: Arc<AtomicBool>,
}

/// What a worker thread gets: where to send events, and whether to stop.
struct Worker {
    tx: Sender<FetchEvent>,
    stop: Arc<AtomicBool>,
}

impl Worker {
    /// Send an event; false once the receiver is gone or we were asked to stop.
    fn send(&self, e: FetchEvent) -> bool {
        !self.stop.load(Ordering::Relaxed) && self.tx.send(e).is_ok()
    }

    /// Fetch one deck and report it. Returns false when the worker should stop.
    fn fetch(&self, http: &Http, store: &DeckStore, row: &TableRow, commanders: &[String]) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        match store.fetch(http, &row.urlhash) {
            Ok(d) => self.send(FetchEvent::Deck(Box::new(finish_deck(d, row, commanders)))),
            Err(e) if e.downcast_ref::<HostBlocked>().is_some() => {
                self.send(FetchEvent::Stopped(e.to_string()));
                false
            }
            Err(e) => self.send(FetchEvent::Failed { what: row.urlhash.clone(), error: format!("{e:#}") }),
        }
    }
}

impl Fetcher {
    fn start(work: impl FnOnce(&Worker) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = Worker { tx, stop: cancel.clone() };
        std::thread::spawn(move || {
            work(&worker);
            let _ = worker.tx.send(FetchEvent::Done);
        });
        Self { rx, cancel }
    }

    /// Fetch these decks of one commander.
    pub fn spawn(http: Arc<Http>, store: DeckStore, rows: Vec<TableRow>, commanders: Vec<String>) -> Self {
        Self::start(move |w| {
            for row in &rows {
                if !w.fetch(&http, &store, row, &commanders) {
                    return;
                }
            }
        })
    }

    /// Search across `candidates`: scan each one's deck table (stored decks show up right away),
    /// then spend the fetch budget across the commanders with matching decks.
    pub fn spawn_search(
        http: Arc<Http>,
        store: DeckStore,
        candidates: Vec<ListedCommander>,
        opts: SelectOptions,
    ) -> Self {
        Self::start(move |w| search(w, &http, &store, candidates, &opts))
    }

    /// The next event if one is waiting.
    pub fn try_next(&self) -> Option<FetchEvent> {
        self.rx.try_recv().ok()
    }

    /// Wait for the next event; `None` once the thread is gone.
    pub fn next(&self) -> Option<FetchEvent> {
        self.rx.recv().ok()
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn search(w: &Worker, http: &Http, store: &DeckStore, candidates: Vec<ListedCommander>, opts: &SelectOptions) {
    let mut with_sources: HashSet<String> = HashSet::new();
    let ensure_sources = |slug: &str, with_sources: &mut HashSet<String>| -> bool {
        if with_sources.insert(slug.to_string()) {
            let sources = load_commander_sources(http, slug);
            return w.send(FetchEvent::Commander { slug: slug.to_string(), sources: Box::new(sources) });
        }
        true
    };

    // Phase 1: scan every candidate's deck table; stored matches appear immediately.
    let mut fresh: Vec<(String, f64, Vec<TableRow>)> = Vec::new();
    for c in &candidates {
        if w.stop.load(Ordering::Relaxed) {
            return;
        }
        let table = match edhrec::deck_table(http, &c.slug) {
            Ok(t) => t,
            Err(e) if e.downcast_ref::<HostBlocked>().is_some() => {
                w.send(FetchEvent::Stopped(e.to_string()));
                return;
            }
            Err(e) => {
                if !w.send(FetchEvent::Failed { what: c.name.clone(), error: format!("{e:#}") })
                    || !w.send(FetchEvent::Table { listed: 0, eligible: 0 })
                {
                    return;
                }
                continue;
            }
        };
        let sel = select(&table.table, opts, |h| store.contains(h));
        if !w.send(FetchEvent::Table { listed: sel.total, eligible: sel.eligible }) {
            return;
        }
        if !sel.cached.is_empty() {
            if !ensure_sources(&c.slug, &mut with_sources) {
                return;
            }
            for row in &sel.cached {
                if let Ok(d) = store.fetch(http, &row.urlhash)
                    && !w.send(FetchEvent::StoredDeck(Box::new(finish_deck(d, row, &[]))))
                {
                    return;
                }
            }
        }
        let fresh_eligible = sel.eligible.saturating_sub(sel.cached.len());
        if !sel.to_fetch.is_empty() && !http.offline() {
            fresh.push((c.slug.clone(), (fresh_eligible as f64).sqrt(), sel.to_fetch.into_iter().cloned().collect()));
        }
    }

    // Phase 2: share the budget out by how many matching decks each commander has, and fetch
    // round-robin so every commander shows up early.
    let weights: Vec<f64> = fresh.iter().map(|f| f.1).collect();
    let caps: Vec<usize> = fresh.iter().map(|f| f.2.len()).collect();
    let quotas = allocate_weighted(&weights, &caps, opts.budget);
    let mut queues: Vec<(String, VecDeque<TableRow>)> = fresh
        .into_iter()
        .zip(quotas)
        .filter(|(_, q)| *q > 0)
        .map(|((slug, _, rows), q)| (slug, rows.into_iter().take(q).collect()))
        .collect();
    if !w.send(FetchEvent::Queued(queues.iter().map(|q| q.1.len()).sum())) {
        return;
    }
    while queues.iter().any(|q| !q.1.is_empty()) {
        for (slug, rows) in queues.iter_mut() {
            let Some(row) = rows.pop_front() else { continue };
            if !ensure_sources(slug, &mut with_sources) || !w.fetch(http, store, &row, &[]) {
                return;
            }
        }
    }
}

/// The decks being compared, analysed as they arrive, each against its own commander's data.
pub struct Pool<'a> {
    cards: &'a CardDb,
    combos: &'a ComboDb,
    salt: HashMap<String, f64>,
    commanders: HashMap<String, CommanderSources>,
    /// Used when a deck's commander slug has no entry (e.g. a `--slug` override).
    fallback: Option<String>,
    pub decks: Vec<(Deck, Analysis)>,
    pub stats: PoolStats,
}

impl<'a> Pool<'a> {
    pub fn new(cards: &'a CardDb, combos: &'a ComboDb, salt: HashMap<String, f64>, stats: PoolStats) -> Self {
        Self { cards, combos, salt, commanders: HashMap::new(), fallback: None, decks: Vec::new(), stats }
    }

    pub fn cards(&self) -> &'a CardDb {
        self.cards
    }

    /// Register a commander's side data; the first one registered is also the fallback.
    pub fn add_commander(&mut self, slug: &str, sources: CommanderSources) {
        if self.fallback.is_none() {
            self.fallback = Some(slug.to_string());
        }
        self.commanders.insert(slug.to_string(), sources);
    }

    pub fn add(&mut self, deck: Deck) {
        let key = commander_slug(&deck.commanders);
        let sources = self.commanders.get(&key).or_else(|| self.fallback.as_ref().and_then(|f| self.commanders.get(f)));
        let ctx = Context {
            cards: self.cards,
            combos: self.combos,
            commander: sources.and_then(|s| s.page.as_ref()),
            salt: &self.salt,
            edhrec_combos: sources.map(|s| s.combos.as_slice()).unwrap_or(&[]),
        };
        let a = analyze(&deck, &ctx);
        self.decks.push((deck, a));
    }

    /// Fold one fetch event into the pool. Returns a message worth showing, if any.
    pub fn apply(&mut self, event: FetchEvent) -> Option<String> {
        match event {
            FetchEvent::Deck(d) => {
                self.stats.fetched += 1;
                self.add(*d);
                None
            }
            FetchEvent::StoredDeck(d) => {
                self.stats.cached += 1;
                self.add(*d);
                None
            }
            FetchEvent::Commander { slug, sources } => {
                self.add_commander(&slug, *sources);
                None
            }
            FetchEvent::Table { listed, eligible } => {
                self.stats.commanders_scanned += 1;
                self.stats.listed += listed;
                self.stats.eligible += eligible;
                None
            }
            FetchEvent::Queued(n) => {
                self.stats.queued = n;
                None
            }
            FetchEvent::Failed { what, error } => {
                self.stats.failed += 1;
                Some(format!("skipped {what}: {error}"))
            }
            FetchEvent::Stopped(why) => {
                self.stats.stopped = Some(why.clone());
                Some(format!("stopped fetching: {why}"))
            }
            FetchEvent::Done => None,
        }
    }

    /// Wait for the fetcher to finish, with a progress bar (for non-interactive output).
    pub fn drain(&mut self, fetcher: Fetcher) {
        let pb = progress_bar();
        while let Some(event) = fetcher.next() {
            if matches!(event, FetchEvent::Done) {
                break;
            }
            if let Some(msg) = self.apply(event) {
                pb.println(format!("  {msg}"));
            }
            let st = &self.stats;
            if st.scanning() {
                pb.set_length(st.commanders_total as u64);
                pb.set_position(st.commanders_scanned as u64);
                pb.set_message("scanning commanders");
            } else {
                pb.set_length(st.queued as u64);
                pb.set_position((st.fetched + st.failed) as u64);
                pb.set_message("fetching decklists");
            }
        }
        pb.finish_and_clear();
    }
}

fn progress_bar() -> ProgressBar {
    let pb = ProgressBar::new(0);
    if std::io::stderr().is_terminal() {
        pb.set_style(
            ProgressStyle::with_template("  {spinner:.cyan} {msg:20} {bar:28.cyan/blue} {pos}/{len}")
                .unwrap()
                .progress_chars("━╸─"),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
    } else {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    }
    pb
}

#[cfg(test)]
mod tests {
    use super::*;
    use edh_data::cards::Card;
    use edh_data::edhrec::CardStat;

    fn creature(name: &str) -> Card {
        Card {
            name: name.into(),
            type_line: "Legendary Creature — Human".into(),
            oracle: vec![String::new()],
            layout: "normal".into(),
            ..Default::default()
        }
    }

    #[test]
    fn decks_are_scored_against_their_own_commander() {
        let cards = CardDb::new(vec![creature("Alpha One"), creature("Beta Two"), creature("Signature Card")]);
        let combos = ComboDb::empty();
        let mut pool = Pool::new(&cards, &combos, HashMap::new(), PoolStats::default());
        let page = |syn: f64| CommanderSources {
            page: Some(CommanderPage {
                cards: HashMap::from([("Signature Card".to_string(), CardStat { synergy: syn, ..Default::default() })]),
                ..Default::default()
            }),
            combos: vec![],
        };
        pool.add_commander("alpha-one", page(0.5));
        pool.add_commander("beta-two", page(0.1));
        let deck = |cmdr: &str| Deck {
            commanders: vec![cmdr.into()],
            cards: [("Signature Card".to_string(), 1)].into(),
            ..Default::default()
        };
        pool.add(deck("Alpha One"));
        pool.add(deck("Beta Two"));
        pool.add(deck("Unknown Gamma")); // falls back to the first commander registered
        let syn: Vec<f64> = pool.decks.iter().map(|(_, a)| a.synergy.as_ref().unwrap().average).collect();
        assert_eq!(syn, vec![0.5, 0.1, 0.5]);
    }

    #[test]
    fn search_events_update_stats() {
        let cards = CardDb::new(vec![]);
        let combos = ComboDb::empty();
        let mut pool =
            Pool::new(&cards, &combos, HashMap::new(), PoolStats { commanders_total: 2, ..Default::default() });
        assert!(pool.stats.scanning());
        pool.apply(FetchEvent::Table { listed: 100, eligible: 10 });
        pool.apply(FetchEvent::Table { listed: 50, eligible: 5 });
        pool.apply(FetchEvent::Queued(7));
        assert!(!pool.stats.scanning());
        assert_eq!((pool.stats.listed, pool.stats.eligible, pool.stats.queued), (150, 15, 7));
        assert!(pool.stats.summary().starts_with("2 commanders, 150 on EDHREC, 15 eligible"));
    }
}
