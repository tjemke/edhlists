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
use edh_data::edhrec::{self, CommanderPage, EdhrecCombo, ListedCommander, TableRow};
use edh_data::http::{HostBlocked, Http, OfflineMiss};
use edh_data::progress::Progress;
use edh_data::text::commander_slug;
use edh_store::DeckStore;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rayon::prelude::*;

use crate::analysis::{Analysis, Context, Score, analyze};
use crate::filters::{CompiledFilter, Matched};
use crate::select::{Ceiling, SelectOptions, select, weighted_order_from};

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
    /// Decks (stored or fetched) left out because a `-T` card filter rejected them.
    pub unmatched: usize,
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
        if self.unmatched > 0 {
            s += &format!(", {} didn't match", self.unmatched);
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
    /// The remaining eligible decks past the budget, continuing that order (empty when offline).
    pub more: Vec<TableRow>,
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

/// These table rows' decks from the deck database (one query), with what the table knows added.
fn stored_decks(store: &DeckStore, rows: &[&TableRow], commanders: &[String]) -> Result<Vec<Deck>> {
    let ids: Vec<&str> = rows.iter().map(|r| r.urlhash.as_str()).collect();
    let found = store.get_many(&ids)?;
    Ok(found.into_iter().zip(rows).filter_map(|(d, row)| d.map(|d| finish_deck(d, row, commanders))).collect())
}

/// Deck table, candidate selection, side pages and stored decks: a handful of (cached) requests.
pub fn prepare(http: &Http, store: &DeckStore, opts: &ScoutOptions, progress: &dyn Progress) -> Result<Prepared> {
    progress.step(&format!("loading EDHREC's deck table for {}", opts.slug));
    let table = edhrec::deck_table(http, &opts.slug)?;
    let sel = select(&table.table, &opts.select, |h| store.contains(h));
    let mut stats = PoolStats { listed: sel.total, eligible: sel.eligible, ceiling: sel.ceiling, ..Default::default() };
    progress.step("loading EDHREC's synergy and combo votes");
    let sources = load_commander_sources(http, &opts.slug);
    progress.step(&format!("reading {} stored decks", sel.cached.len()));
    let decks = stored_decks(store, &sel.cached, &opts.commanders)?;
    stats.failed += sel.cached.len() - decks.len();
    stats.cached = decks.len();
    let (to_fetch, more): (Vec<TableRow>, Vec<TableRow>) = if http.offline() {
        Default::default()
    } else {
        (sel.to_fetch.into_iter().cloned().collect(), sel.more.into_iter().cloned().collect())
    };
    stats.queued = to_fetch.len();
    Ok(Prepared { decks, sources, stats, to_fetch, more })
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
        /// The commander's slug; empty when its table couldn't be loaded.
        slug: String,
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
    /// then spend the fetch budget across the commanders with matching decks. With `keep_going`,
    /// carry on past the budget until every matching deck is fetched (or the fetcher is dropped).
    pub fn spawn_search(
        http: Arc<Http>,
        store: DeckStore,
        candidates: Vec<ListedCommander>,
        opts: SelectOptions,
        keep_going: bool,
    ) -> Self {
        Self::start(move |w| search(w, &http, &store, candidates, &opts, keep_going))
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

fn search(
    w: &Worker,
    http: &Http,
    store: &DeckStore,
    candidates: Vec<ListedCommander>,
    opts: &SelectOptions,
    keep_going: bool,
) {
    let mut with_sources: HashSet<String> = HashSet::new();
    let ensure_sources = |slug: &str, with_sources: &mut HashSet<String>| -> bool {
        if with_sources.insert(slug.to_string()) {
            let sources = load_commander_sources(http, slug);
            return w.send(FetchEvent::Commander { slug: slug.to_string(), sources: Box::new(sources) });
        }
        true
    };

    // Phase 1: scan every candidate's deck table; stored matches appear immediately.
    // (slug, weight, decks already stored, decks to fetch)
    let mut fresh: Vec<(String, f64, usize, VecDeque<TableRow>)> = Vec::new();
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
                    || !w.send(FetchEvent::Table { slug: String::new(), listed: 0, eligible: 0 })
                {
                    return;
                }
                continue;
            }
        };
        let sel = select(&table.table, opts, |h| store.contains(h));
        if !w.send(FetchEvent::Table { slug: c.slug.clone(), listed: sel.total, eligible: sel.eligible }) {
            return;
        }
        if !sel.cached.is_empty() {
            if !ensure_sources(&c.slug, &mut with_sources) {
                return;
            }
            let stored = match stored_decks(store, &sel.cached, &[]) {
                Ok(decks) => decks,
                Err(e) => {
                    w.send(FetchEvent::Failed { what: format!("{}'s stored decks", c.name), error: format!("{e:#}") });
                    Vec::new()
                }
            };
            for d in stored {
                if !w.send(FetchEvent::StoredDeck(Box::new(d))) {
                    return;
                }
            }
        }
        if !sel.to_fetch.is_empty() && !http.offline() {
            let stored = sel.cached.len();
            let rows = sel.to_fetch.into_iter().chain(sel.more).cloned().collect();
            fresh.push((c.slug.clone(), (sel.eligible as f64).sqrt(), stored, rows));
        }
    }

    // Phase 2: share the budget out by how many matching decks each commander has, interleaving
    // the commanders so every one shows up early. Decks already stored count towards a
    // commander's share, so commanders with few or none stored are fetched first instead of being
    // drowned out by one with many. Past the budget, the same order continues.
    let weights: Vec<f64> = fresh.iter().map(|f| f.1).collect();
    let caps: Vec<usize> = fresh.iter().map(|f| f.3.len()).collect();
    let had: Vec<usize> = fresh.iter().map(|f| f.2).collect();
    let mut order = weighted_order_from(&weights, &caps, &had);
    if !keep_going {
        order.truncate(opts.budget);
    }
    if !w.send(FetchEvent::Queued(order.len())) {
        return;
    }
    for i in order {
        let (slug, _, _, rows) = &mut fresh[i];
        let row = rows.pop_front().expect("weighted_order stays within each queue");
        if !ensure_sources(slug, &mut with_sources) || !w.fetch(http, store, &row, &[]) {
            return;
        }
    }
}

