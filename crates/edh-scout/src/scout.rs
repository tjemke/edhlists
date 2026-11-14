//! The `scout` pipeline: deck table(s) -> candidate selection -> stored decks right away, new decks
//! fetched in the background -> scored as they arrive, on a thread of their own ([`Feed`]), so the
//! browser only ever folds in finished results.
//!
//! Two shapes: one commander ([`prepare`] + [`Fetcher::spawn`]), or a search across commanders
//! picked by card-type filters ([`Fetcher::spawn_search`]), where the deck tables themselves load
//! in the background too.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use edh_data::cards::CardDb;
use edh_data::combos::ComboDb;
use edh_data::deck::Deck;
use edh_data::edhrec::{self, CommanderPage, EdhrecCombo, ListedCommander, TableRow};
use edh_data::http::{HostBlocked, Http, OfflineMiss};
use edh_data::progress::Progress;
use edh_data::text::commander_slug;
use edh_store::{DeckStore, StoredScore};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rayon::prelude::*;

use crate::analysis::{Analysis, Context, Score, analyze, synergy_source};
use crate::filters::{CompiledFilter, Matched};
use crate::rank::Ranked;
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
    /// Stored decks still being scored in the background (after a `sync`, or never scored).
    pub scoring: usize,
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
pub type Scored = Option<(Score, Vec<Matched>)>;

/// A score worked out rather than remembered, as the deck database should keep it.
type Fresh = Option<StoredScore>;

/// Decks scored together on the thread pool, and passed on together.
const SCORE_CHUNK: usize = 256;

/// One deck in the pool: the deck, its score, and what each `-T` card filter counted in it.
pub struct Entry {
    pub deck: Deck,
    pub score: Score,
    pub matches: Vec<Matched>,
}

/// Commanders' side data, as it arrives.
#[derive(Default)]
struct Commanders {
    sources: HashMap<String, CommanderSources>,
    /// Per commander, a fingerprint of the data synergy is measured against.
    synergy: HashMap<String, i64>,
    /// Used when a deck's commander slug has no entry (e.g. a `--slug` override): the first
    /// commander registered.
    fallback: Option<String>,
}

impl Commanders {
    /// The slug whose data `deck` is scored against: its own commander's, or the fallback's.
    fn slug_for(&self, deck: &Deck) -> Option<String> {
        let key = commander_slug(&deck.commanders);
        if self.sources.contains_key(&key) { Some(key) } else { self.fallback.clone() }
    }
}

/// Everything scoring a deck needs. The pool shares it with the thread that scores arriving decks
/// (see [`Feed`]), and uses it to work out a deck's details on the browser's thread.
pub struct Scorer<'a> {
    cards: &'a CardDb,
    combos: &'a ComboDb,
    salt: HashMap<String, f64>,
    commanders: RwLock<Commanders>,
    /// `-T` card filters: decks failing one are left out.
    filters: Vec<CompiledFilter>,
    /// Where scores are remembered, and the epoch they must match (see [`crate::analysis::score_epoch`]).
    store: Option<(DeckStore, i64)>,
}

