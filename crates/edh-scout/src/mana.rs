//! How a deck's mana develops: the mana goldfish ([`edh_goldfish`]) run on a deck, at once or (in
//! the browser) in the background.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

use edh_data::cards::{Card, CardDb};
use edh_data::config::Paths;
use edh_data::deck::Deck;
use edh_data::forge::{ForgeDb, ForgeSource};
use edh_goldfish::{DeckEntry, ManaReport};

/// How the goldfish plays: how many games, with which card scripts.
#[derive(Clone)]
pub struct Goldfish {
    /// Games per deck; 0 turns the goldfish off.
    pub games: u32,
    source: ForgeSource,
    db: PathBuf,
    /// The download the database is rebuilt from when it's of an older schema.
    raw: PathBuf,
    /// Forge's card scripts, read the first time a deck is played; why there are none if not.
    scripts: Arc<OnceLock<Result<ForgeDb, String>>>,
}

impl Goldfish {
    /// `configured`: the config file's `[goldfish] forge_cards`.
    pub fn new(games: u32, paths: &Paths, configured: Option<&Path>) -> Self {
        let scripts = Arc::new(OnceLock::new());
        Goldfish { games, source: ForgeSource::find(configured), db: paths.forge_db(), raw: paths.forge_raw(), scripts }
    }

    /// Reading cards with these scripts (tests).
    #[cfg(test)]
    pub fn with_scripts(games: u32, scripts: ForgeDb) -> Self {
        Goldfish {
            games,
            source: ForgeSource::Download,
            db: PathBuf::new(),
            raw: PathBuf::new(),
            scripts: Arc::new(OnceLock::from(Ok(scripts))),
        }
    }

    /// Forge's card scripts, or why there are none (read on first use, which takes a moment).
    pub fn scripts(&self) -> Result<&ForgeDb, &str> {
        let loaded = self.scripts.get_or_init(|| match self.source.load(&self.db, &self.raw) {
            Ok(Some(db)) => Ok(db),
            Ok(None) => Err("no Forge card scripts yet (`edh-scout sync --forge` downloads them)".to_string()),
            Err(e) => Err(format!("Forge's card scripts couldn't be read: {e:#}")),
        });
        loaded.as_ref().map_err(String::as_str)
    }
}

/// The goldfish's games for `deck`, cards the database doesn't know left out; `None` when it's
/// off.
pub fn report(deck: &Deck, cards: &CardDb, goldfish: &Goldfish) -> Option<ManaReport> {
    (goldfish.games > 0).then(|| play(&deck.id, &resolved(deck, cards), goldfish))
}

/// The deck's cards, copied, so another thread can play them.
fn resolved(deck: &Deck, cards: &CardDb) -> Vec<(Card, u32, bool)> {
    deck.entries().filter_map(|(name, qty, commander)| cards.get(name).map(|c| (c.clone(), qty, commander))).collect()
}

fn play(id: &str, cards: &[(Card, u32, bool)], goldfish: &Goldfish) -> ManaReport {
    let scripts = goldfish.scripts().ok();
    let entries: Vec<DeckEntry> = cards
        .iter()
        .map(|(card, qty, commander)| DeckEntry {
            card,
            qty: *qty,
            commander: *commander,
            script: scripts.and_then(|db| db.get(&card.name)),
        })
        .collect();
    edh_goldfish::run(&entries, goldfish.games, edh_goldfish::seed_for(id))
}

struct Job {
    id: String,
    cards: Vec<(Card, u32, bool)>,
}

/// Plays decks on a thread of its own, the latest one asked for first, so moving through the
/// list never waits for it. Reports are kept per deck id for as long as the browser is open.
pub struct Background {
    games: u32,
    jobs: Sender<Job>,
    done: Receiver<(String, ManaReport)>,
    reports: HashMap<String, ManaReport>,
    asked: Option<String>,
}

impl Background {
    /// `None` for 0 games: the goldfish is off.
    pub fn start(goldfish: Goldfish) -> Option<Self> {
        let games = goldfish.games;
        if games == 0 {
            return None;
        }
        let (jobs, todo) = channel::<Job>();
        let (finished, done) = channel();
        std::thread::Builder::new()
            .name("edh-scout-goldfish".into())
            .spawn(move || {
                while let Ok(mut job) = todo.recv() {
                    // Only the deck asked for last matters; skip the ones passed on the way.
                    while let Ok(later) = todo.try_recv() {
                        job = later;
                    }
                    let report = play(&job.id, &job.cards, &goldfish);
                    if finished.send((job.id, report)).is_err() {
                        break;
                    }
                }
            })
            .expect("starting the goldfish thread");
        Some(Background { games, jobs, done, reports: HashMap::new(), asked: None })
    }

    pub fn games(&self) -> u32 {
        self.games
    }

    /// The deck's report, if it's been played.
    pub fn get(&self, id: &str) -> Option<&ManaReport> {
        self.reports.get(id)
    }

    /// Play `deck` next, unless it has been or is being played.
    pub fn want(&mut self, deck: &Deck, cards: &CardDb) {
        if self.reports.contains_key(&deck.id) || self.asked.as_deref() == Some(deck.id.as_str()) {
            return;
        }
        self.asked = Some(deck.id.clone());
        // The thread only ends when this does, so the send can't fail.
        let _ = self.jobs.send(Job { id: deck.id.clone(), cards: resolved(deck, cards) });
    }

    /// Take in finished reports. Returns whether any arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((id, report)) = self.done.try_recv() {
            self.reports.insert(id, report);
            any = true;
        }
        any
    }

    /// Wait for the deck asked for last (tests drive the browser without its event loop).
    #[cfg(test)]
    pub fn wait(&mut self) {
        while self.asked.as_ref().is_some_and(|id| !self.reports.contains_key(id)) {
            let (got, report) = self.done.recv().expect("the goldfish thread");
            self.reports.insert(got, report);
        }
    }
}
