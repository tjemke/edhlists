//! Deck filters on how many cards of a kind a deck runs: `-T WHAT=RANGE`.
//!
//! One syntax, two kinds of count:
//! * EDHREC's own per-deck type counts (`planeswalkers=10+`, `creatures=..5`) come with the deck
//!   table, so they cost nothing and apply before any deck is fetched ([`TypeFilter`]);
//! * everything else is counted in the decklist once it's fetched ([`CardFilter`]): a type-line
//!   word (`equipment=10+` is short for `t:equipment`), a structural role (`ramp=15+`, counted the
//!   way the Structure section counts it) or a Scryfall-style search (`'o:"create a treasure"=6+'`,
//!   see [`edh_data::query`]).
//!
//! Like EDHREC's counts, card counts cover the 99: commanders don't count, quantities do.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use edh_data::cards::CardDb;
use edh_data::deck::Deck;
use edh_data::edhrec::{TableRow, Theme};
use edh_data::query::Query;
use edh_data::tags::TagIndex;
use serde::{Deserialize, Serialize};

use crate::roles::{Role, roles};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeField {
    Creatures,
    Instants,
    Sorceries,
    /// Instants + sorceries.
    Spells,
    Artifacts,
    Enchantments,
    Planeswalkers,
    Battles,
    Lands,
}

impl TypeField {
    pub fn count(self, r: &TableRow) -> u32 {
        match self {
            TypeField::Creatures => r.creature,
            TypeField::Instants => r.instant,
            TypeField::Sorceries => r.sorcery,
            TypeField::Spells => r.instant + r.sorcery,
            TypeField::Artifacts => r.artifact,
            TypeField::Enchantments => r.enchantment,
            TypeField::Planeswalkers => r.planeswalker,
            TypeField::Battles => r.battle,
            TypeField::Lands => r.land,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TypeField::Creatures => "creatures",
            TypeField::Instants => "instants",
            TypeField::Sorceries => "sorceries",
            TypeField::Spells => "spells",
            TypeField::Artifacts => "artifacts",
            TypeField::Enchantments => "enchantments",
            TypeField::Planeswalkers => "planeswalkers",
            TypeField::Battles => "battles",
            TypeField::Lands => "lands",
        }
    }
}

impl FromStr for TypeField {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_lowercase().as_str() {
            "creature" | "creatures" => TypeField::Creatures,
            "instant" | "instants" => TypeField::Instants,
            "sorcery" | "sorceries" => TypeField::Sorceries,
            "spell" | "spells" => TypeField::Spells,
            "artifact" | "artifacts" => TypeField::Artifacts,
            "enchantment" | "enchantments" => TypeField::Enchantments,
            "planeswalker" | "planeswalkers" | "walkers" => TypeField::Planeswalkers,
            "battle" | "battles" => TypeField::Battles,
            "land" | "lands" => TypeField::Lands,
            other => bail!(
                "unknown card type '{other}' (use creatures, instants, sorceries, spells, artifacts, \
                 enchantments, planeswalkers, battles or lands)"
            ),
        })
    }
}

/// `10+`, `..5`, `25-40` or `36`.
fn parse_range(range: &str, whole: &str) -> Result<(Option<u32>, Option<u32>)> {
    let num = |x: &str| {
        x.trim().parse::<u32>().map_err(|_| {
            anyhow!("'{}' in '{whole}' isn't a count (ranges look like 10+, ..5, 25-40 or 36)", range.trim())
        })
    };
    let range = range.trim();
    Ok(if let Some(lo) = range.strip_suffix('+') {
        (Some(num(lo)?), None)
    } else if let Some(hi) = range.strip_prefix("..") {
        (None, Some(num(hi)?))
    } else if let Some((lo, hi)) = range.split_once('-') {
        let (lo, hi) = (num(lo)?, num(hi)?);
        (Some(lo.min(hi)), Some(lo.max(hi)))
    } else {
        let n = num(range)?;
        (Some(n), Some(n))
    })
}

