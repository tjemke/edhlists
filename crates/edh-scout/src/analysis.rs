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

/// Bump when a change to scoring (here, in `roles.rs` or in `edhpl`) changes any deck's [`Score`]:
/// scores the deck database kept from an older version are then worked out again. The
/// `scores_are_pinned_to_the_scorer_version` test fails when a change goes unnoticed.
pub const SCORER_VERSION: u32 = 1;

/// What a [`Score`] depends on besides the deck itself: the scorer and the card and combo
/// databases (prices, Game Changers, Tagger roles and combos all move with `sync`).
pub fn score_epoch(cards: &CardDb, combos: &ComboDb) -> i64 {
    fingerprint([u64::from(SCORER_VERSION), cards.built, combos.built].iter().flat_map(|n| n.to_le_bytes()))
}

/// What synergy depends on besides the deck: the commander's EDHREC page, refreshed weekly
/// (0 without one).
pub fn synergy_source(page: Option<&CommanderPage>) -> i64 {
    let Some(page) = page else { return 0 };
    let mut cards: Vec<(&String, f64)> = page.cards.iter().map(|(n, s)| (n, s.synergy)).collect();
    cards.sort_by(|a, b| a.0.cmp(b.0));
    let bytes = page.num_decks.to_le_bytes().into_iter().chain(
        cards
            .into_iter()
            .flat_map(|(n, syn)| n.bytes().chain([0]).chain(syn.to_bits().to_le_bytes()).collect::<Vec<u8>>()),
    );
    fingerprint(bytes)
}

/// FNV-1a over `bytes`: stable across runs and builds, unlike the standard library's hasher.
fn fingerprint(bytes: impl IntoIterator<Item = u8>) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    // Never 0, which stands for "no commander page".
    (h | 1) as i64
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

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, mv: f64, usd: f64, rank: u32, type_line: &str, oracle: &str) -> Card {
        Card {
            name: name.into(),
            mv,
            usd,
            rank: Some(rank),
            type_line: type_line.into(),
            oracle: vec![oracle.into()],
            layout: "normal".into(),
            colors: Some(vec!["G".into()]),
            identity: vec!["G".into()],
            ..Default::default()
        }
    }

    /// If this fails after a change to scoring, bump [`SCORER_VERSION`] (so stored scores are
    /// worked out again) and update the numbers here.
    #[test]
    fn scores_are_pinned_to_the_scorer_version() {
        let mut ramp =
            card("Cultivate", 3.0, 0.4, 120, "Sorcery", "Search your library for up to two basic land cards");
        ramp.roles = edh_data::tags::RAMP;
        ramp.tagged = true;
        let mut gc = card("Craterhoof Behemoth", 8.0, 55.0, 900, "Creature — Beast", "Haste");
        gc.game_changer = true;
        let cards = CardDb::new(vec![
            card("Omnath, Locus of Mana", 3.0, 1.5, 2000, "Legendary Creature — Elemental", "Green mana doesn't empty"),
            ramp,
            gc,
            card("Llanowar Elves", 1.0, 0.3, 80, "Creature — Elf Druid", "{T}: Add {G}."),
            card("Time Warp", 5.0, 9.0, 600, "Sorcery", "Target player takes an extra turn after this one."),
            card("Forest", 0.0, 0.1, 1, "Basic Land — Forest", ""),
        ]);
        let combos = ComboDb::empty();
        let salt = HashMap::new();
        let ctx = Context { cards: &cards, combos: &combos, commander: None, salt: &salt, edhrec_combos: &[] };
        let deck = Deck {
            commanders: vec!["Omnath, Locus of Mana".into()],
            cards: [
                ("Cultivate".to_string(), 1),
                ("Craterhoof Behemoth".to_string(), 1),
                ("Llanowar Elves".to_string(), 1),
                ("Time Warp".to_string(), 1),
                ("Forest".to_string(), 30),
            ]
            .into(),
            ..Default::default()
        };
        let s = Score::of(&analyze(&deck, &ctx));
        assert_eq!(SCORER_VERSION, 1, "update the pinned score below along with the version");
        let pinned = (0.365_377_042_483_660_15, 4, 4, 0.066_666_666_666_666_65);
        assert!(
            (s.power - pinned.0).abs() < 1e-9
                && s.bracket == pinned.1
                && s.floor == pinned.2
                && (s.structure - pinned.3).abs() < 1e-9,
            "scores changed: bump SCORER_VERSION and pin the new score: {s:?}"
        );
    }

    #[test]
    fn epochs_and_synergy_sources_are_stable_and_tell_inputs_apart() {
        use edh_data::edhrec::CardStat;
        let page = |syn: f64| CommanderPage {
            num_decks: 10,
            commander_salt: None,
            cards: HashMap::from([("Sol Ring".to_string(), CardStat { synergy: syn, ..Default::default() })]),
        };
        assert_eq!(synergy_source(None), 0);
        assert_eq!(synergy_source(Some(&page(0.1))), synergy_source(Some(&page(0.1))));
        assert_ne!(synergy_source(Some(&page(0.1))), synergy_source(Some(&page(0.2))));
        assert_ne!(synergy_source(Some(&page(0.1))), 0);
        let (cards, combos) = (CardDb::new(vec![]), ComboDb::empty());
        assert_eq!(score_epoch(&cards, &combos), score_epoch(&cards, &combos));
    }
}
