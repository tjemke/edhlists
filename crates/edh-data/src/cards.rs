//! The local card database, built from Scryfall's `oracle-cards` bulk file by `sync`.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::flags::EnumSet;
use crate::mana::{ColorSet, ManaCost, ManaSet};
use crate::storage::{open_maybe_gz, read_json_gz, write_json_gz};
use crate::tags::{RoleTags, Roles};
use crate::text::{fold, front_face};
use crate::types::{CardType, Supertype, Types};

/// Bump when [`Card`] changes shape; `sync` rebuilds from the kept raw bulk file without downloading.
pub const CARDS_SCHEMA: u32 = 6;

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
    /// What each face costs to cast, like `oracle` (one entry for single-faced cards); `None` for
    /// a face without a mana cost (lands, Ancestral Vision, a transformed back face) and for the
    /// few Un-card symbols [`ManaCost`] doesn't read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mana_cost: Vec<Option<ManaCost>>,
    /// Full type line; multi-face cards are joined with " // ".
    pub type_line: String,
    /// Oracle text per face (one entry for single-faced cards), exactly as EDHPowerLevel sees it.
    pub oracle: Vec<String>,
    pub layout: String,
    /// `None` when Scryfall has no colors at all; EDHPowerLevel treats that differently from `[]`.
    pub colors: Option<ColorSet>,
    /// Each face's colours, for cards whose faces have their own and the card none (transforming
    /// and modal double-faced cards: Plargg // Augusta is red, then white); empty otherwise. A `c`
    /// search takes such a card as all its faces' colours together (red-white).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub face_colors: Vec<ColorSet>,
    #[serde(default)]
    pub identity: ColorSet,
    /// Cheapest non-foil USD price across real printings (what EDHPowerLevel prices cards at).
    pub usd: f64,
    pub rank: Option<u32>,
    #[serde(default)]
    pub reserved: bool,
    #[serde(default)]
    pub game_changer: bool,
    #[serde(default)]
    pub banned: bool,
    /// The kinds of mana it can make, as Scryfall lists them (`produced_mana`).
    #[serde(default, skip_serializing_if = "EnumSet::is_empty")]
    pub produced: ManaSet,
    /// The roles Scryfall Tagger's tags give it ([`crate::tags`]): ramp, draw, removal, wipe.
    #[serde(default, skip_serializing_if = "EnumSet::is_empty")]
    pub roles: Roles,
    /// Whether Scryfall Tagger knows this card at all (if not, roles come from heuristics).
    #[serde(default)]
    pub tagged: bool,
    /// Keyword abilities, as Scryfall lists them ("Flying", "Equip", ...), all faces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