fn in_range(n: u32, min: Option<u32>, max: Option<u32>) -> bool {
    min.is_none_or(|m| n >= m) && max.is_none_or(|m| n <= m)
}

fn fmt_range(f: &mut fmt::Formatter<'_>, min: Option<u32>, max: Option<u32>, name: &str) -> fmt::Result {
    match (min, max) {
        (Some(lo), None) => write!(f, "{lo}+ {name}"),
        (None, Some(hi)) => write!(f, "≤{hi} {name}"),
        (Some(lo), Some(hi)) if lo == hi => write!(f, "{lo} {name}"),
        (Some(lo), Some(hi)) => write!(f, "{lo}–{hi} {name}"),
        (None, None) => write!(f, "any {name}"),
    }
}

/// A count EDHREC's deck table has: `planeswalkers=10+`, `creatures=..5`, `spells=25-40`, `lands=36`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeFilter {
    pub field: TypeField,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl TypeFilter {
    pub fn matches(&self, r: &TableRow) -> bool {
        in_range(self.field.count(r), self.min, self.max)
    }
}

impl FromStr for TypeFilter {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let (field, range) =
            s.split_once('=').ok_or_else(|| anyhow!("expected TYPE=RANGE like planeswalkers=10+, got '{s}'"))?;
        let field: TypeField = field.parse()?;
        let (min, max) = parse_range(range, s)?;
        Ok(TypeFilter { field, min, max })
    }
}

impl fmt::Display for TypeFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_range(f, self.min, self.max, self.field.name())
    }
}

/// What a [`CardFilter`] counts.
#[derive(Clone, Debug)]
pub enum Counted {
    Query(Query),
    Role(Role),
}

/// A count taken from the decklist: `equipment=10+`, `ramp=15+`, `'o:"create a treasure"=6+'`.
#[derive(Clone, Debug)]
pub struct CardFilter {
    /// What was typed left of the `=`.
    pub label: String,
    pub counted: Counted,
    /// For a type-line word, other spellings to try if it matches no card (`sagas` -> `saga`).
    alternatives: Vec<Query>,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

fn role_named(word: &str) -> Option<Role> {
    Some(match word.to_lowercase().as_str() {
        "ramp" => Role::Ramp,
        "draw" | "card-draw" | "carddraw" => Role::Draw,
        "removal" | "interaction" => Role::Removal,
        "wipe" | "wipes" | "boardwipe" | "boardwipes" | "board-wipe" | "board-wipes" => Role::Wipe,
        _ => return None,
    })
}

/// A plain word (no search syntax): `equipment`, `half-elf`.
fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'')
}

/// Singular forms to try for a type-line word that matches nothing as typed.
fn singulars(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(stem) = word.strip_suffix("ies") {
        out.push(format!("{stem}y"));
    }
    if let Some(stem) = word.strip_suffix("ves") {
        out.push(format!("{stem}f"));
    }
    if let Some(stem) = word.strip_suffix("es") {
        out.push(stem.to_string());
    }
    if let Some(stem) = word.strip_suffix('s') {
        out.push(stem.to_string());
    }
    out.retain(|s| !s.is_empty());
    out
}

impl CardFilter {
    pub fn passes(&self, n: u32) -> bool {
        in_range(n, self.min, self.max)
    }

    pub fn needs_tags(&self) -> bool {
        matches!(&self.counted, Counted::Query(q) if q.needs_tags())
    }

