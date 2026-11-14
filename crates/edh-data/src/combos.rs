//! Commander Spellbook combos, from the full variants dump downloaded by `sync`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cards::CardDb;
use crate::storage::{open_maybe_gz, read_json_gz, write_json_gz};

pub const COMBOS_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComboCard {
    pub name: String,
    /// Spellbook zone letters: B(attlefield), H(and), G(raveyard), C(ommand zone), L(ibrary), E(xile).
    pub zones: String,
    #[serde(default)]
    pub must_be_commander: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Requirement {
    pub template: u32,
    pub quantity: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Combo {
    pub id: String,
    pub cards: Vec<ComboCard>,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    /// Feature ids; names live in [`ComboDb::features`].
    pub produces: Vec<u32>,
    pub mana_value_needed: f64,
    #[serde(default)]
    pub notable_prerequisites: String,
    /// Spellbook's own bracket tag: E(xhibition), C(ore), O(ddball), P(owerful), S(picy), R(uthless), B(anned).
    pub bracket_tag: String,
    #[serde(default)]
    pub popularity: Option<u64>,
}

impl Combo {
    pub fn card_names(&self) -> impl Iterator<Item = &str> {
        self.cards.iter().map(|c| c.name.as_str())
    }
}

#[derive(Serialize, Deserialize)]
pub struct ComboDb {
    pub schema: u32,
    pub built: u64,
    pub source_timestamp: String,
    pub features: BTreeMap<u32, String>,
    pub combos: Vec<Combo>,
    /// Each combo under one of its cards: the one that appears in the fewest combos.
    #[serde(skip)]
    by_card: HashMap<String, Vec<usize>>,
}

impl ComboDb {
    pub fn new(source_timestamp: String, features: BTreeMap<u32, String>, combos: Vec<Combo>) -> Self {
        let built = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut db = Self { schema: COMBOS_SCHEMA, built, source_timestamp, features, combos, by_card: HashMap::new() };
        db.reindex();
        db
    }

    fn reindex(&mut self) {
        // A combo is only included when all its cards are, so it's enough to look at it once,
        // under its card that appears in the fewest combos. Filing it under every card made a
        // deck with Sol Ring re-check thousands of combos it can't have.
        let mut uses: HashMap<&str, usize> = HashMap::new();
        for c in &self.combos {
            for name in c.card_names().collect::<HashSet<_>>() {
                *uses.entry(name).or_default() += 1;
            }
        }
        let mut by_card: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in self.combos.iter().enumerate() {
            if let Some(rarest) = c.card_names().min_by_key(|n| (uses[n], *n)) {
                by_card.entry(rarest.to_string()).or_default().push(i);
            }
        }
        self.by_card = by_card;
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut db: ComboDb = read_json_gz(path)?;
        db.reindex();
        Ok(db)
    }

    pub fn empty() -> Self {
        Self::new(String::new(), BTreeMap::new(), Vec::new())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_gz(path, self)
    }

    pub fn age_days(&self) -> u64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        now.saturating_sub(self.built) / 86_400
    }

    pub fn feature_name(&self, id: u32) -> &str {
        self.features.get(&id).map(String::as_str).unwrap_or("?")
    }

    /// Combos whose every card is in the deck — what Spellbook's find-my-combos calls "included".
    /// `deck` and `commanders` hold canonical card names; templates are not checked, exactly like
    /// the site, and cards that must be the commander have to be one.
    pub fn included(&self, deck: &HashSet<String>, commanders: &HashSet<String>) -> Vec<&Combo> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for name in deck {
            for &i in self.by_card.get(name).into_iter().flatten() {
                if !seen.insert(i) {
                    continue;
                }
                let c = &self.combos[i];
                let ok = c
                    .cards
                    .iter()
                    .all(|cc| deck.contains(&cc.name) && (!cc.must_be_commander || commanders.contains(&cc.name)));
                if ok {
                    out.push(c);
                }
            }
        }
        out.sort_by(|a, b| a.cards.len().cmp(&b.cards.len()).then_with(|| a.id.cmp(&b.id)));
        out
    }

    /// [`ComboDb::included`] the slow way, checking every combo (for tests).
    #[cfg(test)]
    fn included_by_scanning(&self, deck: &HashSet<String>, commanders: &HashSet<String>) -> Vec<&Combo> {
        let mut out: Vec<&Combo> = self
            .combos
            .iter()
            .filter(|c| {
                c.cards
                    .iter()
                    .all(|cc| deck.contains(&cc.name) && (!cc.must_be_commander || commanders.contains(&cc.name)))
            })
            .collect();
        out.sort_by(|a, b| a.cards.len().cmp(&b.cards.len()).then_with(|| a.id.cmp(&b.id)));
        out
    }
}

// --- building from the Spellbook dump -------------------------------------------------------------

