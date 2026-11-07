//! The local card database, built from Scryfall's `oracle-cards` bulk file by `sync`.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::storage::{open_maybe_gz, read_json_gz, write_json_gz};
use crate::tags::RoleTags;
use crate::text::{fold, front_face};

/// Bump when [`Card`] changes shape; `sync` rebuilds from the kept raw bulk file without downloading.
pub const CARDS_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Card {
    pub name: String,
    #[serde(default)]
    pub oracle_id: String,
    /// Face names for multi-face cards (empty for single-faced ones).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flavor_name: Option<String>,
    pub mv: f64,
    /// Full type line; multi-face cards are joined with " // ".
    pub type_line: String,
    /// Oracle text per face (one entry for single-faced cards), exactly as EDHPowerLevel sees it.
    pub oracle: Vec<String>,
    pub layout: String,
    /// `None` when Scryfall has no colors at all; EDHPowerLevel treats that differently from `[]`.
    pub colors: Option<Vec<String>>,
    #[serde(default)]
    pub identity: Vec<String>,
    /// Cheapest non-foil USD price across real printings (what EDHPowerLevel prices cards at).
    pub usd: f64,
    pub rank: Option<u32>,
    #[serde(default)]
    pub reserved: bool,
    #[serde(default)]
    pub game_changer: bool,
    #[serde(default)]
    pub banned: bool,
    #[serde(default)]
    pub produces_mana: bool,
    /// Scryfall Tagger role bits ([`crate::tags`]): ramp, draw, removal, wipe.
    #[serde(default)]
    pub roles: u8,
    /// Whether Scryfall Tagger knows this card at all (if not, roles come from heuristics).
    #[serde(default)]
    pub tagged: bool,
}

impl Card {
    /// The front face's type, as EDHPowerLevel reads it ("Legendary Creature — Elf" -> "Legendary Creature").
    pub fn front_type(&self) -> &str {
        let front = self.type_line.split(" // ").next().unwrap_or("");
        front.split(" — ").next().unwrap_or(front)
    }
    pub fn is_land(&self) -> bool {
        self.front_type().contains("Land")
    }
    pub fn is_mdfc(&self) -> bool {
        self.layout == "modal_dfc"
    }
    pub fn is_basic(&self) -> bool {
        self.front_type().contains("Basic")
    }
    pub fn primary_type(&self) -> &'static str {
        let t = self.front_type();
        for (needle, label) in [
            ("Land", "Land"),
            ("Creature", "Creature"),
            ("Planeswalker", "Planeswalker"),
            ("Battle", "Battle"),
            ("Instant", "Instant"),
            ("Sorcery", "Sorcery"),
            ("Artifact", "Artifact"),
            ("Enchantment", "Enchantment"),
        ] {
            if t.contains(needle) {
                return label;
            }
        }
        "Other"
    }
    pub fn can_be_commander(&self) -> bool {
        let t = self.type_line.split(" // ").next().unwrap_or("");
        (t.contains("Legendary") && (t.contains("Creature") || t.contains("Background")))
            || self.oracle.iter().any(|o| o.contains("can be your commander"))
    }
    pub fn oracle_joined(&self) -> String {
        self.oracle.join("\n")
    }
}

#[derive(Serialize, Deserialize)]
pub struct CardDb {
    pub schema: u32,
    /// Unix seconds when this DB was built.
    pub built: u64,
    pub cards: Vec<Card>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

impl CardDb {
    pub fn new(cards: Vec<Card>) -> Self {
        let built = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut db = Self { schema: CARDS_SCHEMA, built, cards, index: HashMap::new() };
        db.reindex();
        db
    }

    fn reindex(&mut self) {
        let mut index = HashMap::with_capacity(self.cards.len() * 2);
        // Full names win over face names and flavor names when they collide.
        for (i, c) in self.cards.iter().enumerate() {
            index.insert(c.name.to_lowercase(), i);
            index.entry(fold(&c.name)).or_insert(i);
        }
        for (i, c) in self.cards.iter().enumerate() {
            for f in &c.faces {
                index.entry(f.to_lowercase()).or_insert(i);
                index.entry(fold(f)).or_insert(i);
            }
            if let Some(fl) = &c.flavor_name {
                index.entry(fl.to_lowercase()).or_insert(i);
            }
        }
        self.index = index;
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut db: CardDb = read_json_gz(path)?;
        db.reindex();
        Ok(db)
    }