impl Card {
    /// The front face's type line ("Legendary Creature — Elf").
    pub fn front_type_line(&self) -> &str {
        self.type_line.split(" // ").next().unwrap_or("")
    }
    /// The front face's types.
    pub fn types(&self) -> Types {
        Types::parse(self.front_type_line())
    }
    /// Each face's types, front first.
    pub fn face_types(&self) -> impl Iterator<Item = Types> + '_ {
        self.type_line.split(" // ").map(Types::parse)
    }
    pub fn is_land(&self) -> bool {
        self.types().is(CardType::Land)
    }
    pub fn is_mdfc(&self) -> bool {
        self.layout == "modal_dfc"
    }
    pub fn is_basic(&self) -> bool {
        self.types().supertypes.contains(Supertype::Basic)
    }
    /// The one main type EDHREC files the card under, by its front face ([`Types::primary`]).
    /// Checked against EDHREC's deck tables: counted this way, at least 99.5% of 24,696 stored
    /// decks match EDHREC's count of each type.
    pub fn primary_type(&self) -> Option<CardType> {
        self.types().primary()
    }
    pub fn can_be_commander(&self) -> bool {
        let t = self.types();
        let background = self.front_type_line().split_once('—').is_some_and(|(_, sub)| sub.contains("Background"));
        (t.supertypes.contains(Supertype::Legendary) && (t.is(CardType::Creature) || background))
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
        self.cards.get(self.position(name)?)
    }

    /// Where [`CardDb::get`]'s card sits in `cards`.
    pub fn position(&self, name: &str) -> Option<usize> {
        let key = name.trim().to_lowercase();
        self.index
            .get(&key)
            .or_else(|| self.index.get(key.split(" // ").next().unwrap_or("")))
            .or_else(|| self.index.get(&fold(&key)))
            .copied()
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
    mana_cost: Option<String>,
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
    mana_cost: Option<String>,
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
    #[serde(default)]
    keywords: Vec<String>,
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
    let cost = |s: &Option<String>| s.as_deref().and_then(|s| s.parse::<ManaCost>().ok());
    // Split and adventure cards also have a joined top-level cost ("{R} // {U}"); the faces' are
    // the ones to cast.
    let mana_cost =
        if multi { c.card_faces.iter().map(|f| cost(&f.mana_cost)).collect() } else { vec![cost(&c.mana_cost)] };
    let type_line = c.type_line.clone().unwrap_or_else(|| {
        c.card_faces.iter().map(|f| f.type_line.clone().unwrap_or_default()).collect::<Vec<_>>().join(" // ")
    });
    let set = |codes: &Vec<String>| ColorSet::from_codes(codes.iter().map(String::as_str));
    let colors = c.colors.as_ref().or_else(|| c.card_faces.first().and_then(|f| f.colors.as_ref())).map(set);
    // Split, adventure and flip cards have the card's colours and none per face; double-faced
    // cards the other way round.
    let face_colors = match c.card_faces.iter().map(|f| f.colors.as_ref().map(set)).collect::<Option<Vec<_>>>() {
        Some(faces) if c.colors.is_none() && multi => faces,
        _ => vec![],
    };
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
        mana_cost,
        type_line,
        oracle,
        layout: c.layout,
        colors,
        face_colors,
        identity: ColorSet::from_codes(c.color_identity.iter().map(String::as_str)),
        usd: 0.0,
        rank: c.edhrec_rank,
        reserved: c.reserved,
        game_changer: c.game_changer,
        banned: legal == "banned",
        produced: ManaSet::from_codes(c.produced_mana.iter().flatten().map(String::as_str)),
        roles: Roles::empty(),
        tagged: false,
        keywords: c.keywords,
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
            card.roles = t.roles.get(&card.oracle_id).copied().unwrap_or_default();
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
    use crate::mana::Color;

    #[test]
    fn double_faced_cards_keep_each_faces_colours() {
        let raw = |json: &str| convert(serde_json::from_str(json).unwrap()).unwrap();
        let plargg = raw(r#"{"name": "Plargg, Dean of Chaos // Augusta, Dean of Order", "layout": "modal_dfc",
                "color_identity": ["R", "W"], "card_faces": [
                    {"name": "Plargg, Dean of Chaos", "colors": ["R"]},
                    {"name": "Augusta, Dean of Order", "colors": ["W"]}]}"#);
        assert_eq!(plargg.face_colors, vec![ColorSet::of(Color::R), ColorSet::of(Color::W)]);
        assert_eq!(plargg.colors, Some(ColorSet::of(Color::R)), "colors stays the front face's, as scoring reads it");
        let fire_ice =
            raw(r#"{"name": "Fire // Ice", "layout": "split", "colors": ["U", "R"], "color_identity": ["U", "R"],
                "card_faces": [{"name": "Fire"}, {"name": "Ice"}]}"#);
        assert!(fire_ice.face_colors.is_empty(), "a split card's colours are the card's: {:?}", fire_ice.face_colors);
        let bolt = raw(r#"{"name": "Lightning Bolt", "layout": "normal", "colors": ["R"]}"#);
        assert!(bolt.face_colors.is_empty());
    }

    #[test]
    fn each_face_keeps_its_own_mana_cost_and_cards_keep_what_mana_they_make() {
        let raw = |json: &str| convert(serde_json::from_str(json).unwrap()).unwrap();
        let costs = |c: &Card| c.mana_cost.iter().map(|m| m.as_ref().map(|m| m.to_string())).collect::<Vec<_>>();
        let tower = raw(r#"{"name": "Command Tower", "layout": "normal", "mana_cost": "",
                "produced_mana": ["W", "U", "B", "R", "G"]}"#);
        assert_eq!(costs(&tower), vec![None], "a land has no mana cost, not {{0}}");
        assert_eq!(tower.produced.to_string(), "WUBRG");
        let sol = raw(r#"{"name": "Sol Ring", "layout": "normal", "mana_cost": "{1}", "produced_mana": ["C"]}"#);
        assert_eq!((costs(&sol), sol.produced.to_string()), (vec![Some("{1}".to_string())], "C".to_string()));
        let fire_ice = raw(r#"{"name": "Fire // Ice", "layout": "split", "mana_cost": "{1}{R} // {1}{U}",
                "card_faces": [{"name": "Fire", "mana_cost": "{1}{R}"}, {"name": "Ice", "mana_cost": "{1}{U}"}]}"#);
        assert_eq!(costs(&fire_ice), vec![Some("{1}{R}".to_string()), Some("{1}{U}".to_string())]);
        let bolt = raw(r#"{"name": "Lightning Bolt", "layout": "normal", "mana_cost": "{R}"}"#);
        assert!(bolt.produced.is_empty(), "makes no mana: {:?}", bolt.produced);
    }

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
    fn each_card_has_the_main_type_edhrec_files_it_under() {
        let typed = |type_line: &str| Card { type_line: type_line.into(), ..Default::default() }.primary_type();
        use CardType::*;
        assert_eq!(typed("Legendary Artifact Planeswalker — Equipment"), Some(Artifact), "The Aetherspark");
        assert_eq!(typed("Artifact Creature — Equipment Lizard"), Some(Creature));
        assert_eq!(typed("Artifact Land"), Some(Land));
        assert_eq!(typed("Enchantment Land — Urza's Saga"), Some(Land));
        assert_eq!(typed("Legendary Enchantment Artifact"), Some(Artifact));
        assert_eq!(typed("Legendary Planeswalker — Jace"), Some(Planeswalker));
        assert_eq!(typed("Legendary Creature — Human Wizard // Legendary Planeswalker — Jace"), Some(Creature));
        assert_eq!(typed("Kindred Instant — Elf"), Some(Instant));
        assert_eq!(typed("Conspiracy"), None);
    }

    #[test]
    fn legendary_creatures_backgrounds_and_cards_that_say_so_can_lead_a_deck() {
        let leads = |type_line: &str, oracle: &str| {
            Card { type_line: type_line.into(), oracle: vec![oracle.into()], ..Default::default() }.can_be_commander()
        };
        assert!(leads("Legendary Creature — Elf", ""));
        assert!(leads("Legendary Enchantment — Background", ""));
        assert!(leads("Legendary Planeswalker — Teferi", "Teferi can be your commander."));
        assert!(!leads("Legendary Planeswalker — Jace", ""));
        assert!(!leads("Creature — Elf", ""), "not legendary");
        assert!(!leads("Legendary Artifact", ""));
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