#[derive(Deserialize)]
struct RawDump {
    #[serde(default)]
    timestamp: String,
    variants: Vec<RawVariant>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawVariant {
    id: String,
    status: String,
    uses: Vec<RawUse>,
    #[serde(default)]
    requires: Vec<RawRequire>,
    #[serde(default)]
    produces: Vec<RawProduce>,
    #[serde(default)]
    mana_value_needed: Option<f64>,
    #[serde(default)]
    notable_prerequisites: Option<String>,
    #[serde(default)]
    bracket_tag: Option<String>,
    #[serde(default)]
    popularity: Option<u64>,
    #[serde(default)]
    legalities: HashMap<String, bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUse {
    card: RawNamed,
    #[serde(default)]
    zone_locations: Vec<String>,
    #[serde(default)]
    must_be_commander: bool,
}

#[derive(Deserialize)]
struct RawNamed {
    name: String,
}

#[derive(Deserialize)]
struct RawRequire {
    template: RawTemplate,
    #[serde(default)]
    quantity: Option<u32>,
}

#[derive(Deserialize)]
struct RawTemplate {
    id: u32,
}

#[derive(Deserialize)]
struct RawProduce {
    feature: RawFeature,
}

#[derive(Deserialize)]
struct RawFeature {
    id: u32,
    name: String,
}

pub fn build_from_dump(path: &Path, cards: Option<&CardDb>) -> Result<ComboDb> {
    let reader = open_maybe_gz(path)?;
    let dump: RawDump = serde_json::from_reader(reader).context("parsing the Commander Spellbook dump")?;
    let mut features = BTreeMap::new();
    let mut combos = Vec::with_capacity(dump.variants.len());
    let canon = |n: &str| -> String {
        cards.and_then(|db| db.canonical(n)).map(str::to_string).unwrap_or_else(|| n.to_string())
    };
    for v in dump.variants {
        // find-my-combos only ever returns OK variants, and the site only considers commander-legal ones.
        if v.status != "OK" || v.legalities.get("commander") == Some(&false) {
            continue;
        }
        let produces = v
            .produces
            .into_iter()
            .map(|p| {
                features.entry(p.feature.id).or_insert(p.feature.name);
                p.feature.id
            })
            .collect();
        combos.push(Combo {
            id: v.id,
            cards: v
                .uses
                .into_iter()
                .map(|u| ComboCard {
                    name: canon(&u.card.name),
                    zones: u.zone_locations.concat(),
                    must_be_commander: u.must_be_commander,
                })
                .collect(),
            requires: v
                .requires
                .into_iter()
                .map(|r| Requirement { template: r.template.id, quantity: r.quantity.unwrap_or(1) })
                .collect(),
            produces,
            mana_value_needed: v.mana_value_needed.unwrap_or(0.0),
            notable_prerequisites: v.notable_prerequisites.unwrap_or_default(),
            bracket_tag: v.bracket_tag.unwrap_or_else(|| "R".into()),
            popularity: v.popularity,
        });
    }
    Ok(ComboDb::new(dump.timestamp, features, combos))
}

pub fn bracket_tag_name(tag: &str) -> &'static str {
    match tag {
        "E" => "Exhibition",
        "C" => "Core",
        "O" => "Oddball",
        "P" => "Powerful",
        "S" => "Spicy",
        "R" => "Ruthless",
        "B" => "Banned",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(id: &str, cards: &[(&str, bool)]) -> Combo {
        Combo {
            id: id.into(),
            cards: cards
                .iter()
                .map(|(n, cmdr)| ComboCard { name: n.to_string(), zones: "B".into(), must_be_commander: *cmdr })
                .collect(),
            requires: vec![],
            produces: vec![1],
            mana_value_needed: 0.0,
            notable_prerequisites: String::new(),
            bracket_tag: "S".into(),
            popularity: None,
        }
    }

    #[test]
    fn included_needs_every_card_and_commander_roles() {
        let db = ComboDb::new(
            String::new(),
            BTreeMap::from([(1, "Infinite mana".to_string())]),
            vec![
                combo("a", &[("A", false), ("B", false)]),
                combo("b", &[("A", true), ("C", false)]),
                combo("c", &[("A", false), ("D", false)]),
            ],
        );
        let deck: HashSet<String> = ["A", "B", "C"].iter().map(|s| s.to_string()).collect();
        let none = HashSet::new();
        let ids: Vec<_> = db.included(&deck, &none).iter().map(|c| c.id.clone()).collect();
        assert_eq!(ids, vec!["a"]);
        let cmdr: HashSet<String> = ["A".to_string()].into();
        let ids: Vec<_> = db.included(&deck, &cmdr).iter().map(|c| c.id.clone()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn included_finds_what_checking_every_combo_finds() {
        // "Common" is in every combo; each other card in only a few. Combos are filed under their
        // rarest card, so decks with and without that card must still get exactly the combos a
        // full scan would find.
        let mut combos = Vec::new();
        for i in 0..40 {
            let a = format!("Rare {}", i % 7);
            let b = format!("Rare {}", (i * 3 + 1) % 11);
            combos.push(combo(&format!("c{i}"), &[("Common", i % 13 == 0), (&a, false), (&b, false)]));
        }
        combos.push(combo("pair", &[("Common", false), ("Rare 3", false)]));
        let db = ComboDb::new(String::new(), BTreeMap::new(), combos);
        let names = |v: &[&str]| -> HashSet<String> { v.iter().map(|s| s.to_string()).collect() };
        let ids = |v: Vec<&Combo>| -> Vec<String> { v.into_iter().map(|c| c.id.clone()).collect() };
        let decks = [
            names(&["Common", "Rare 1", "Rare 3", "Rare 4", "Rare 6"]),
            names(&["Common", "Rare 0", "Rare 1", "Rare 2", "Rare 3", "Rare 5", "Rare 7", "Rare 10"]),
            names(&["Rare 0", "Rare 1", "Rare 2", "Rare 3"]),
            names(&["Common"]),
        ];
        for commanders in [HashSet::new(), names(&["Common"])] {
            for deck in &decks {
                let found = ids(db.included(deck, &commanders));
                assert_eq!(found, ids(db.included_by_scanning(deck, &commanders)), "{deck:?}");
            }
        }
        assert!(ids(db.included(&decks[0], &HashSet::new())).contains(&"pair".to_string()));
        assert!(db.included(&decks[3], &HashSet::new()).is_empty());
    }
}
