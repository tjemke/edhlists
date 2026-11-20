//! A colour-aware mana goldfish: how a Commander deck develops when nobody interacts.
//!
//! Each game is solitaire. Cards are reduced to what matters for mana ([`profile`]): lands (with
//! their enters-tapped rules, fetches and bounce lands), mana rocks and dorks, land-ramp spells,
//! card draw, and everything else as a spell of its cost. Each turn the deck plays the land that
//! lets it spend the most, then casts greedily: ramp, the commander, draw, then the biggest
//! spells it can pay for, paying coloured pips from the sources that make them ([`pay`]).
//!
//! Not modelled: opponents, combat, tutors, card-specific engines beyond ramp and draw,
//! recasting the commander. So it says whether a deck's mana works, not how strong it is.
//!
//! A deck always gets the same report for the same seed ([`seed_for`] its id), however many
//! threads play the games.

mod game;
mod pay;
mod profile;

use edh_data::cards::Card;
use rayon::prelude::*;

/// One line of a decklist.
#[derive(Clone, Copy)]
pub struct DeckEntry<'a> {
    pub card: &'a Card,
    pub qty: u32,
    pub commander: bool,
}

/// Turns played per game.
pub const TURNS: usize = 10;

/// Below this share of games, a card stuck on colours isn't worth mentioning.
pub const STUCK_WORTH_SHOWING: f64 = 0.005;

/// What the games came to. Rates are shares of games, 0 to 1.
#[derive(Clone, Debug, PartialEq)]
pub struct ManaReport {
    pub games: u32,
    /// Games that took at least one mulligan.
    pub mulligan: f64,
    /// Games where a commander was cast by turn 4 (the first, for partners). `None` without one.
    pub commander_by_t4: Option<f64>,
    /// Games where the mana to cast the commander was there but not its colours, before it was
    /// cast.
    pub colour_delayed: Option<f64>,
    /// Average mana the deck can make at the end of turns 1 to 8: every land and rock, tapped or
    /// not.
    pub capacity: [f64; 8],
    /// Average mana value cast by the end of turn 8.
    pub spent_by_t8: f64,
    /// Cards that sat in hand (or the command zone) with enough mana but the wrong colours to cast
    /// them, with the share of games it happened in by turn 8; most often first.
    pub stuck: Vec<(String, f64)>,
}

impl ManaReport {
    /// Mana on turn 4 beyond the 4 of playing a land a turn.
    pub fn ahead_on_t4(&self) -> f64 {
        self.capacity[3] - 4.0
    }

    /// The cards stuck on colours often enough to mention ([`STUCK_WORTH_SHOWING`]).
    pub fn stuck_often(&self) -> impl Iterator<Item = &(String, f64)> {
        self.stuck.iter().filter(|(_, share)| *share > STUCK_WORTH_SHOWING)
    }
}

/// A stable seed for a deck: the same id always plays the same games.
pub fn seed_for(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Play `games` games of the deck and sum them up.
pub fn run(entries: &[DeckEntry], games: u32, seed: u64) -> ManaReport {
    let deck = game::Deck::new(entries);
    let totals = (0..games)
        .into_par_iter()
        .fold(
            || game::Totals::new(deck.profiles.len()),
            |mut t, i| {
                t.add(&deck.play(game::Rng::new(seed ^ u64::from(i).wrapping_mul(0x9e37_79b9_7f4a_7c15))));
                t
            },
        )
        .reduce(|| game::Totals::new(deck.profiles.len()), game::Totals::merge);
    totals.report(&deck)
}

#[cfg(test)]
mod tests;
