//! How far to trust a deck's numbers: what the goldfish leaves out or couldn't read, and whether
//! that's enough to move them.
//!
//! A card the goldfish doesn't play that would make mana early (ramp of mana value 4 or less by
//! Scryfall Tagger) is in hand by turn 4 in about one game in ten. A mana engine (Crypt Ghast,
//! Explore) then adds about a mana every turn, so each moves mana on turn 4 by about 0.1, and
//! two reach the 0.2 the numbers are shown to. One-shot mana (Treasure, rituals), and a source
//! counted as making one mana though it makes more as the board grows (Priest of Titania), are
//! worth half that; whatever else Tagger calls ramp (a land search that needs an attack, a free
//! spell) a quarter. A commander's own ramp is there every game from the turn it's cast. A mana
//! source whose script couldn't be read is a guess either way.

use edh_data::tags::Role;

use crate::game::Deck;
use crate::profile::{Effect, Fidelity, LeftOut, Profile};

/// Ramp at or under this mana value counts as early.
const EARLY: u32 = 4;
/// Weight in doubt from which the numbers are rough, and from which they're poor.
const ROUGH_AT: f64 = 2.0;
const POOR_AT: f64 = 5.0;

/// How far a deck's numbers can be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Within about 0.2 mana on turn 4 of what the deck does.
    Good,
    /// Likely low by more than that.
    Rough,
    /// Clearly low: much of the deck's early ramp isn't played, or its commander's isn't.
    Poor,
}

/// What makes a deck's numbers doubtful, and how much.
#[derive(Clone, Debug, PartialEq)]
pub struct Doubts {
    pub confidence: Confidence,
    /// Commanders whose own ramp the goldfish doesn't play (Urza, Prosper).
    pub commander_ramp: Vec<String>,
    /// Early mana engines in the 99 it doesn't play ([`EARLY`]): Crypt Ghast, Explore.
    pub engines: Vec<String>,
    /// Early one-shot mana it doesn't play: Treasure makers, rituals.
    pub one_shot: Vec<String>,
    /// Other early ramp it doesn't play, by Tagger.
    pub other_ramp: Vec<String>,
    /// Early sources it counts as making one mana, or none for others, though they make more as
    /// the board grows ([`Profile::undercounted`]): Priest of Titania, Marwyn.
    pub growing: Vec<String>,
    /// Mana sources whose Forge script it couldn't read (read from oracle text instead), or that
    /// have none.
    pub unread: Vec<String>,
    /// Whether it had Forge's scripts at all (any card was read from one; basic lands have
    /// them); without them every card is read from oracle text.
    pub scripts: bool,
}

/// Does the card make or find mana, by what the goldfish read or what Tagger says?
fn mana_source(p: &Profile) -> bool {
    !p.lands.is_empty()
        || matches!(p.spell.as_ref().map(|s| &s.effect), Some(Effect::Rock { .. } | Effect::LandRamp { .. }))
        || p.unplayed == Some(Role::Ramp)
}

pub(crate) fn doubts(deck: &Deck) -> Doubts {
    let scripts = deck.profiles.iter().any(|p| p.fidelity != Fidelity::NoScript);
    let mut d = Doubts {
        confidence: Confidence::Good,
        commander_ramp: Vec::new(),
        engines: Vec::new(),
        one_shot: Vec::new(),
        other_ramp: Vec::new(),
        growing: Vec::new(),
        unread: Vec::new(),
        scripts,
    };
    let mut early = Vec::new();
    let mut growing = Vec::new();
    for (i, p) in deck.profiles.iter().enumerate() {
        let mv = p.spell.as_ref().map_or(0, |s| s.mana_value());
        if p.undercounted && p.unplayed.is_none() && (mv <= EARLY || deck.is_commander(i)) {
            growing.push((!deck.is_commander(i), mv, p.name.clone()));
        }
        if p.unplayed == Some(Role::Ramp) {
            if deck.is_commander(i) {
                d.commander_ramp.push(p.name.clone());
            } else if mv <= EARLY {
                early.push((mv, p.left_out, p.name.clone()));
            }
        }
        let doubtful = match p.fidelity {
            Fidelity::Unreadable(_) => true,
            Fidelity::NoScript => scripts,
            _ => false,
        };
        if doubtful && mana_source(p) && p.unplayed.is_none() {
            d.unread.push(p.name.clone());
        }
    }
    early.sort();
    early.dedup();
    let mut weight = 0.0;
    for (_, kind, name) in early {
        let (list, w) = match kind {
            Some(LeftOut::Engine) => (&mut d.engines, 1.0),
            Some(LeftOut::OneShot) => (&mut d.one_shot, 0.5),
            Some(LeftOut::Other) => (&mut d.other_ramp, 0.25),
            // Read without a script: what it does is unknown.
            None => (&mut d.other_ramp, 0.5),
        };
        list.push(name);
        weight += w;
    }
    // The commander first: it's there every game, so it counts double.
    growing.sort();
    growing.dedup();
    for (not_commander, _, name) in growing {
        weight += if not_commander { 0.5 } else { 1.0 };
        d.growing.push(name);
    }
    d.unread.sort();
    d.unread.dedup();
    weight += d.unread.len() as f64;
    // Without scripts, every reading is the oracle reader's guess.
    if !scripts {
        weight += ROUGH_AT;
    }
    let commander = !d.commander_ramp.is_empty();
    d.confidence = if weight >= POOR_AT || (commander && weight >= ROUGH_AT) {
        Confidence::Poor
    } else if weight >= ROUGH_AT || commander {
        Confidence::Rough
    } else {
        Confidence::Good
    };
    d
}