    /// Check every card in the database once. An `otag:` needs Scryfall Tagger's tag tree; a
    /// filter that matches no card at all is an error (a typo, or a forgotten range).
    pub fn compile(mut self, cards: &CardDb, tags: Option<&TagIndex>) -> Result<CompiledFilter> {
        let scan = |q: &Query| cards.cards.iter().map(|c| q.matches(c)).collect::<Vec<bool>>();
        let matching = match &mut self.counted {
            Counted::Role(role) => cards.cards.iter().map(|c| roles(c).contains(role)).collect(),
            Counted::Query(q) => {
                if q.needs_tags() {
                    let tags = tags.ok_or_else(|| {
                        anyhow!("otag: needs Scryfall Tagger's tag file; run `edh-scout sync --cards` once")
                    })?;
                    q.resolve_tags(tags)?;
                }
                let mut matching = scan(q);
                if !matching.contains(&true) {
                    for alt in std::mem::take(&mut self.alternatives) {
                        let m = scan(&alt);
                        if m.contains(&true) {
                            *q = alt;
                            matching = m;
                            break;
                        }
                    }
                }
                if !matching.contains(&true) {
                    let numeric_key = ["mv", "cmc", "usd", "c", "color", "id", "identity"];
                    let hint = if numeric_key.contains(&self.label.to_lowercase().as_str()) {
                        format!(
                            " (a search with its count left off? the count goes after the last '=', like {}<=2=10+)",
                            self.label
                        )
                    } else if is_word(&self.label) {
                        format!(
                            " (as a type: no card has '{0}' on its type line; for card names use name:{0})",
                            self.label
                        )
                    } else {
                        " (check the spelling; the count goes after the last '=')".to_string()
                    };
                    bail!("'{}' matches no card in the card database{hint}", self.label);
                }
                matching
            }
        };
        Ok(CompiledFilter { filter: self, matching })
    }

    /// Words an EDHREC theme built around this might be named after.
    fn words(&self) -> Vec<String> {
        match &self.counted {
            Counted::Role(Role::Ramp) => vec!["ramp".into()],
            Counted::Role(Role::Draw) => vec!["card-draw".into()],
            Counted::Role(Role::Removal) => vec!["removal".into()],
            Counted::Role(Role::Wipe) => vec!["board-wipes".into(), "wipes".into()],
            Counted::Query(q) => q.words(),
        }
    }
}

impl fmt::Display for CardFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_range(f, self.min, self.max, &self.label)
    }
}

/// One `-T` argument.
#[derive(Clone, Debug)]
pub enum Filter {
    Table(TypeFilter),
    Cards(CardFilter),
}

impl FromStr for Filter {
    type Err = anyhow::Error;
    /// Split at the last `=` (a range never contains one, a search may). Left of it: one of
    /// EDHREC's nine counts, a role (`ramp`, `draw`, `removal`, `wipes`), a type-line word, or a
    /// Scryfall-style search.
    fn from_str(s: &str) -> Result<Self> {
        let (what, range) = s
            .rsplit_once('=')
            .ok_or_else(|| anyhow!("expected WHAT=RANGE like equipment=10+ or planeswalkers=..2, got '{s}'"))?;
        let what = what.trim();
        if what.is_empty() {
            bail!("nothing to count in '{s}' (expected WHAT=RANGE like equipment=10+)");
        }
        let (min, max) = parse_range(range, s)?;
        if let Ok(field) = what.parse::<TypeField>() {
            return Ok(Filter::Table(TypeFilter { field, min, max }));
        }
        let (counted, alternatives) = if let Some(role) = role_named(what) {
            (Counted::Role(role), vec![])
        } else if is_word(what) {
            let typed = |w: &str| Query::parse(&format!("t:{w}"));
            (Counted::Query(typed(what)?), singulars(what).iter().map(|w| typed(w)).collect::<Result<_>>()?)
        } else {
            (Counted::Query(what.parse()?), vec![])
        };
        Ok(Filter::Cards(CardFilter { label: what.to_string(), counted, alternatives, min, max }))
    }
}

/// Split parsed `-T` arguments into table counts and decklist counts.
pub fn split(filters: Vec<Filter>) -> (Vec<TypeFilter>, Vec<CardFilter>) {
    let mut table = Vec::new();
    let mut cards = Vec::new();
    for f in filters {
        match f {
            Filter::Table(t) => table.push(t),
            Filter::Cards(c) => cards.push(c),
        }
    }
    (table, cards)
}

/// How many cards of one filter a deck has, and which.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Matched {
    pub label: String,
    pub count: u32,
    /// (name as the decklist has it, quantity), in decklist order.
    pub cards: Vec<(String, u32)>,
}