impl Scorer<'_> {
    fn commanders(&self) -> std::sync::RwLockReadGuard<'_, Commanders> {
        self.commanders.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a commander's side data; the first one registered is also the fallback.
    pub fn add_commander(&self, slug: &str, sources: CommanderSources) {
        let mut c = self.commanders.write().unwrap_or_else(|e| e.into_inner());
        if c.fallback.is_none() {
            c.fallback = Some(slug.to_string());
        }
        c.synergy.insert(slug.to_string(), synergy_source(sources.page.as_ref()));
        c.sources.insert(slug.to_string(), sources);
    }

    /// The scores the deck database has for these decks, in order.
    fn kept_scores(&self, decks: &[Deck]) -> Vec<Option<StoredScore>> {
        let none = || vec![None; decks.len()];
        let Some((store, _)) = &self.store else { return none() };
        let ids: Vec<&str> = decks.iter().map(|d| d.id.as_str()).collect();
        store.db().scores(&ids).unwrap_or_else(|_| none())
    }

    /// Was `kept` scored against what `deck` would be scored against now?
    fn current(&self, deck: &Deck, kept: &StoredScore) -> bool {
        self.store.as_ref().is_some_and(|(_, epoch)| kept.epoch == *epoch) && kept.synergy_src == self.synergy_src(deck)
    }

    /// Score decks on the thread pool, in order, reusing the kept scores that are still current
    /// and keeping the others.
    pub fn score_many(&self, decks: &[Deck]) -> Vec<Scored> {
        self.score_with(decks, self.kept_scores(decks))
    }

    fn score_with(&self, decks: &[Deck], kept: Vec<Option<StoredScore>>) -> Vec<Scored> {
        let scored: Vec<(Scored, Fresh)> = decks.par_iter().zip(kept).map(|(d, k)| self.score(d, k)).collect();
        let fresh: Vec<(&str, StoredScore)> =
            decks.iter().zip(&scored).filter_map(|(d, (_, f))| f.map(|f| (d.id.as_str(), f))).collect();
        if let (Some((store, _)), false) = (&self.store, fresh.is_empty()) {
            // A score that can't be kept is just worked out again next time.
            let _ = store.db().put_scores(&fresh);
        }
        scored.into_iter().map(|(s, _)| s).collect()
    }

    /// The deck's score and filter counts, or `None` if a card filter rejects it. `kept` is used
    /// when it's still current; otherwise the score is worked out and also returned as what to
    /// keep.
    fn score(&self, deck: &Deck, kept: Option<StoredScore>) -> (Scored, Fresh) {
        let matches: Vec<Matched> = self.filters.iter().map(|f| f.count(deck, self.cards)).collect();
        if matches.iter().any(|m| !m.passes) {
            return (None, None);
        }
        if let Some(k) = kept.filter(|k| self.current(deck, k)) {
            return (Some((stored_score(&k, deck), matches)), None);
        }
        let s = Score::of(&self.with_context(deck, |ctx| analyze(deck, ctx)));
        let fresh = self.store.as_ref().map(|(_, epoch)| StoredScore {
            epoch: *epoch,
            synergy_src: self.synergy_src(deck),
            power: s.power,
            bracket: s.bracket,
            floor: s.floor,
            structure: s.structure,
            synergy: s.synergy,
        });
        (Some((s, matches)), fresh)
    }

    /// A deck's full analysis (bracket reasons, card lists, combos), with the cards each filter
    /// counted: about a millisecond.
    pub fn details(&self, deck: &Deck, matches: &[Matched]) -> Analysis {
        let mut a = self.with_context(deck, |ctx| analyze(deck, ctx));
        a.matches = matches.to_vec();
        a
    }

    fn synergy_src(&self, deck: &Deck) -> i64 {
        let c = self.commanders();
        c.slug_for(deck).and_then(|s| c.synergy.get(&s).copied()).unwrap_or(0)
    }

    /// Run `f` with what analysing `deck` needs: its own commander's data, or the fallback's.
    fn with_context<R>(&self, deck: &Deck, f: impl FnOnce(&Context) -> R) -> R {
        let c = self.commanders();
        let sources = c.slug_for(deck).and_then(|s| c.sources.get(&s));
        f(&Context {
            cards: self.cards,
            combos: self.combos,
            commander: sources.and_then(|s| s.page.as_ref()),
            salt: &self.salt,
            edhrec_combos: sources.map(|s| s.combos.as_slice()).unwrap_or(&[]),
        })
    }
}

/// A kept score as the pool uses it; salt comes from the deck itself.
fn stored_score(k: &StoredScore, deck: &Deck) -> Score {
    Score {
        power: k.power,
        bracket: k.bracket,
        floor: k.floor,
        structure: k.structure,
        synergy: k.synergy,
        salt: deck.meta.salt,
    }
}

