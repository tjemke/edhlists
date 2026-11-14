//! Everything we know about one deck: EDHPowerLevel score and bracket, structure, synergy, salt, combos.

use std::collections::{HashMap, HashSet};

use edh_data::cards::{Card, CardDb};
use edh_data::combos::{self as spellbook, ComboDb, bracket_tag_name};
use edh_data::deck::Deck;
use edh_data::edhrec::{CommanderPage, EdhrecCombo};
use edh_data::text::{fold, front_face};
use edhpl::{BracketReport, PowerLevel};
use serde::{Deserialize, Serialize};

use crate::filters::Matched;
use crate::roles::{self, Entry, Structure};

/// Shared, per-commander inputs for analysing many decks.
pub struct Context<'a> {
    pub cards: &'a CardDb,
    pub combos: &'a ComboDb,
    pub commander: Option<&'a CommanderPage>,
    /// Top-salt cards, by front-face name.
    pub salt: &'a HashMap<String, f64>,
    pub edhrec_combos: &'a [EdhrecCombo],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Synergy {
    /// Mean EDHREC synergy over the deck's nonland cards (unlisted cards count as 0).
    pub average: f64,
    /// Cards notably more common with this commander than elsewhere (synergy >= 0.15).
    pub signature: Vec<(String, f64)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComboInfo {
    pub cards: Vec<String>,
    pub results: Vec<String>,
    /// Commander Spellbook's tag (Exhibition ... Ruthless).
    pub tag: String,
    /// EDHREC community vote on which bracket the combo belongs in, if anyone voted.
    pub vote: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Analysis {
    pub power: PowerLevel,
    pub bracket: BracketReport,
    pub structure: Structure,
    pub synergy: Option<Synergy>,
    pub salt: Option<f64>,
    pub salty_cards: Vec<(String, f64)>,
    pub combos: Vec<ComboInfo>,
    /// Names the card database doesn't know; they are left out of every score.
    pub unknown: Vec<String>,
    pub card_count: u32,
    /// Per `-T` card filter: how many of the deck's cards count, and which.
    #[serde(default)]
    pub matches: Vec<Matched>,
}

/// What the deck list, the ranking and the deck database keep for every deck: a few numbers.
/// The rest of an [`Analysis`] (reasons, card lists, combos) is worked out again, in about a
/// millisecond, when a deck is opened or exported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    /// EDHPowerLevel's 0-10 power level.
    pub power: f64,
    pub bracket: u8,
    /// The bracket EDHPowerLevel's rules alone set.
    pub floor: u8,
    /// Closeness to the deckbuilding template, 0-1.
    pub structure: f64,
    /// Average EDHREC synergy with the commander, when its page was there to compare with.
    pub synergy: Option<f64>,
    /// EDHREC's deck salt.
    pub salt: Option<f64>,
}

impl Score {
    pub fn of(a: &Analysis) -> Self {
        Self {
            power: a.power.level,
            bracket: a.bracket.bracket,
            floor: a.bracket.floor,
            structure: a.structure.score,
            synergy: a.synergy.as_ref().map(|s| s.average),
            salt: a.salt,
        }
    }

    /// The power level as EDHPowerLevel shows it ("6.2", "10+").
    pub fn power_label(&self) -> String {
        if self.power >= 10.0 { "10+".into() } else { format!("{:.1}", self.power) }
    }
}

/// A card record in the shape EDHPowerLevel's scorer takes.
pub fn edhpl_card(card: &Card, quantity: u32, is_commander: bool) -> edhpl::DeckCard {
    edhpl::DeckCard {
        name: card.name.clone(),
        quantity,
        is_commander,
        cmc: card.mv,
        price: card.usd,
        edhrec_rank: card.rank,
        reserved: card.reserved,
        colors: card.colors.clone(),
        layout: card.layout.clone(),
        type_line: card.type_line.clone(),
        oracle_text: card.oracle.clone(),
        game_changer: card.game_changer,
    }
}

