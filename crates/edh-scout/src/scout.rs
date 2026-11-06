//! The `scout` pipeline: deck table -> candidate selection -> (cached or fetched) decks -> analysis.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::Duration;

use anyhow::Result;
use edh_data::cards::CardDb;
use edh_data::combos::ComboDb;
use edh_data::deck::Deck;
use edh_data::edhrec::{self, CommanderPage, EdhrecCombo};
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

#[derive(Debug, Default)]
pub struct PoolStats {
    pub listed: usize,
    pub eligible: usize,
    pub cached: usize,
    pub fetched: usize,
    pub failed: usize,
    pub ceiling: Option<Ceiling>,
    /// Why fetching stopped before the budget was used up, if it did.
    pub stopped: Option<String>,
}

pub struct Pool {
    pub decks: Vec<(Deck, Analysis)>,
    pub sources: Sources,
    pub stats: PoolStats,
}

pub struct ScoutOptions {
    pub commanders: Vec<String>,
    pub slug: String,
    pub select: SelectOptions,
}

fn progress_bar(len: usize) -> ProgressBar {
    let pb = ProgressBar::new(len as u64);
    if std::io::stderr().is_terminal() {
        pb.set_style(
            ProgressStyle::with_template("  {spinner:.cyan} fetching decklists {bar:28.cyan/blue} {pos}/{len}  {msg}")
                .unwrap()
                .progress_chars("━╸─"),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
    } else {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    }
    pb
}

pub fn scout(http: &Http, cards: &CardDb, combos: &ComboDb, opts: &ScoutOptions) -> Result<Pool> {
    let table = edhrec::deck_table(http, &opts.slug)?;
    let sel = select(&table.table, &opts.select, |h| edhrec::deck_is_cached(http, h));
    let mut stats = PoolStats {
        listed: sel.total,
        eligible: sel.eligible,
        cached: sel.cached.len(),
        ceiling: sel.ceiling,
        ..Default::default()
    };
    let sources = load_sources(http, Some(&opts.slug));

    let mut raw: Vec<(Deck, Option<u8>, f64, f64)> = Vec::new();
    for row in &sel.cached {
        match edhrec::fetch_deck(http, &row.urlhash) {
            Ok(d) => raw.push((d, row.bracket, row.price, row.salt)),
            Err(_) => stats.failed += 1,
        }
    }
    if !sel.to_fetch.is_empty() && !http.offline() {
        let pb = progress_bar(sel.to_fetch.len());
        for row in &sel.to_fetch {
            pb.set_message(row.urlhash.clone());
            match edhrec::fetch_deck(http, &row.urlhash) {
                Ok(d) => {
                    stats.fetched += 1;
                    raw.push((d, row.bracket, row.price, row.salt));
                }
                Err(e) if e.downcast_ref::<HostBlocked>().is_some() => {
                    stats.stopped = Some(e.to_string());
                    break;
                }
                Err(e) => {
                    stats.failed += 1;
                    pb.println(format!("  skipped {}: {e:#}", row.urlhash));
                }
            }
            pb.inc(1);
        }
        pb.finish_and_clear();
    }

    let ctx = sources.context(cards, combos);
    let decks = raw
        .into_iter()
        .map(|(mut deck, bracket, price, salt)| {
            deck.meta.declared_bracket = bracket;
            deck.meta.price = deck.meta.price.or(Some(price));
            deck.meta.salt = deck.meta.salt.or(Some(salt));
            if deck.commanders.is_empty() {
                deck.commanders = opts.commanders.clone();
            }
            let a = analyze(&deck, &ctx);
            (deck, a)
        })
        .collect();
    Ok(Pool { decks, sources, stats })
}
