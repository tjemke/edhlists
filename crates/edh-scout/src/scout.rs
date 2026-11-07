//! The `scout` pipeline: deck table -> candidate selection -> stored decks right away, new decks
//! fetched in the background -> analysis as they arrive.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::Result;
use edh_data::cards::CardDb;
use edh_data::combos::ComboDb;
use edh_data::deck::Deck;
use edh_data::edhrec::{self, CommanderPage, DeckStore, EdhrecCombo, TableRow};
use edh_data::http::{HostBlocked, Http, OfflineMiss};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::analysis::{Analysis, Context, analyze};
use crate::select::{Ceiling, SelectOptions, select};

/// Per-commander side data. Each piece is optional: a failure costs that signal, not the run.
#[derive(Default)]
pub struct Sources {
    pub commander: Option<CommanderPage>,
    pub combos: Vec<EdhrecCombo>,
    pub salt: HashMap<String, f64>,
}

pub fn load_sources(http: &Http, slug: Option<&str>) -> Sources {
    let mut s = Sources::default();
    let warn = |what: &str, e: anyhow::Error| {
        if e.downcast_ref::<OfflineMiss>().is_none() {
            eprintln!("  note: couldn't load {what} ({e:#}); continuing without it");
        }
    };
    if let Some(slug) = slug {
        match edhrec::commander_page(http, slug) {
            Ok(p) => s.commander = Some(p),
            Err(e) => warn("EDHREC synergy data", e),
        }
        match edhrec::combos_page(http, slug) {
            Ok(c) => s.combos = c,
            Err(e) => warn("EDHREC combo votes", e),
        }
    }
    match edhrec::top_salt(http) {
        Ok(m) => s.salt = m,
        Err(e) => warn("EDHREC's salt list", e),
    }
    s
}

impl Sources {
    pub fn context<'a>(&'a self, cards: &'a CardDb, combos: &'a ComboDb) -> Context<'a> {
        Context { cards, combos, commander: self.commander.as_ref(), salt: &self.salt, edhrec_combos: &self.combos }
    }
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
    pub ceiling: Option<Ceiling>,
    /// Why fetching stopped before the budget was used up, if it did.
    pub stopped: Option<String>,
}

impl PoolStats {
    /// "34431 on EDHREC, 23804 eligible, 57 reused, 12 fetched"
    pub fn summary(&self) -> String {
        let mut s = format!("{} on EDHREC, {} eligible", self.listed, self.eligible);
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
}

pub struct ScoutOptions {
    pub commanders: Vec<String>,
    pub slug: String,
    pub select: SelectOptions,
}

/// The quick part of a scout: everything except fetching new decks.
pub struct Prepared {
    /// Decks already in the deck store, ready to analyse.
    pub decks: Vec<Deck>,
    pub sources: Sources,
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
    let sources = load_sources(http, Some(&opts.slug));
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
    Failed {
        urlhash: String,
        error: String,
    },
    /// The host asked us to back off; nothing more will be fetched this run.
    Stopped(String),
    Done,
}

/// Fetches decks on a background thread, strictly one request at a time (the shared [`Http`]
/// enforces pacing). Dropping it stops the thread before its next request.
pub struct Fetcher {
    rx: Receiver<FetchEvent>,
    cancel: Arc<AtomicBool>,
    pub total: usize,
}

impl Fetcher {
    pub fn spawn(http: Arc<Http>, store: DeckStore, rows: Vec<TableRow>, commanders: Vec<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = rows.len();
        let stop = cancel.clone();
        std::thread::spawn(move || {
            for row in rows {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let event = match store.fetch(&http, &row.urlhash) {
                    Ok(d) => FetchEvent::Deck(Box::new(finish_deck(d, &row, &commanders))),
                    Err(e) if e.downcast_ref::<HostBlocked>().is_some() => {
                        let _ = tx.send(FetchEvent::Stopped(e.to_string()));
                        break;
                    }
                    Err(e) => FetchEvent::Failed { urlhash: row.urlhash.clone(), error: format!("{e:#}") },
                };
                if tx.send(event).is_err() {
                    return; // nobody is listening any more
                }
            }
            let _ = tx.send(FetchEvent::Done);
        });
        Self { rx, cancel, total }
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

/// The decks being compared, analysed as they arrive.
pub struct Pool<'a> {
    pub ctx: Context<'a>,
    pub decks: Vec<(Deck, Analysis)>,
    pub stats: PoolStats,
}

impl<'a> Pool<'a> {
    pub fn new(ctx: Context<'a>, decks: Vec<Deck>, stats: PoolStats) -> Self {
        let mut pool = Self { ctx, decks: Vec::with_capacity(decks.len() + stats.queued), stats };
        for d in decks {
            pool.add(d);
        }
        pool
    }

    pub fn add(&mut self, deck: Deck) {
        let a = analyze(&deck, &self.ctx);
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
            FetchEvent::Failed { urlhash, error } => {
                self.stats.failed += 1;
                Some(format!("skipped {urlhash}: {error}"))
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
        let pb = progress_bar(fetcher.total);
        while let Some(event) = fetcher.next() {
            if matches!(event, FetchEvent::Done) {
                break;
            }
            if let Some(msg) = self.apply(event) {
                pb.println(format!("  {msg}"));
            }
            pb.set_position((self.stats.fetched + self.stats.failed) as u64);
        }
        pb.finish_and_clear();
    }
}

fn progress_bar(len: usize) -> ProgressBar {
    let pb = ProgressBar::new(len as u64);
    if std::io::stderr().is_terminal() {
        pb.set_style(
            ProgressStyle::with_template("  {spinner:.cyan} fetching decklists {bar:28.cyan/blue} {pos}/{len}")
                .unwrap()
                .progress_chars("━╸─"),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
    } else {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    }
    pb
}