/// A [`CardFilter`] checked against every card in the database.
#[derive(Clone, Debug)]
pub struct CompiledFilter {
    pub filter: CardFilter,
    /// Per [`CardDb`] position: does that card count?
    matching: Vec<bool>,
}

impl CompiledFilter {
    /// The deck's matching cards among the 99. Cards the database doesn't know never count.
    pub fn count(&self, deck: &Deck, cards: &CardDb) -> Matched {
        let mut m = Matched { label: self.filter.label.clone(), ..Default::default() };
        for (name, &qty) in &deck.cards {
            if cards.position(name).is_some_and(|i| self.matching.get(i) == Some(&true)) {
                m.count += qty;
                m.cards.push((name.clone(), qty));
            }
        }
        m
    }
}

pub fn describe(table: &[TypeFilter], cards: &[CompiledFilter]) -> String {
    table
        .iter()
        .map(ToString::to_string)
        .chain(cards.iter().map(|c| c.filter.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A column header / detail label for the `i`th card filter: the label itself if it is short, else
/// the value of a single `key:value` term, else `Q1`, `Q2`, ...
pub fn short_label(label: &str, i: usize) -> String {
    const MAX: usize = 9;
    if label.chars().count() <= MAX {
        return label.to_string();
    }
    if !label.contains(' ')
        && let Some((_, value)) = label.split_once(':')
    {
        let value = value.trim_matches('"');
        if !value.is_empty() && value.chars().count() <= MAX {
            return value.to_string();
        }
    }
    format!("Q{}", i + 1)
}

/// The EDHREC themes whose commanders are worth searching for these table filters (empty: no
/// theme fits).
pub fn themes(filters: &[TypeFilter]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for f in filters {
        let wants_many = f.min.is_some_and(|m| m > 0);
        let wants_few = f.max.is_some() && f.min.is_none_or(|m| m == 0);
        let theme = match f.field {
            TypeField::Planeswalkers if wants_many => Some("planeswalkers"),
            TypeField::Spells | TypeField::Instants | TypeField::Sorceries if wants_many => Some("spellslinger"),
            TypeField::Artifacts if wants_many => Some("artifacts"),
            TypeField::Enchantments if wants_many => Some("enchantress"),
            TypeField::Lands if wants_many => Some("lands-matter"),
            TypeField::Creatures if wants_few => Some("creatureless"),
            _ => None,
        };
        if let Some(t) = theme
            && !out.contains(&t)
        {
            out.push(t);
        }
    }
    out
}

/// The EDHREC themes (from `known`, EDHREC's theme list) named after what these card filters ask
/// for many of: `equipment` -> equipment, `saga` -> sagas, `legendary` -> legends, `ramp` -> ramp.
pub fn card_themes(filters: &[CompiledFilter], known: &[Theme]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in filters.iter().map(|c| &c.filter).filter(|f| f.min.is_some_and(|m| m > 0)) {
        for word in f.words() {
            let w = word.to_lowercase();
            let mut names = vec![w.clone(), format!("{w}s"), format!("{w}es")];
            if let Some(stem) = w.strip_suffix('y') {
                names.push(format!("{stem}ies"));
            }
            if let Some(stem) = w.strip_suffix('f').or_else(|| w.strip_suffix("fe")) {
                names.push(format!("{stem}ves"));
            }
            if matches!(w.as_str(), "legendary" | "legend") {
                names.push("legends".into());
            }
            for t in known.iter().filter(|t| names.contains(&t.slug)) {
                if !out.contains(&t.slug) {
                    out.push(t.slug.clone());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use edh_data::cards::Card;

    fn row(creature: u32, instant: u32, sorcery: u32, planeswalker: u32) -> TableRow {
        TableRow { creature, instant, sorcery, planeswalker, land: 36, ..Default::default() }
    }

    #[test]
    fn parsing_and_matching() {
        let f: TypeFilter = "planeswalkers=10+".parse().unwrap();
        assert_eq!((f.field, f.min, f.max), (TypeField::Planeswalkers, Some(10), None));
        assert!(f.matches(&row(20, 5, 5, 12)) && !f.matches(&row(20, 5, 5, 9)));
        let few: TypeFilter = "Creature=..5".parse().unwrap();
        assert!(few.matches(&row(3, 20, 20, 0)) && !few.matches(&row(6, 20, 20, 0)));
        let spells: TypeFilter = "spells=40-25".parse().unwrap();
        assert_eq!((spells.min, spells.max), (Some(25), Some(40)));
        assert!(spells.matches(&row(10, 15, 15, 0)));
        assert_eq!("lands=36".parse::<TypeFilter>().unwrap().to_string(), "36 lands");
        assert_eq!(few.to_string(), "≤5 creatures");
        assert!("goblins=5+".parse::<TypeFilter>().is_err());
        assert!("planeswalkers".parse::<TypeFilter>().is_err());
        assert!("planeswalkers=lots".parse::<TypeFilter>().is_err());
    }

    #[test]
    fn filters_pick_themes() {
        let p = |s: &[&str]| themes(&s.iter().map(|x| x.parse().unwrap()).collect::<Vec<_>>());
        assert_eq!(p(&["planeswalkers=10+"]), vec!["planeswalkers"]);
        assert_eq!(p(&["instants=15+", "sorceries=10+"]), vec!["spellslinger"]);
        assert_eq!(p(&["creatures=..5"]), vec!["creatureless"]);
        assert_eq!(p(&["creatures=40+"]), Vec::<&str>::new()); // no theme: most-built commanders
        assert_eq!(p(&["planeswalkers=8+", "spells=25+"]), vec!["planeswalkers", "spellslinger"]);
    }

    fn card(name: &str, type_line: &str, oracle: &str) -> Card {
        Card {
            name: name.into(),
            oracle_id: name.into(),
            type_line: type_line.into(),
            oracle: vec![oracle.into()],
            layout: "normal".into(),
            tagged: true,
            ..Default::default()
        }
    }

    fn db() -> CardDb {
        let mut signet = card("Arcane Signet", "Artifact", "{T}: Add one mana of any color.");
        signet.roles = edh_data::tags::RAMP;
        CardDb::new(vec![
            card("Bonesplitter", "Artifact — Equipment", "Equipped creature gets +2/+0."),
            card("Lizard Blades", "Artifact Creature — Equipment Lizard", "Double strike"),
            card("History of Benalia", "Enchantment — Saga", "I, II — Create a 2/2 white Knight."),
            card("Krenko, Mob Boss", "Legendary Creature — Goblin Warrior", "Create X 1/1 red Goblin tokens."),
            card("Mountain", "Basic Land — Mountain", "({T}: Add {R}.)"),
            card("Smothering Tithe", "Enchantment", "Whenever an opponent draws a card, create a Treasure token."),
            signet,
        ])
    }

    fn cards_filter(s: &str) -> CardFilter {
        match s.parse::<Filter>().unwrap() {
            Filter::Cards(c) => c,
            Filter::Table(t) => panic!("{s} parsed as the table count {t}"),
        }
    }

    #[test]
    fn what_left_of_the_last_equals_means() {
        assert!(matches!("artifacts=10+".parse::<Filter>().unwrap(), Filter::Table(_)));
        assert!(matches!("Creature=..5".parse::<Filter>().unwrap(), Filter::Table(_)));
        assert!(matches!(cards_filter("ramp=15+").counted, Counted::Role(Role::Ramp)));
        assert!(matches!(cards_filter("wipes=3+").counted, Counted::Role(Role::Wipe)));
        let eq = cards_filter("equipment=10+");
        assert!(matches!(&eq.counted, Counted::Query(q) if q.source() == "t:equipment"));
        assert_eq!(eq.to_string(), "10+ equipment");
        let q = cards_filter("t:equipment mv<=2=8+");
        assert!(matches!(&q.counted, Counted::Query(q) if q.source() == "t:equipment mv<=2"));
        assert_eq!((q.min, q.max), (Some(8), None));
        let q = cards_filter(r#"o:"x=y"=..3"#);
        assert!(matches!(&q.counted, Counted::Query(q) if q.source() == r#"o:"x=y""#));
        assert_eq!(q.to_string(), r#"≤3 o:"x=y""#);
        for bad in ["equipment", "=5", "equipment=lots", "set:neo=5+", "t:saga or=2"] {
            assert!(bad.parse::<Filter>().is_err(), "should reject {bad:?}");
        }
    }

    fn deck(cards: &[(&str, u32)]) -> Deck {
        Deck {
            commanders: vec!["Krenko, Mob Boss".into()],
            cards: cards.iter().map(|(n, q)| (n.to_string(), *q)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn counting_covers_the_99_with_quantities() {
        let db = db();
        let d = deck(&[
            ("Bonesplitter", 1),
            ("Lizard Blades", 1),
            ("Mountain", 30),
            ("Arcane Signet", 1),
            ("Not A Card", 1),
            ("History of Benalia", 1),
        ]);
        let count = |s: &str| cards_filter(s).compile(&db, None).unwrap().count(&d, &db);
        let eq = count("equipment=2+");
        assert_eq!(eq.count, 2);
        assert_eq!(eq.cards, vec![("Bonesplitter".to_string(), 1), ("Lizard Blades".to_string(), 1)]);
        assert_eq!(count("t:mountain=30").count, 30);
        assert_eq!(count("legendary=1+").count, 0, "the commander doesn't count");
        assert_eq!(count("sagas=1+").count, 1, "plural words fall back to the singular");
        assert_eq!(count("ramp=1+").count, 1, "lands play no role, the Signet is ramp");
        assert_eq!(count("(t:saga or t:equipment) -t:creature=1+").count, 2);
        let f = cards_filter("equipment=2-3");
        assert!(f.passes(2) && f.passes(3) && !f.passes(1) && !f.passes(4));
    }

    #[test]
    fn filters_that_match_no_card_are_errors() {
        let db = db();
        let err = |s: &str| cards_filter(s).compile(&db, None).unwrap_err().to_string();
        assert!(err("equipmnet=10+").contains("name:equipmnet"));
        assert!(err("mv=3").contains("the count goes after the last '='"), "a forgotten range reads as t:mv");
        assert!(err("o:dragon=2+").contains("after the last '='"));
        assert!(err("otag:ramp=10+").contains("sync"));
    }

    #[test]
    fn labels_for_columns() {
        assert_eq!(short_label("equipment", 0), "equipment");
        assert_eq!(short_label("otag:sweeper", 0), "sweeper");
        assert_eq!(short_label(r#"o:"treasure""#, 0), "treasure");
        assert_eq!(short_label(r#"o:"create a treasure""#, 1), "Q2");
        assert_eq!(short_label("t:equipment mv<=2", 2), "Q3");
    }

    #[test]
    fn card_filters_pick_edhrec_themes() {
        let db = db();
        let known: Vec<Theme> = ["equipment", "sagas", "legends", "ramp", "treasure", "goblins", "elves"]
            .iter()
            .map(|s| Theme { name: s.to_string(), slug: s.to_string(), num_decks: 1 })
            .collect();
        let pick = |s: &[&str]| {
            let compiled: Vec<CompiledFilter> = s.iter().map(|x| cards_filter(x).compile(&db, None).unwrap()).collect();
            card_themes(&compiled, &known)
        };
        assert_eq!(pick(&["equipment=10+"]), vec!["equipment"]);
        assert_eq!(pick(&["saga=4+", "legendary=20+"]), vec!["sagas", "legends"]);
        assert_eq!(pick(&["sagas=4+"]), vec!["sagas"]);
        assert_eq!(pick(&["ramp=15+", "o:treasure=5+"]), vec!["ramp", "treasure"]);
        assert_eq!(pick(&["t:goblin -t:artifact=10+"]), vec!["goblins"]);
        assert!(pick(&["equipment=..2"]).is_empty(), "asking for few doesn't pick a theme");
        assert!(pick(&["t:mountain=10+"]).is_empty());
    }
}