    /// Load the pinned test fixture (plain JSON).
    pub fn from_json(text: &str) -> Result<Self> {
        let mut db: CardDb = serde_json::from_str(text)?;
        db.reindex();
        Ok(db)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_gz(path, self)
    }

    pub fn age_days(&self) -> u64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        now.saturating_sub(self.built) / 86_400
    }

    pub fn get(&self, name: &str) -> Option<&Card> {
        let key = name.trim().to_lowercase();
        let i = self
            .index
            .get(&key)
            .or_else(|| self.index.get(key.split(" // ").next().unwrap_or("")))
            .or_else(|| self.index.get(&fold(&key)))?;
        self.cards.get(*i)
    }

    /// Canonical (Scryfall) name for whatever a decklist called the card.
    pub fn canonical<'a>(&'a self, name: &str) -> Option<&'a str> {
        self.get(name).map(|c| c.name.as_str())
    }

    /// Every card that can lead a deck (legendary creatures, backgrounds, "can be your commander"),
    /// most popular first.
    pub fn commanders(&self) -> Vec<&Card> {
        let mut v: Vec<&Card> = self.cards.iter().filter(|c| c.can_be_commander() && !c.banned).collect();
        v.sort_by(|a, b| a.rank.unwrap_or(u32::MAX).cmp(&b.rank.unwrap_or(u32::MAX)).then(a.name.cmp(&b.name)));
        v
    }

    /// Commanders whose name contains every word of `query` (case, accents and punctuation
    /// ignored), most popular first.
    pub fn commander_candidates(&self, query: &str) -> Vec<&Card> {
        let terms = search_terms(query);
        self.commanders().into_iter().filter(|c| matches_terms(&search_key(&c.name), &terms)).collect()
    }

    /// Resolve a user-typed commander: an exact name, else the single candidate (or the single one
    /// whose name starts with the query). `Err` carries every candidate, possibly none.
    pub fn find_commander(&self, query: &str) -> std::result::Result<&Card, Vec<&Card>> {
        if let Some(c) = self.get(query).filter(|c| c.can_be_commander()) {
            return Ok(c);
        }
        let hits = self.commander_candidates(query);
        if hits.len() == 1 {
            return Ok(hits[0]);
        }
        let q = search_key(query);
        let prefix: Vec<&Card> = hits.iter().copied().filter(|c| search_key(&c.name).starts_with(q.trim())).collect();
        match prefix.as_slice() {
            [one] if !q.trim().is_empty() => Ok(one),
            _ => Err(hits),
        }
    }

    /// Restrict to the named cards (for building test fixtures).
    pub fn subset<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> CardDb {
        let mut cards: Vec<Card> = names.into_iter().filter_map(|n| self.get(n).cloned()).collect();
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        cards.dedup_by(|a, b| a.name == b.name);
        let mut db = CardDb::new(cards);
        db.built = self.built;
        db
    }
}

// --- building from Scryfall bulk ------------------------------------------------------------------