/// The decks being compared, each scored against its own commander's data. Only the scores are
/// kept; [`Pool::details`] works out a deck's full analysis when it's needed.
pub struct Pool<'a> {
    scorer: Arc<Scorer<'a>>,
    pub decks: Vec<Entry>,
    /// Searching across commanders: how many eligible decks EDHREC lists for each one (by slug),
    /// a measure of how popular the commander is for this kind of deck.
    eligible: HashMap<String, usize>,
    pub stats: PoolStats,
}

impl<'a> Pool<'a> {
    pub fn new(cards: &'a CardDb, combos: &'a ComboDb, salt: HashMap<String, f64>, stats: PoolStats) -> Self {
        let scorer = Scorer { cards, combos, salt, commanders: RwLock::default(), filters: Vec::new(), store: None };
        Self { scorer: Arc::new(scorer), decks: Vec::new(), eligible: HashMap::new(), stats }
    }

    fn scorer_mut(&mut self) -> &mut Scorer<'a> {
        Arc::get_mut(&mut self.scorer).expect("the pool is set up before scoring is shared")
    }

    /// Only keep decks that pass every one of these card filters (set before adding decks).
    pub fn with_filters(mut self, filters: Vec<CompiledFilter>) -> Self {
        self.scorer_mut().filters = filters;
        self
    }

    /// Remember scores in the deck database, and reuse those still valid for `epoch`.
    pub fn with_store(mut self, store: DeckStore, epoch: i64) -> Self {
        self.scorer_mut().store = Some((store, epoch));
        self
    }

    pub fn cards(&self) -> &'a CardDb {
        self.scorer.cards
    }

    /// The scorer, to score decks on another thread (see [`Feed`]).
    pub fn scorer(&self) -> Arc<Scorer<'a>> {
        Arc::clone(&self.scorer)
    }

    /// Register a commander's side data; the first one registered is also the fallback.
    pub fn add_commander(&mut self, slug: &str, sources: CommanderSources) {
        self.scorer.add_commander(slug, sources);
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
        self.add_many(vec![deck]);
    }

    /// [`Pool::add`] for many decks at once; they're added in order. Scores the deck database
    /// still has are reused; the rest are worked out on the thread pool and kept there.
    pub fn add_many(&mut self, decks: Vec<Deck>) {
        let scored = self.scorer.score_many(&decks);
        for (deck, s) in decks.into_iter().zip(scored) {
            self.push(deck, s);
        }
    }

    /// Add the stored decks a run starts with. Those whose kept score is still current go in at
    /// once. The others (after a `sync`, or never scored) are scored for up to `budget`, best
    /// first by their old score as `rank` orders it, so the top of the list is likely settled when
    /// the browser opens; the rest are returned, in that order, for a [`Feed`] to score while the
    /// browser is open. Without a budget all of them are scored here.
    pub fn add_stored(
        &mut self,
        decks: Vec<Deck>,
        budget: Option<Duration>,
        rank: &dyn Fn(&[&Score]) -> Vec<Ranked>,
    ) -> Vec<Deck> {
        let started = Instant::now();
        let kept = self.scorer.kept_scores(&decks);
        let (mut ready, mut stale, mut unscored) = (Vec::new(), Vec::new(), Vec::new());
        for (deck, k) in decks.into_iter().zip(kept) {
            match k {
                Some(k) if self.scorer.current(&deck, &k) => ready.push((deck, Some(k))),
                Some(k) => stale.push((stored_score(&k, &deck), deck)),
                None => unscored.push(deck),
            }
        }
        let (ready, kept): (Vec<Deck>, Vec<Option<StoredScore>>) = ready.into_iter().unzip();
        let scored = self.scorer.score_with(&ready, kept);
        for (deck, s) in ready.into_iter().zip(scored) {
            self.push(deck, s);
        }
        let order = rank(&stale.iter().map(|(s, _)| s).collect::<Vec<_>>());
        let mut stale: Vec<Option<Deck>> = stale.into_iter().map(|(_, d)| Some(d)).collect();
        let mut pending = order.iter().filter_map(|r| stale[r.index].take()).chain(unscored);
        let mut rest = Vec::new();
        loop {
            if budget.is_some_and(|b| started.elapsed() >= b) {
                rest.extend(pending);
                break;
            }
            let chunk: Vec<Deck> = pending.by_ref().take(SCORE_CHUNK).collect();
            if chunk.is_empty() {
                break;
            }
            self.add_many(chunk);
        }
        self.stats.scoring = rest.len();
        rest
    }

    fn push(&mut self, deck: Deck, scored: Scored) {
        match scored {
            Some((score, matches)) => self.decks.push(Entry { deck, score, matches }),
            None => self.stats.unmatched += 1,
        }
    }

    /// The full analysis of the pool's deck `i` (bracket reasons, card lists, combos, the cards
    /// each filter counted), worked out again: about a millisecond.
    pub fn details(&self, i: usize) -> Analysis {
        let e = &self.decks[i];
        self.scorer.details(&e.deck, &e.matches)
    }

    /// The scores, in pool order, for ranking.
    pub fn scores(&self) -> Vec<&Score> {
        self.decks.iter().map(|e| &e.score).collect()
    }

    /// Fold one event into the pool. Returns a message worth showing, if any.
    pub fn apply(&mut self, event: FeedEvent) -> Option<String> {
        let event = match event {
            FeedEvent::Scored { deck, scored, from } => {
                match from {
                    Origin::Backlog => self.stats.scoring = self.stats.scoring.saturating_sub(1),
                    Origin::Stored => self.stats.cached += 1,
                    Origin::Fetched => self.stats.fetched += 1,
                }
                self.push(*deck, scored);
                return None;
            }
            FeedEvent::Fetch(e) => e,
        };
        match event {
            // Not scored yet (no feed in between): score them here.
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

    /// Wait for the feed to finish, with a progress bar (for non-interactive output).
    pub fn drain(&mut self, feed: Feed) {
        let pb = progress_bar();
        while let Some(event) = feed.next() {
            if matches!(event, FeedEvent::Fetch(FetchEvent::Done)) {
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

/// Where a scored deck came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A stored deck [`Pool::add_stored`] left to score in the background.
    Backlog,
    /// A stored deck a search across commanders found.
    Stored,
    /// A deck just fetched.
    Fetched,
}

/// What a [`Feed`] passes on: fetch events, with the decks in them already scored.
pub enum FeedEvent {
    Scored { deck: Box<Deck>, scored: Scored, from: Origin },
    Fetch(FetchEvent),
}

/// Scores decks on a thread of its own, so the browser only ever folds in finished results:
/// first the stored decks [`Pool::add_stored`] left, then whatever the fetcher brings, in order.
/// It runs inside a thread scope, so it can borrow the card and combo databases. Dropping it stops
/// that thread within a fraction of a second, and the fetcher with it.
pub struct Feed {
    rx: Receiver<FeedEvent>,
    stop: Arc<AtomicBool>,
}

impl Feed {
    pub fn start<'scope, 'env>(
        scope: &'scope std::thread::Scope<'scope, 'env>,
        scorer: Arc<Scorer<'env>>,
        backlog: Vec<Deck>,
        fetcher: Option<Fetcher>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        scope.spawn(move || feed(&scorer, backlog, fetcher, &tx, &flag));
        Self { rx, stop }
    }

    /// The next event if one is waiting.
    pub fn try_next(&self) -> Option<FeedEvent> {
        self.rx.try_recv().ok()
    }

    /// Wait for the next event; `None` once the feed is gone.
    pub fn next(&self) -> Option<FeedEvent> {
        self.rx.recv().ok()
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn feed(scorer: &Scorer, backlog: Vec<Deck>, fetcher: Option<Fetcher>, tx: &Sender<FeedEvent>, stop: &AtomicBool) {
    // Score a batch together, then pass each deck on; false once nobody is listening.
    let pass_on = |batch: Vec<(Deck, Origin)>| -> bool {
        let (decks, from): (Vec<Deck>, Vec<Origin>) = batch.into_iter().unzip();
        let scored = scorer.score_many(&decks);
        decks
            .into_iter()
            .zip(scored)
            .zip(from)
            .all(|((deck, scored), from)| tx.send(FeedEvent::Scored { deck: Box::new(deck), scored, from }).is_ok())
    };
    let mut backlog = backlog.into_iter();
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let chunk: Vec<(Deck, Origin)> = backlog.by_ref().take(SCORE_CHUNK).map(|d| (d, Origin::Backlog)).collect();
        if chunk.is_empty() {
            break;
        }
        if !pass_on(chunk) {
            return;
        }
    }
    let Some(fetcher) = fetcher else {
        let _ = tx.send(FeedEvent::Fetch(FetchEvent::Done));
        return;
    };
    // Decks arrive one at a time when fetched, or by the thousand when a search reads a
    // commander's stored decks: whatever has arrived together is scored together.
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let first = match fetcher.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut events = vec![first];
        while events.len() < SCORE_CHUNK
            && let Ok(e) = fetcher.rx.try_recv()
        {
            events.push(e);
        }
        let mut batch = Vec::new();
        for event in events {
            match event {
                FetchEvent::Deck(d) => batch.push((*d, Origin::Fetched)),
                FetchEvent::StoredDeck(d) => batch.push((*d, Origin::Stored)),
                other => {
                    // In order: the decks before a commander's data arrived don't need it.
                    if !pass_on(std::mem::take(&mut batch)) {
                        return;
                    }
                    match other {
                        FetchEvent::Commander { slug, sources } => scorer.add_commander(&slug, *sources),
                        other => {
                            let done = matches!(other, FetchEvent::Done);
                            if tx.send(FeedEvent::Fetch(other)).is_err() || done {
                                return;
                            }
                        }
                    }
                }
            }
        }
        if !pass_on(batch) {
            return;
        }
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
    fn scores_are_remembered_and_worked_out_again_when_their_inputs_change() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeckStore::open(&dir.path().join("decks.sqlite"), dir.path().join("pages")).unwrap();
        let cards = CardDb::new(vec![creature("Alpha One"), creature("Grizzly Bears")]);
        let combos = ComboDb::empty();
        let deck = Deck {
            id: "abc".into(),
            commanders: vec!["Alpha One".into()],
            cards: [("Grizzly Bears".to_string(), 1)].into(),
            ..Default::default()
        };
        let pool = |epoch: i64, synergy: Option<f64>| {
            let mut p =
                Pool::new(&cards, &combos, HashMap::new(), PoolStats::default()).with_store(store.clone(), epoch);
            if let Some(syn) = synergy {
                let page = CommanderPage {
                    cards: HashMap::from([(
                        "Grizzly Bears".to_string(),
                        CardStat { synergy: syn, ..Default::default() },
                    )]),
                    ..Default::default()
                };
                p.add_commander("alpha-one", CommanderSources { page: Some(page), combos: vec![] });
            }
            p.add(deck.clone());
            p.decks[0].score
        };
        let first = pool(1, None);
        let kept = store.db().scores(&["abc"]).unwrap()[0].unwrap();
        assert_eq!((kept.epoch, kept.power), (1, first.power));
        // Plant a marker to tell a remembered score from a recomputed one.
        store.db().put_scores(&[("abc", StoredScore { power: 9.99, ..kept })]).unwrap();
        assert_eq!(pool(1, None).power, 9.99);
        // A new epoch (a card database update, a scorer change) works it out again, and keeps it.
        assert_eq!(pool(2, None).power, first.power);
        assert_eq!(store.db().scores(&["abc"]).unwrap()[0].unwrap().epoch, 2);
        // So does new commander data, which synergy depends on.
        store.db().put_scores(&[("abc", StoredScore { power: 9.99, epoch: 2, ..kept })]).unwrap();
        let with_page = pool(2, Some(0.4));
        assert_eq!((with_page.power, with_page.synergy), (first.power, Some(0.4)));
    }

    fn decks(n: usize) -> Vec<Deck> {
        (0..n)
            .map(|i| Deck {
                id: format!("d{i}"),
                commanders: vec!["Alpha One".into()],
                cards: [("Grizzly Bears".to_string(), (i % 5) as u32 + 1)].into(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn stored_decks_with_stale_scores_wait_their_turn_best_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeckStore::open(&dir.path().join("decks.sqlite"), dir.path().join("pages")).unwrap();
        let cards = CardDb::new(vec![creature("Alpha One"), creature("Grizzly Bears")]);
        let combos = ComboDb::empty();
        let target = crate::rank::Target::new((2, 3), None);
        let by_power = |s: &[&Score]| crate::rank::rank(s, &target, crate::rank::RankMode::Power, &Default::default());
        // Kept from an older epoch: d3 had the highest power, then d1; d0 and d2 were never scored.
        let old = |power: f64| StoredScore {
            epoch: 1,
            synergy_src: 0,
            power,
            bracket: 3,
            floor: 1,
            structure: 0.5,
            synergy: None,
        };
        store.db().put_scores(&[("d1", old(5.0)), ("d3", old(7.0))]).unwrap();
        let pool = || Pool::new(&cards, &combos, HashMap::new(), PoolStats::default()).with_store(store.clone(), 2);
        let mut p = pool();
        let rest = p.add_stored(decks(4), Some(Duration::ZERO), &by_power);
        assert!(p.decks.is_empty());
        assert_eq!(rest.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["d3", "d1", "d0", "d2"]);
        assert_eq!(p.stats.scoring, 4);
        // Without a budget everything is scored before returning, and the scores are kept.
        let rest = pool().add_stored(decks(4), None, &by_power);
        assert!(rest.is_empty());
        assert!(store.db().scores(&["d0", "d3"]).unwrap().iter().all(|s| s.is_some_and(|s| s.epoch == 2)));
        // Next time they're all current: in at once, whatever the budget.
        let mut p = pool();
        assert!(p.add_stored(decks(4), Some(Duration::ZERO), &by_power).is_empty());
        assert_eq!(p.decks.len(), 4);
    }

    #[test]
    fn a_feed_scores_the_backlog_then_the_fetched_decks() {
        let cards = CardDb::new(vec![creature("Alpha One"), creature("Grizzly Bears")]);
        let combos = ComboDb::empty();
        let mut pool = Pool::new(&cards, &combos, HashMap::new(), PoolStats { scoring: 3, ..Default::default() });
        let (tx, rx) = mpsc::channel();
        let fetcher = Fetcher { rx, cancel: Arc::new(AtomicBool::new(false)) };
        let mut fetched = decks(5).split_off(3);
        tx.send(FetchEvent::Deck(Box::new(fetched.remove(0)))).unwrap();
        tx.send(FetchEvent::StoredDeck(Box::new(fetched.remove(0)))).unwrap();
        tx.send(FetchEvent::Done).unwrap();
        std::thread::scope(|s| {
            let feed = Feed::start(s, pool.scorer(), decks(3), Some(fetcher));
            pool.drain(feed);
        });
        let ids: Vec<&str> = pool.decks.iter().map(|e| e.deck.id.as_str()).collect();
        assert_eq!(ids, vec!["d0", "d1", "d2", "d3", "d4"]);
        assert_eq!((pool.stats.scoring, pool.stats.fetched, pool.stats.cached), (0, 1, 1));
        // Dropping a feed stops its thread even while the fetcher is still quiet.
        let (_tx, rx) = mpsc::channel();
        let quiet = Fetcher { rx, cancel: Arc::new(AtomicBool::new(false)) };
        std::thread::scope(|s| drop(Feed::start(s, pool.scorer(), vec![], Some(quiet))));
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
        pool.apply(FeedEvent::Fetch(FetchEvent::Table { slug: "a".into(), listed: 100, eligible: 10 }));
        pool.apply(FeedEvent::Fetch(FetchEvent::Table { slug: "b".into(), listed: 50, eligible: 5 }));
        pool.apply(FeedEvent::Fetch(FetchEvent::Queued(7)));
        assert!(!pool.stats.scanning());
        assert_eq!((pool.stats.listed, pool.stats.eligible, pool.stats.queued), (150, 15, 7));
        assert!(pool.stats.summary().starts_with("2 commanders, 150 on EDHREC, 15 eligible"));
    }
}