/// A deck's score and what each card filter counted, or `None` when a filter rejects it.
type Scored = Option<(Score, Vec<Matched>)>;

/// One deck in the pool: the deck, its score, and what each `-T` card filter counted in it.
pub struct Entry {
    pub deck: Deck,
    pub score: Score,
    pub matches: Vec<Matched>,
}

/// The decks being compared, scored as they arrive, each against its own commander's data. Only
/// the scores are kept; [`Pool::details`] works out a deck's full analysis when it's needed.
pub struct Pool<'a> {
    cards: &'a CardDb,
    combos: &'a ComboDb,
    salt: HashMap<String, f64>,
    commanders: HashMap<String, CommanderSources>,
    /// Used when a deck's commander slug has no entry (e.g. a `--slug` override).
    fallback: Option<String>,
    /// `-T` card filters: decks failing one are left out.
    filters: Vec<CompiledFilter>,
    pub decks: Vec<Entry>,
    /// Searching across commanders: how many eligible decks EDHREC lists for each one (by slug),
    /// a measure of how popular the commander is for this kind of deck.
    eligible: HashMap<String, usize>,
    pub stats: PoolStats,
}

impl<'a> Pool<'a> {
    pub fn new(cards: &'a CardDb, combos: &'a ComboDb, salt: HashMap<String, f64>, stats: PoolStats) -> Self {
        Self {
            cards,
            combos,
            salt,
            commanders: HashMap::new(),
            fallback: None,
            filters: Vec::new(),
            decks: Vec::new(),
            eligible: HashMap::new(),
            stats,
        }
    }

