//! The Commander bracket, as the site's `bracketHelper` computes it.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{Combo, DeckCard, bracket_from_level};

static TURNS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(take an extra turn|target player takes an extra turn|target player takes [A-Za-z0-9_]* extra turns)")
        .unwrap()
});
// Translated from JavaScript: `{\d}` is literal there, `\w` is ASCII-only, and `^` means start of
// the text (no `m` flag) — so "Noncreature spells cost {1} more" only counts as a face's first line.
static DENIAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(^(noncreature|creature|red|white|blue|black|green) spells|^spells your opponents cast) cost \{\d\} more to cast",
        r"|each player sacrifices [A-Za-z0-9_]* (lands|land for each)",
        r"|destroy all (lands|islands|mountains|forests|swamps|plains)",
        r"|destroy all ([A-Za-z0-9_]*, )*and lands",
        r"|untap (only|more than) [A-Za-z0-9_]* (land|permanent|nonbasic)",
        r"|(islands|mountains|forests|swamps|plains|[A-Za-z0-9_]* lands) don't untap",
        r"|nonbasic lands are (mountains|islands)",
    ))
    .unwrap()
});
static TUTOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"search your library for (an*|up to [A-Za-z0-9_]*) (creature |artifact |enchantment |aura |equipment |",
        r"planeswalker |instant |sorcery |permanent |[A-Za-z0-9_]* creature |[A-Za-z0-9_]* permanent)*",
        r"(or [A-Za-z0-9_]* )*card",
    ))
    .unwrap()
});

const MASS_LAND_DENIAL: &[&str] = &[
    "Vorinclex, Voice of Hunger",
    "Hall of Gemstone",
    "Contamination",
    "Cataclysm",
    "Dimensional Breach",
    "Epicenter",
    "Global Ruin",
    "Hokori, Dust Drinker",
    "Razia's Purification",
    "Rising Waters",
    "Soulscour",
    "Sunder",
    "Apocalypse",
    "Bearer of the Heavens",
    "Conversion",
    "Glaciers",
    "Pox",
    "Death Cloud",
    "Tangle Wire",
    "Restore Balance",
    "Realm Razer",
    "Spreading Algae",
    "Numot, the Devastator",
    "Kudzu",
    "Demonic Hordes",
    "Urza's Sylex",
    "Infernal Darkness",
    "Trinisphere",
    "Worldfire",
    "Worldslayer",
    "Gilt-Leaf Archdruid",
    "Worldpurge",
    "Stasis",
];
const MLD_WHITELIST: &[&str] = &["Charitable Levy"];
const CHAINABLE_EXTRA_TURNS: &[&str] = &[
    "Time Warp",
    "Temporal Manipulation",
    "Walk the Aeons",
    "Capture of Jingzhou",
    "Expropriate",
    "Time Stretch",
    "Nexus of Fate",
    "Timestream Navigator",
    "Sage of Hours",
    "Lighthouse Chronologist",
    "Time Sieve",
    "Magosi, the Waterveil",
];
/// Spellbook feature ids that don't make a combo "game-defining" on their own (infinite mana etc.).
const PRODUCER_FEATURES: &[u32] = &[7, 4, 3, 6, 17, 77, 60, 79, 325, 110, 505, 1111, 227, 87, 547, 2742, 2617];
const EXCUSED_TEMPLATES: &[u32] = &[28];
static PREREQS: LazyLock<Vec<(Regex, Option<f64>)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"walking ballista has at least two \+1/\+1 counters on it\.").unwrap(), Some(4.0)),
        (
            Regex::new(
                r"you have a way to give (.+) (lifelink|indestructible|deathtouch|vigilance|persist|undying|trample|double strike|defender)\.",
            )
            .unwrap(),
            None,
        ),
    ]
});