#[derive(Deserialize)]
struct RawFace {
    #[serde(default)]
    name: String,
    #[serde(default)]
    oracle_id: Option<String>,
    #[serde(default)]
    type_line: Option<String>,
    #[serde(default)]
    oracle_text: Option<String>,
    #[serde(default)]
    colors: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct RawPrices {
    usd: Option<String>,
    usd_foil: Option<String>,
    usd_etched: Option<String>,
}

#[derive(Deserialize)]
struct RawCard {
    name: String,
    #[serde(default)]
    oracle_id: Option<String>,
    #[serde(default)]
    digital: bool,
    #[serde(default)]
    layout: String,
    #[serde(default)]
    set_type: String,
    #[serde(default)]
    cmc: f64,
    #[serde(default)]
    type_line: Option<String>,
    #[serde(default)]
    oracle_text: Option<String>,
    #[serde(default)]
    colors: Option<Vec<String>>,
    #[serde(default)]
    color_identity: Vec<String>,
    #[serde(default)]
    card_faces: Vec<RawFace>,
    #[serde(default)]
    prices: RawPrices,
    #[serde(default)]
    edhrec_rank: Option<u32>,
    #[serde(default)]
    reserved: bool,
    #[serde(default)]
    game_changer: bool,
    #[serde(default)]
    legalities: HashMap<String, String>,
    #[serde(default)]
    produced_mana: Option<Vec<String>>,
    #[serde(default)]
    flavor_name: Option<String>,
}

const SKIP_LAYOUTS: &[&str] =
    &["token", "double_faced_token", "emblem", "art_series", "planar", "scheme", "vanguard", "reversible_card"];

fn convert(c: RawCard) -> Option<Card> {
    if SKIP_LAYOUTS.contains(&c.layout.as_str()) || c.set_type == "memorabilia" {
        return None;
    }
    let legal = c.legalities.get("commander").map(String::as_str).unwrap_or("not_legal");
    let multi = !c.card_faces.is_empty();
    let oracle = if multi {
        c.card_faces.iter().map(|f| f.oracle_text.clone().unwrap_or_default()).collect()
    } else {
        vec![c.oracle_text.clone().unwrap_or_default()]
    };
    let type_line = c.type_line.clone().unwrap_or_else(|| {
        c.card_faces.iter().map(|f| f.type_line.clone().unwrap_or_default()).collect::<Vec<_>>().join(" // ")
    });
    let colors = c.colors.clone().or_else(|| c.card_faces.first().and_then(|f| f.colors.clone()));
    let oracle_id = c
        .oracle_id
        .clone()
        .or_else(|| c.card_faces.first().and_then(|f| f.oracle_id.clone()))
        .unwrap_or_else(|| c.name.clone());
    Some(Card {
        oracle_id,
        faces: if multi { c.card_faces.iter().map(|f| f.name.clone()).collect() } else { vec![] },
        flavor_name: c.flavor_name,
        mv: c.cmc,
        type_line,
        oracle,
        layout: c.layout,
        colors,
        identity: c.color_identity,
        usd: 0.0,
        rank: c.edhrec_rank,
        reserved: c.reserved,
        game_changer: c.game_changer,
        banned: legal == "banned",
        produces_mana: c.produced_mana.is_some_and(|m| !m.is_empty()),
        roles: 0,
        tagged: false,
        name: c.name,
    })
}

fn parse_price(p: &Option<String>) -> Option<f64> {
    p.as_deref().and_then(|s| s.parse::<f64>().ok()).filter(|v| *v > 0.0)
}

fn keep_min(slot: &mut Option<f64>, v: Option<f64>) {
    if let Some(v) = v {
        *slot = Some(slot.map_or(v, |s| s.min(v)));
    }
}

/// Build the DB from a Scryfall bulk file: `default_cards` (every printing; prices become the
/// cheapest real printing, like EDHPowerLevel's) or `oracle_cards` (one printing per card).
/// Gzipped JSON Lines or a plain JSON array both work.
pub fn build_from_bulk(path: &Path, tags: Option<&RoleTags>) -> Result<CardDb> {
    let mut reader = open_maybe_gz(path)?;
    let first = reader.fill_buf()?.iter().find(|b| !b.is_ascii_whitespace()).copied();
    // oracle id -> (card, cheapest usd, cheapest foil, cheapest etched)
    type Slot = (Card, Option<f64>, Option<f64>, Option<f64>);
    let mut by_oracle: HashMap<String, Slot> = HashMap::new();
    let mut add = |raw: RawCard| {
        if raw.digital {
            return;
        }
        let (usd, foil, etched) =
            (parse_price(&raw.prices.usd), parse_price(&raw.prices.usd_foil), parse_price(&raw.prices.usd_etched));
        let Some(card) = convert(raw) else { return };
        let slot = by_oracle.entry(card.oracle_id.clone()).or_insert_with(|| (card, None, None, None));
        keep_min(&mut slot.1, usd);
        keep_min(&mut slot.2, foil);
        keep_min(&mut slot.3, etched);
    };
    if first == Some(b'[') {
        let raws: Vec<RawCard> = serde_json::from_reader(reader).context("parsing Scryfall bulk JSON array")?;
        raws.into_iter().for_each(&mut add);
    } else {
        for line in reader.lines() {
            let line = line?;
            let line = line.trim().trim_end_matches(',');
            if line.starts_with('{') {
                add(serde_json::from_str(line).context("parsing a Scryfall bulk line")?);
            }
        }
    }
    let mut by_name: HashMap<String, Card> = HashMap::with_capacity(by_oracle.len());
    for (_, (mut card, usd, foil, etched)) in by_oracle {
        card.usd = usd.or(foil).or(etched).unwrap_or(0.0);
        if let Some(t) = tags {
            card.roles = t.roles.get(&card.oracle_id).copied().unwrap_or(0);
            card.tagged = t.tagged.contains(&card.oracle_id);
        }
        // On a name clash (rare), prefer the one people actually play.
        match by_name.get(&card.name) {
            Some(prev) if prev.rank.is_some() || card.rank.is_none() => {}
            _ => {
                by_name.insert(card.name.clone(), card);
            }
        }
    }
    let mut cards: Vec<Card> = by_name.into_values().collect();
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(CardDb::new(cards))
}

/// A name folded for searching: lowercase ASCII, apostrophes dropped, other punctuation as spaces
/// ("Kraum, Ludevic's Opus" -> "kraum  ludevics opus").
pub fn search_key(name: &str) -> String {
    fold(name).chars().filter(|c| *c != '\'').map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect()
}

pub fn search_terms(query: &str) -> Vec<String> {
    search_key(query).split_whitespace().map(String::from).collect()
}

/// Does `key` (a [`search_key`]) contain every term?
pub fn matches_terms(key: &str, terms: &[String]) -> bool {
    terms.iter().all(|t| key.contains(t.as_str()))
}

/// Names a decklist line might use for this card.
pub fn display_name(card: &Card) -> &str {
    front_face(&card.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, faces: &[&str]) -> Card {
        Card {
            name: name.into(),
            faces: faces.iter().map(|s| s.to_string()).collect(),
            type_line: "Legendary Creature — Elf".into(),
            oracle: vec![String::new()],
            layout: "normal".into(),
            ..Default::default()
        }
    }

    #[test]
    fn lookup_by_full_face_and_folded_names() {
        let db = CardDb::new(vec![
            card("Esika, God of the Tree // The Prismatic Bridge", &["Esika, God of the Tree", "The Prismatic Bridge"]),
            card("Juzám Djinn", &[]),
        ]);
        assert_eq!(db.canonical("esika, god of the tree").unwrap(), "Esika, God of the Tree // The Prismatic Bridge");
        assert_eq!(db.canonical("The Prismatic Bridge").unwrap(), "Esika, God of the Tree // The Prismatic Bridge");
        assert_eq!(db.canonical("Juzam Djinn").unwrap(), "Juzám Djinn");
        assert!(db.get("Nope").is_none());
    }

    #[test]
    fn commander_search() {
        let db = CardDb::new(vec![card("Jodah, the Unifier", &[]), card("Jodah, Archmage Eternal", &[])]);
        assert_eq!(db.find_commander("jodah, the unifier").unwrap().name, "Jodah, the Unifier");
        assert_eq!(db.find_commander("unifier").unwrap().name, "Jodah, the Unifier");
        assert_eq!(db.find_commander("jodah").unwrap_err().len(), 2);
        assert!(db.find_commander("zzz").unwrap_err().is_empty());
    }

    #[test]
    fn candidate_search_ignores_order_punctuation_and_case() {
        let mut kraum = card("Kraum, Ludevic's Opus", &[]);
        kraum.rank = Some(900);
        let mut violent = card("Kraum, Violent Cacophony", &[]);
        violent.rank = Some(5000);
        let mut not_cmdr = card("Kraum's Ally", &[]);
        not_cmdr.type_line = "Creature — Zombie".into();
        let db = CardDb::new(vec![violent, kraum, not_cmdr]);
        let names = |q: &str| db.commander_candidates(q).iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("kraum"), vec!["Kraum, Ludevic's Opus", "Kraum, Violent Cacophony"]); // by popularity
        assert_eq!(names("opus kraum"), vec!["Kraum, Ludevic's Opus"]);
        assert_eq!(names("LUDEVICS"), vec!["Kraum, Ludevic's Opus"]);
        assert_eq!(names("kraum, lud"), vec!["Kraum, Ludevic's Opus"]);
        assert_eq!(db.find_commander("kraum lud").unwrap().name, "Kraum, Ludevic's Opus");
        assert_eq!(search_key("Kraum, Ludevic's Opus"), "kraum  ludevics opus");
    }
}