    /// Only keep decks that pass every one of these card filters (set before adding decks).
    pub fn with_filters(mut self, filters: Vec<CompiledFilter>) -> Self {
        self.filters = filters;
        self
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

    /// For mixing commanders in the ranking: each deck's commander, and how much room that
    /// commander gets (the square root of its eligible decks on EDHREC, as when fetching).
    pub fn commander_groups(&self) -> (Vec<usize>, Vec<f64>) {
        let mut slugs: HashMap<String, usize> = HashMap::new();
        let mut weights = Vec::new();
        let groups = self
            .decks
            .iter()
            .map(|e| {
                let slug = commander_slug(&e.deck.commanders);
                *slugs.entry(slug.clone()).or_insert_with(|| {
                    weights.push((self.eligible.get(&slug).copied().unwrap_or(1).max(1) as f64).sqrt());
                    weights.len() - 1
                })
            })
            .collect();
        (groups, weights)
    }

    /// Score a deck and add it, unless a card filter rejects it.
    pub fn add(&mut self, deck: Deck) {
        let scored = self.score(&deck);
        self.push(deck, scored);
    }

    /// [`Pool::add`] for many decks at once, scored on the thread pool; they're added in order.
    pub fn add_many(&mut self, decks: Vec<Deck>) {
        let scored: Vec<(Deck, Scored)> = decks
            .into_par_iter()
            .map(|deck| {
                let s = self.score(&deck);
                (deck, s)
            })
            .collect();
        for (deck, s) in scored {
            self.push(deck, s);
        }
    }

    fn push(&mut self, deck: Deck, scored: Scored) {
        match scored {
            Some((score, matches)) => self.decks.push(Entry { deck, score, matches }),
            None => self.stats.unmatched += 1,
        }
    }

    /// The deck's score and filter counts, or `None` if a card filter rejects it.
    fn score(&self, deck: &Deck) -> Scored {
        let matches: Vec<Matched> = self.filters.iter().map(|f| f.count(deck, self.cards)).collect();
        if matches.iter().any(|m| !m.passes) {
            return None;
        }
        Some((Score::of(&analyze(deck, &self.context(deck))), matches))
    }

    /// The full analysis of the pool's deck `i` (bracket reasons, card lists, combos, the cards
    /// each filter counted), worked out again: about a millisecond.
    pub fn details(&self, i: usize) -> Analysis {
        let e = &self.decks[i];
        let mut a = analyze(&e.deck, &self.context(&e.deck));
        a.matches = e.matches.clone();
        a
    }

    /// The scores, in pool order, for ranking.
    pub fn scores(&self) -> Vec<&Score> {
        self.decks.iter().map(|e| &e.score).collect()
    }

    /// What analysing `deck` needs: its own commander's data, or the fallback's.
    fn context(&self, deck: &Deck) -> Context<'_> {
        let key = commander_slug(&deck.commanders);
        let sources = self.commanders.get(&key).or_else(|| self.fallback.as_ref().and_then(|f| self.commanders.get(f)));
        Context {
            cards: self.cards,
            combos: self.combos,
            commander: sources.and_then(|s| s.page.as_ref()),
            salt: &self.salt,
            edhrec_combos: sources.map(|s| s.combos.as_slice()).unwrap_or(&[]),
        }
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
            FetchEvent::Table { slug, listed, eligible } => {
                self.stats.commanders_scanned += 1;
                if !slug.is_empty() {
                    self.eligible.insert(slug, eligible);
                }
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
        let syn: Vec<f64> = pool.decks.iter().map(|e| e.score.synergy.unwrap()).collect();
        assert_eq!(syn, vec![0.5, 0.1, 0.5]);
    }

    #[test]
    fn card_filters_keep_matching_decks_only() {
        let mut eq = creature("Bonesplitter");
        eq.type_line = "Artifact — Equipment".into();
        let cards = CardDb::new(vec![creature("Alpha One"), eq, creature("Grizzly Bears")]);
        let combos = ComboDb::empty();
        let filter = match "equipment=1+".parse::<crate::filters::Filter>().unwrap() {
            crate::filters::Filter::Cards(c) => c.compile(&cards, None).unwrap(),
            _ => unreachable!(),
        };
        let mut pool = Pool::new(&cards, &combos, HashMap::new(), PoolStats::default()).with_filters(vec![filter]);
        let deck = |card: &str| Deck {
            commanders: vec!["Alpha One".into()],
            cards: [(card.to_string(), 1)].into(),
            ..Default::default()
        };
        pool.add(deck("Bonesplitter"));
        pool.add(deck("Grizzly Bears"));
        assert_eq!(pool.decks.len(), 1);
        assert_eq!(pool.decks[0].matches[0].count, 1);
        let details = pool.details(0);
        assert_eq!((details.matches[0].count, details.card_count), (1, 2));
        assert_eq!(pool.stats.unmatched, 1);
        assert!(pool.stats.summary().ends_with("1 didn't match"));
    }

    #[test]
    fn search_events_update_stats() {
        let cards = CardDb::new(vec![]);
        let combos = ComboDb::empty();
        let mut pool =
            Pool::new(&cards, &combos, HashMap::new(), PoolStats { commanders_total: 2, ..Default::default() });
        assert!(pool.stats.scanning());
        pool.apply(FetchEvent::Table { slug: "a".into(), listed: 100, eligible: 10 });
        pool.apply(FetchEvent::Table { slug: "b".into(), listed: 50, eligible: 5 });
        pool.apply(FetchEvent::Queued(7));
        assert!(!pool.stats.scanning());
        assert_eq!((pool.stats.listed, pool.stats.eligible, pool.stats.queued), (150, 15, 7));
        assert!(pool.stats.summary().starts_with("2 commanders, 150 on EDHREC, 15 eligible"));
    }
}