/// Per-bracket maximums (index = bracket - 1) for each rule.
const MAX_TURNS: [usize; 5] = [0, 2, 3, 100, 100];
const MAX_DENIAL: [usize; 5] = [0, 0, 0, 100, 100];
const MAX_GAME_CHANGERS: [usize; 5] = [0, 0, 3, 100, 100];
const MAX_EARLY_COMBOS: [usize; 5] = [0, 0, 0, 100, 100];
const MAX_LATE_COMBOS: [usize; 5] = [0, 0, 100, 100, 100];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ComboHit {
    pub id: String,
    pub cards: Vec<String>,
    /// Mana needed including the battlefield pieces; > 7 makes the combo "late".
    pub mana: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BracketReport {
    /// What the site shows: max(rules floor, bracket implied by the power level).
    pub bracket: u8,
    /// The rules alone.
    pub floor: u8,
    /// The power level alone.
    pub from_level: u8,
    pub extra_turns: Vec<String>,
    pub extra_turns_chainable: Vec<String>,
    pub land_denial: Vec<String>,
    pub land_denial_listed: Vec<String>,
    pub game_changers: Vec<String>,
    pub early_combos: Vec<ComboHit>,
    pub late_combos: Vec<ComboHit>,
    /// Informational only: the site defines a tutor rule but never feeds it any matches.
    pub tutors: Vec<String>,
}

impl BracketReport {
    /// Human-readable reasons the rules floor is where it is.
    pub fn reasons(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.game_changers.is_empty() {
            out.push(format!("{} Game Changer(s)", self.game_changers.len()));
        }
        if !self.extra_turns.is_empty() {
            out.push(format!("{} extra-turn card(s)", self.extra_turns.len()));
        }
        if !self.extra_turns_chainable.is_empty() {
            out.push("chainable extra turns".into());
        }
        if !self.land_denial.is_empty() || !self.land_denial_listed.is_empty() {
            out.push("mass land denial".into());
        }
        if !self.early_combos.is_empty() {
            out.push(format!("{} early 2-card combo(s)", self.early_combos.len()));
        }
        if !self.late_combos.is_empty() {
            out.push(format!("{} late 2-card combo(s)", self.late_combos.len()));
        }
        out
    }
}

fn rule_level(matches: usize, maxes: &[usize; 5], restricted: usize, under_bracket: usize) -> usize {
    let mut h = 0;
    for (i, &m) in maxes.iter().enumerate() {
        if matches > m {
            h = i + 1;
        }
    }
    if restricted > 0 && h < under_bracket {
        h = under_bracket;
    }
    h
}

fn push_unique(v: &mut Vec<String>, name: &str) {
    if !v.iter().any(|x| x == name) {
        v.push(name.to_string());
    }
}

fn listed(list: &[&str], name: &str) -> bool {
    list.contains(&name) || list.contains(&name.split(" // ").next().unwrap_or(name))
}

/// Sort Spellbook's "included" combos into early and late 2-card combos, as the site does.
pub fn classify_combos(cards: &[DeckCard], included: &[Combo]) -> (Vec<ComboHit>, Vec<ComboHit>) {
    let (mut early, mut late) = (Vec::new(), Vec::new());
    for combo in included {
        let mut w = combo.cards.len() as u32;
        let la = combo.produces.iter().any(|f| !PRODUCER_FEATURES.contains(&f.id));
        let mut ba = combo.requires.is_empty();
        for r in &combo.requires {
            w += r.quantity;
            if !EXCUSED_TEMPLATES.contains(&r.template.id) {
                ba = true;
            }
            // quirk: runs after the line above, so a combo needing any template never counts
            if w < 3 {
                ba = false;
            }
        }
        let mut da = true;
        let mut ka = 0.0;
        if !combo.notable_prerequisites.is_empty() {
            for line in combo.notable_prerequisites.split('\n') {
                let lower = line.to_lowercase();
                for (re, add) in PREREQS.iter() {
                    if re.is_match(&lower) {
                        match add {
                            Some(a) => ka += a,
                            None => da = false,
                        }
                    }
                }
            }
        }
        if la && ba && da && w < 3 {
            let ya: f64 = combo
                .cards
                .iter()
                .filter(|c| c.zone.iter().any(|z| z == "B"))
                .map(|c| cards.iter().find(|d| d.name == c.name).map(DeckCard::effective_cmc).unwrap_or(0.0))
                .sum();
            let hit = ComboHit {
                id: combo.id.clone(),
                cards: combo.cards.iter().map(|c| c.name.clone()).collect(),
                mana: combo.mv + ya + ka,
            };
            if hit.mana > 7.0 { late.push(hit) } else { early.push(hit) }
        }
    }
    (early, late)
}

/// The bracket the site would show for this deck. `included` are the Commander Spellbook combos
/// whose cards are all in the deck (what Spellbook's find-my-combos calls "included").
pub fn bracket(cards: &[DeckCard], included: &[Combo], level: f64) -> BracketReport {
    let mut r = BracketReport::default();
    let mut seen = HashSet::new();
    for card in cards {
        if !seen.insert(card.name.as_str()) {
            continue;
        }
        for text in &card.oracle_text {
            let w = text.to_lowercase();
            if TURNS_RE.is_match(&w) {
                push_unique(&mut r.extra_turns, &card.name);
            }
            if DENIAL_RE.is_match(&w) && !listed(MLD_WHITELIST, &card.name) {
                push_unique(&mut r.land_denial, &card.name);
            }
            if card.game_changer {
                push_unique(&mut r.game_changers, &card.name);
            }
            if listed(CHAINABLE_EXTRA_TURNS, &card.name) {
                push_unique(&mut r.extra_turns_chainable, &card.name);
            }
            if listed(MASS_LAND_DENIAL, &card.name) {
                push_unique(&mut r.land_denial_listed, &card.name);
            }
            if !card.is_land() && TUTOR_RE.is_match(&w) {
                push_unique(&mut r.tutors, &card.name);
            }
        }
    }
    let (early, late) = classify_combos(cards, included);
    r.early_combos = early;
    r.late_combos = late;
    let na = [
        rule_level(r.early_combos.len(), &MAX_EARLY_COMBOS, 0, 0),
        rule_level(r.late_combos.len(), &MAX_LATE_COMBOS, 0, 0),
        rule_level(r.extra_turns.len(), &MAX_TURNS, r.extra_turns_chainable.len(), 3),
        rule_level(r.land_denial.len(), &MAX_DENIAL, r.land_denial_listed.len(), 3),
        rule_level(r.game_changers.len(), &MAX_GAME_CHANGERS, 0, 0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    r.floor = na as u8 + 1;
    r.from_level = bracket_from_level(level);
    r.bracket = r.from_level.max(r.floor);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denial_regex_matches_like_javascript() {
        assert!(DENIAL_RE.is_match("noncreature spells cost {1} more to cast."));
        assert!(!DENIAL_RE.is_match("first strike\nnoncreature spells cost {1} more to cast."));
        assert!(DENIAL_RE.is_match("destroy all lands."));
        assert!(DENIAL_RE.is_match(
            "as long as winter orb is untapped, players can't untap more than one land during their untap steps."
        ));
        assert!(TURNS_RE.is_match("take an extra turn after this one."));
    }

    #[test]
    fn rule_levels() {
        assert_eq!(rule_level(0, &MAX_GAME_CHANGERS, 0, 0), 0);
        assert_eq!(rule_level(1, &MAX_GAME_CHANGERS, 0, 0), 2);
        assert_eq!(rule_level(4, &MAX_GAME_CHANGERS, 0, 0), 3);
        assert_eq!(rule_level(1, &MAX_TURNS, 0, 3), 1);
        assert_eq!(rule_level(1, &MAX_TURNS, 1, 3), 3);
    }

    fn two_card(id: &str, a: &str, b: &str, produces: u32) -> Combo {
        Combo {
            id: id.into(),
            mv: 0.0,
            cards: [a, b].iter().map(|n| crate::ComboCard { name: n.to_string(), zone: vec!["B".into()] }).collect(),
            produces: vec![crate::Feature { id: produces }],
            requires: vec![],
            notable_prerequisites: String::new(),
        }
    }

    fn creature(name: &str, cmc: f64) -> DeckCard {
        DeckCard {
            name: name.into(),
            quantity: 1,
            cmc,
            layout: "normal".into(),
            type_line: "Creature".into(),
            ..Default::default()
        }
    }

    #[test]
    fn early_late_and_mana_only_combos() {
        let deck = vec![creature("A", 2.0), creature("B", 3.0), creature("C", 5.0), creature("D", 5.0)];
        let early = two_card("1", "A", "B", 999);
        let late = two_card("2", "C", "D", 999);
        let mana_only = two_card("3", "A", "C", 7); // feature 7 is in the producer list
        let (e, l) = classify_combos(&deck, &[early, late, mana_only]);
        assert_eq!(e.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["1"]);
        assert_eq!(l.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["2"]);
        let r = bracket(&deck, &[two_card("1", "A", "B", 999)], 6.0);
        assert_eq!((r.floor, r.from_level, r.bracket), (4, 2, 4));
    }
}
