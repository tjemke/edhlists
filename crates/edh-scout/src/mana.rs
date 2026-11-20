//! How a deck's mana develops: the mana goldfish ([`edh_goldfish`]) run on a deck, at once or (in
//! the browser) in the background.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use edh_data::cards::{Card, CardDb};
use edh_data::deck::Deck;
use edh_goldfish::{DeckEntry, ManaReport};

/// The goldfish's games for `deck`, cards the database doesn't know left out; `None` for 0 games.
pub fn report(deck: &Deck, cards: &CardDb, games: u32) -> Option<ManaReport> {
    (games > 0).then(|| play(&deck.id, &resolved(deck, cards), games))
}

/// The deck's cards, copied, so another thread can play them.
fn resolved(deck: &Deck, cards: &CardDb) -> Vec<(Card, u32, bool)> {
    deck.entries().filter_map(|(name, qty, commander)| cards.get(name).map(|c| (c.clone(), qty, commander))).collect()
}

fn play(id: &str, cards: &[(Card, u32, bool)], games: u32) -> ManaReport {
    let entries: Vec<DeckEntry> =
        cards.iter().map(|(card, qty, commander)| DeckEntry { card, qty: *qty, commander: *commander }).collect();
    edh_goldfish::run(&entries, games, edh_goldfish::seed_for(id))
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
    pub fn start(games: u32) -> Option<Self> {
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
                    let report = play(&job.id, &job.cards, games);
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