/// A Spellbook combo in the shape EDHPowerLevel's bracket rules take.
pub fn edhpl_combo(c: &spellbook::Combo) -> edhpl::Combo {
    edhpl::Combo {
        id: c.id.clone(),
        mv: c.mana_value_needed,
        cards: c
            .cards
            .iter()
            .map(|cc| edhpl::ComboCard { name: cc.name.clone(), zone: cc.zones.chars().map(String::from).collect() })
            .collect(),
        produces: c.produces.iter().map(|&id| edhpl::Feature { id }).collect(),
        requires: c
            .requires
            .iter()
            .map(|r| edhpl::Requirement { template: edhpl::Template { id: r.template }, quantity: r.quantity })
            .collect(),
        notable_prerequisites: c.notable_prerequisites.clone(),
    }
}

/// Resolve a deck against the card DB: known entries, plus the names it doesn't know.
pub fn resolve<'a>(deck: &Deck, cards: &'a CardDb) -> (Vec<Entry<'a>>, Vec<String>) {
    let mut unknown = Vec::new();
    let mut entries = Vec::with_capacity(deck.cards.len() + 2);
    for (name, qty, commander) in deck.entries() {
        match cards.get(name) {
            Some(card) => entries.push(Entry { card, qty, commander }),
            None => unknown.push(name.to_string()),
        }
    }
    (entries, unknown)
}

/// The Spellbook combos whose cards are all in the deck.
pub fn included_combos<'a>(entries: &[Entry], combos: &'a ComboDb) -> Vec<&'a spellbook::Combo> {
    let names: HashSet<String> = entries.iter().map(|e| e.card.name.clone()).collect();
    let commanders: HashSet<String> = entries.iter().filter(|e| e.commander).map(|e| e.card.name.clone()).collect();
    combos.included(&names, &commanders)
}

fn combo_key<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut v: Vec<String> = names.map(|n| fold(front_face(n))).collect();
    v.sort();
    v
}

pub fn analyze(deck: &Deck, ctx: &Context) -> Analysis {
    let (entries, unknown) = resolve(deck, ctx.cards);
    let included = included_combos(&entries, ctx.combos);

    let pl_cards: Vec<edhpl::DeckCard> = entries.iter().map(|e| edhpl_card(e.card, e.qty, e.commander)).collect();
    let pl_combos: Vec<edhpl::Combo> = included.iter().map(|c| edhpl_combo(c)).collect();
    let power = edhpl::power_level(&pl_cards);
    let bracket = edhpl::bracket(&pl_cards, &pl_combos, power.level);
    let structure = roles::structure(&entries);

    let synergy = ctx.commander.map(|page| {
        let mut sum = 0.0;
        let mut n = 0u32;
        let mut signature = Vec::new();
        for e in entries.iter().filter(|e| !e.commander && !e.card.is_land()) {
            let syn = page.cards.get(front_face(&e.card.name)).map(|s| s.synergy).unwrap_or(0.0);
            sum += syn * f64::from(e.qty);
            n += e.qty;
            if syn >= 0.15 {
                signature.push((front_face(&e.card.name).to_string(), syn));
            }
        }
        signature.sort_by(|a, b| b.1.total_cmp(&a.1));
        Synergy { average: if n > 0 { sum / f64::from(n) } else { 0.0 }, signature }
    });

    let mut salty_cards: Vec<(String, f64)> = entries
        .iter()
        .filter_map(|e| {
            let face = front_face(&e.card.name);
            ctx.salt.get(face).or_else(|| ctx.salt.get(&e.card.name)).map(|s| (face.to_string(), *s))
        })
        .collect();
    salty_cards.sort_by(|a, b| b.1.total_cmp(&a.1));

    let votes: HashMap<Vec<String>, &EdhrecCombo> =
        ctx.edhrec_combos.iter().map(|c| (combo_key(c.cards.iter().map(String::as_str)), c)).collect();
    let combos = included
        .iter()
        .map(|c| ComboInfo {
            cards: c.cards.iter().map(|cc| front_face(&cc.name).to_string()).collect(),
            results: c.produces.iter().take(2).map(|id| ctx.combos.feature_name(*id).to_string()).collect(),
            tag: bracket_tag_name(&c.bracket_tag).to_string(),
            vote: votes.get(&combo_key(c.card_names())).and_then(|v| v.vote_bracket.clone()),
        })
        .collect();

    Analysis {
        power,
        bracket,
        structure,
        synergy,
        salt: deck.meta.salt,
        salty_cards,
        combos,
        unknown,
        card_count: deck.card_count(),
        matches: Vec::new(),
    }
}
