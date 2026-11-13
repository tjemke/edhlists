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
//! Like EDHREC's counts, card counts cover the 99: commanders don't count, quantities do. A card
//! filter without a range (`-T equipment`) only counts, it doesn't filter.
//!
//! Before a deck is fetched, a card filter can still use the deck table ([`Prior`]): its cards can
//! only sit in some of EDHREC's type columns, which bounds the count and says which decks are
//! likeliest to match.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use edh_data::cards::{Card, CardDb};
use edh_data::deck::Deck;
use edh_data::edhrec::{ListedCommander, TableRow, Theme};
use edh_data::query::Query;
use edh_data::tags::TagIndex;
use edh_data::text::{slugify, strip_reminder};
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

    /// The search that counts the same type from a decklist's type lines.
    pub fn type_query(self) -> &'static str {
        match self {
            TypeField::Creatures => "t:creature",
            TypeField::Instants => "t:instant",
            TypeField::Sorceries => "t:sorcery",
            TypeField::Spells => "(t:instant or t:sorcery)",
            TypeField::Artifacts => "t:artifact",
            TypeField::Enchantments => "t:enchantment",
            TypeField::Planeswalkers => "t:planeswalker",
            TypeField::Battles => "t:battle",
            TypeField::Lands => "t:land",
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

/// `15+`, `≤2`, `3–5`, `36`; `None` for no range.
fn range_text(min: Option<u32>, max: Option<u32>) -> Option<String> {
    match (min, max) {
        (Some(lo), None) => Some(format!("{lo}+")),
        (None, Some(hi)) => Some(format!("≤{hi}")),
        (Some(lo), Some(hi)) if lo == hi => Some(lo.to_string()),
        (Some(lo), Some(hi)) => Some(format!("{lo}–{hi}")),
        (None, None) => None,
    }
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

    /// The range as `-T` takes it: `10+`, `..5`, `25-40`, `36`.
    pub fn range_arg(&self) -> String {
        match (self.min, self.max) {
            (Some(lo), None) => format!("{lo}+"),
            (None, Some(hi)) => format!("..{hi}"),
            (Some(lo), Some(hi)) if lo == hi => lo.to_string(),
            (Some(lo), Some(hi)) => format!("{lo}-{hi}"),
            (None, None) => "0+".to_string(),
        }
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

    /// Does it ask for a minimum (and so pick themes and commanders when searching)?
    pub fn wants_many(&self) -> bool {
        self.min.is_some_and(|m| m > 0)
    }

    /// Does it filter at all, or only count?
    pub fn has_range(&self) -> bool {
        self.min.is_some() || self.max.is_some()
    }

    /// The one count it lets through, if its range has only one (`=0`, `=4`, `=..0`).
    pub fn exact(&self) -> Option<u32> {
        let lo = self.min.unwrap_or(0);
        (self.max == Some(lo)).then_some(lo)
    }

    /// Check every card in the database once. An `otag:` needs Scryfall Tagger's tag tree; a
    /// filter that matches no card at all is an error (usually a typo).
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
                    let hint = if is_word(&self.label) {
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

/// Search keys that compare (`mv=3`, `c=rg`): an `=` after one is part of the search, not a range.
const COMPARING_KEYS: [&str; 10] = ["mv", "cmc", "manavalue", "usd", "c", "color", "colour", "id", "identity", "ci"];

/// One `-T` argument.
#[derive(Clone, Debug)]
pub enum Filter {
    Table(TypeFilter),
    Cards(CardFilter),
}

impl FromStr for Filter {
    type Err = anyhow::Error;
    /// `WHAT=RANGE`, or just `WHAT` to count without filtering. The range is whatever follows the
    /// last `=` (a range never contains one, a search may), unless that `=` belongs to the search
    /// (`mv<=2`, `mv=3`, `c=rg`). `WHAT` is one of EDHREC's nine counts, a role (`ramp`, `draw`,
    /// `removal`, `wipes`), a type-line word, or a Scryfall-style search.
    fn from_str(s: &str) -> Result<Self> {
        let split = s.rsplit_once('=').filter(|(what, _)| {
            let what = what.trim_end();
            !what.ends_with(['<', '>', '!']) && !COMPARING_KEYS.contains(&what.to_lowercase().as_str())
        });
        let (what, range) = match split {
            Some((what, range)) => (what.trim(), Some(range)),
            None => (s.trim(), None),
        };
        if what.is_empty() {
            bail!("nothing to count in '{s}' (expected WHAT=RANGE like equipment=10+)");
        }
        let (min, max) = match range {
            Some(r) => match parse_range(r, s) {
                Ok(range) => range,
                // Not a range after all: maybe an `=` inside a search (`o:"x=y"`).
                Err(e) => return Self::cards(s.trim(), None, None).map_err(|q| q.context(e)),
            },
            None => (None, None),
        };
        if let Ok(field) = what.parse::<TypeField>() {
            if range.is_none() {
                bail!("EDHREC's {} count needs a range, like {}=10+", field.name(), field.name());
            }
            return Ok(Filter::Table(TypeFilter { field, min, max }));
        }
        Self::cards(what, min, max)
    }
}

impl Filter {
    fn cards(what: &str, min: Option<u32>, max: Option<u32>) -> Result<Self> {
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
    /// The filter's range (`15+`, `≤2`), if it has one.
    pub wanted: Option<String>,
    /// Is the count within that range? (Always, without one.)
    pub passes: bool,
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
        let f = &self.filter;
        let mut m = Matched { label: f.label.clone(), wanted: range_text(f.min, f.max), ..Default::default() };
        for (name, &qty) in &deck.cards {
            if cards.position(name).is_some_and(|i| self.matching.get(i) == Some(&true)) {
                m.count += qty;
                m.cards.push((name.clone(), qty));
            }
        }
        m.passes = f.passes(m.count);
        m
    }

    /// What the deck table can say about this filter before fetching; `None` without a minimum.
    pub fn prior(&self, cards: &CardDb) -> Option<Prior> {
        let min = self.filter.min.filter(|m| *m > 0)?;
        let mut possible = [false; 8];
        let (mut hits, mut all) = ([0.0f64; 8], [0.0f64; 8]);
        for (card, &matches) in cards.cards.iter().zip(&self.matching) {
            let col = edhrec_column(card);
            // Zipf: a card's share of deck slots falls off roughly as 1/rank.
            let weight = card.rank.map_or(0.0, |r| 1.0 / f64::from(r.max(1)));
            if let Some(c) = col {
                all[c] += weight;
            }
            if !matches {
                continue;
            }
            let types: Vec<usize> = (0..8).filter(|&c| card.type_line.contains(COLUMNS[c])).collect();
            if types.is_empty() {
                possible = [true; 8]; // no main type we know of: it could be counted anywhere
            }
            types.into_iter().for_each(|c| possible[c] = true);
            if let Some(c) = col {
                hits[c] += weight;
            }
        }
        let rate = std::array::from_fn(|c| if all[c] > 0.0 { hits[c] / all[c] } else { 0.0 });
        Some(Prior { min, possible, rate })
    }
}

/// EDHREC's per-deck type columns, in [`columns`] order (also [`Card::primary_type`]'s labels).
const COLUMNS: [&str; 8] =
    ["Creature", "Instant", "Sorcery", "Artifact", "Enchantment", "Battle", "Planeswalker", "Land"];

fn columns(r: &TableRow) -> [u32; 8] {
    [r.creature, r.instant, r.sorcery, r.artifact, r.enchantment, r.battle, r.planeswalker, r.land]
}

/// The column EDHREC counts a card in: its front face's main type, land before creature before
/// the rest ("Artifact Creature" is a creature, "Artifact Land" a land), as its deck pages file them.
fn edhrec_column(card: &Card) -> Option<usize> {
    let t = card.primary_type();
    COLUMNS.iter().position(|c| *c == t)
}

/// What the deck table says about a card filter's minimum before a deck is fetched.
#[derive(Clone, Debug)]
pub struct Prior {
    min: u32,
    /// Columns a matching card could be counted in: any main type on any of its faces.
    possible: [bool; 8],
    /// Per column: the share of the cards decks play there that match, by EDHREC popularity.
    rate: [f64; 8],
}

impl Prior {
    /// Could the deck reach the minimum at all? False only when the columns its matching cards
    /// could sit in hold fewer cards than that, so skipping it never loses a match.
    pub fn possible(&self, r: &TableRow) -> bool {
        let cols = columns(r);
        (0..8).filter(|&c| self.possible[c]).map(|c| cols[c]).sum::<u32>() >= self.min
    }

    /// Expected matching cards as a share of the minimum: higher means likelier to pass.
    pub fn score(&self, r: &TableRow) -> f64 {
        let cols = columns(r);
        (0..8).map(|c| self.rate[c] * f64::from(cols[c])).sum::<f64>() / f64::from(self.min)
    }
}

/// What the card filters that ask for many look for in type lines and rules text ("equipment",
/// "create a treasure"): what a commander built for them is likely to mention.
pub fn mention_texts(filters: &[CompiledFilter]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in filters.iter().filter(|f| f.filter.wants_many()) {
        if let Counted::Query(q) = &f.filter.counted {
            out.extend(q.texts().into_iter().filter(|t| !out.contains(t)).collect::<Vec<_>>());
        }
    }
    out
}

/// Single commanders whose own rules text mentions one of `texts` (Wyleth's "Aura or Equipment
/// spell" for "equipment"), most popular first, up to `limit`. Free: read from the card database.
pub fn mentioning_commanders(texts: &[String], cards: &CardDb, limit: usize) -> Vec<ListedCommander> {
    if texts.is_empty() {
        return vec![];
    }
    // At the start of a word, so "aura" doesn't find "Laura" (plurals still match), and not about
    // itself or in the negative ("exile this Saga", "the token isn't legendary").
    let mentions = |text: &str, t: &str| {
        text.match_indices(t).any(|(i, _)| {
            let before = &text[..i];
            before.chars().next_back().is_none_or(|c| !c.is_alphanumeric())
                && !["this ", "isn't ", "not "].iter().any(|p| before.ends_with(p))
        })
    };
    cards
        .commanders()
        .into_iter()
        .filter(|c| c.rank.is_some())
        .filter(|c| {
            let text = strip_reminder(&c.oracle_joined().to_lowercase());
            texts.iter().any(|t| mentions(&text, t))
        })
        .take(limit)
        .map(|c| ListedCommander { name: c.name.clone(), slug: slugify(&c.name), num_decks: 0 })
        .collect()
}

pub fn describe(table: &[TypeFilter], cards: &[CompiledFilter]) -> String {
    table
        .iter()
        .map(ToString::to_string)
        .chain(cards.iter().map(|c| c.filter.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A count column in the deck tables: the card filter it shows (its index in
/// [`Analysis::matches`](crate::analysis::Analysis)) and its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountColumn {
    pub filter: usize,
    pub label: String,
}

impl CountColumn {
    /// The cell for a deck's matches: its count, or `–` if the deck wasn't counted.
    pub fn cell(&self, matches: &[Matched]) -> String {
        matches.get(self.filter).map_or("–".to_string(), |m| m.count.to_string())
    }
}

/// The deck tables' count columns: one per card filter, except those that let only one count
/// through (`sagas=0`), since every deck shown has that count. The detail view still lists them.
pub fn count_columns(filters: &[CompiledFilter]) -> Vec<CountColumn> {
    filters
        .iter()
        .enumerate()
        .filter(|(_, f)| f.filter.exact().is_none())
        .map(|(i, f)| CountColumn { filter: i, label: short_label(&f.filter.label, i) })
        .collect()
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
    for f in filters.iter().map(|c| &c.filter).filter(|f| f.wants_many()) {
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
        for bad in ["=5", "equipment=lots", "set:neo=5+", "t:saga or=2", "artifacts", "creatures=..x"] {
            assert!(bad.parse::<Filter>().is_err(), "should reject {bad:?}");
        }
        assert!("equipment=lots".parse::<Filter>().unwrap_err().to_string().contains("isn't a count"));
    }

    #[test]
    fn without_a_range_a_filter_only_counts() {
        let only = |s: &str, query: &str| {
            let f = cards_filter(s);
            assert!(matches!(&f.counted, Counted::Query(q) if q.source() == query), "{s}");
            assert!(!f.has_range() && f.passes(0) && f.passes(99), "{s}");
        };
        only("equipment", "t:equipment");
        only("mv=3", "mv=3"); // the '=' belongs to the search: a bare key never takes a range
        only("c=rg", "c=rg");
        only("t:equipment mv<=2", "t:equipment mv<=2");
        only(r#"o:"x=y""#, r#"o:"x=y""#);
        assert!(!cards_filter("ramp").has_range());
        let ranged = cards_filter("mv=3=10+");
        assert!(matches!(&ranged.counted, Counted::Query(q) if q.source() == "mv=3"));
        assert_eq!((ranged.min, ranged.max), (Some(10), None));
        let t: TypeFilter = "spells=25-40".parse().unwrap();
        assert_eq!((t.range_arg(), t.field.type_query()), ("25-40".to_string(), "(t:instant or t:sorcery)"));
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
        assert!(err("mv=3").contains("matches no card"));
        assert!(err("o:dragon=2+").contains("after the last '='"));
        assert!(err("otag:ramp=10+").contains("sync"));
    }

    fn ranked(mut c: Card, rank: u32) -> Card {
        c.rank = Some(rank);
        c
    }

    #[test]
    fn priors_bound_and_rank_decks_by_where_matching_cards_sit() {
        let db = CardDb::new(vec![
            ranked(card("Bonesplitter", "Artifact — Equipment", ""), 10),
            ranked(card("Lizard Blades", "Artifact Creature — Equipment Lizard", ""), 50),
            ranked(card("Sol Ring", "Artifact", ""), 1),
            ranked(card("Llanowar Elves", "Creature — Elf Druid", ""), 2),
            ranked(card("Counterspell", "Instant", ""), 3),
            ranked(card("Brainstorm", "Instant", ""), 4),
        ]);
        let prior = |s: &str| cards_filter(s).compile(&db, None).unwrap().prior(&db);
        let eq = prior("equipment=10+").unwrap();
        // Equipment can only be counted as artifacts or creatures.
        assert_eq!(eq.possible, [true, false, false, true, false, false, false, false]);
        let deck =
            |creature, instant, artifact| TableRow { creature, instant, artifact, land: 36, ..Default::default() };
        assert!(eq.possible(&deck(4, 40, 6)) && !eq.possible(&deck(4, 40, 5)), "4 + 5 < 10 can't reach 10");
        assert!(eq.score(&deck(10, 10, 25)) > eq.score(&deck(25, 10, 10)), "artifact-heavy first");
        assert!(eq.score(&deck(10, 10, 25)) > eq.score(&deck(10, 30, 10)));
        let counters = prior("t:instant=12+").unwrap();
        assert!(!counters.possible(&deck(30, 11, 20)) && counters.possible(&deck(0, 12, 0)));
        assert!(prior("equipment=..2").is_none() && prior("equipment").is_none(), "no minimum, no prior");
    }

    #[test]
    fn commanders_whose_text_mentions_what_is_counted() {
        let cmdr = |name: &str, text: &str, rank: u32| ranked(card(name, "Legendary Creature — Human", text), rank);
        let db = CardDb::new(vec![
            cmdr("Wyleth, Soul of Steel", "Whenever you cast an Aura or Equipment spell, draw a card.", 300),
            cmdr("Sram, Senior Edificer", "Whenever you cast an Aura, Equipment, or Vehicle spell, draw a card.", 200),
            cmdr("Laura, the Lost", "Flying", 10),
            cmdr("Brenna, the Bold", "Equipped creatures you control get +1/+1.", 20),
            cmdr("Self, the Armed", "When this Equipment is put into a graveyard, draw a card.", 5),
            cmdr("Copy, the Cloner", "Create a token copy of it, except the token isn't legendary.", 6),
            cmdr("Lord, of Legends", "Legendary spells you cast cost {1} less.", 400),
            card("Unranked Smith", "Legendary Creature — Dwarf", "Equipment you control have equip {1}."),
            card("Bonesplitter", "Artifact — Equipment", ""),
            card("Ethereal Armor", "Enchantment — Aura", ""),
        ]);
        let compile = |s: &[&str]| -> Vec<CompiledFilter> {
            s.iter().map(|x| cards_filter(x).compile(&db, None).unwrap()).collect()
        };
        let names = |f: &[CompiledFilter], limit| {
            mentioning_commanders(&mention_texts(f), &db, limit).into_iter().map(|c| c.slug).collect::<Vec<_>>()
        };
        assert_eq!(names(&compile(&["equipment=10+"]), 5), vec!["sram-senior-edificer", "wyleth-soul-of-steel"]);
        assert_eq!(names(&compile(&["equipment=10+"]), 1), vec!["sram-senior-edificer"]);
        assert_eq!(names(&compile(&["aura=5+"]), 5), vec!["sram-senior-edificer", "wyleth-soul-of-steel"]);
        assert!(names(&compile(&["equipment=..2", "ramp=10+"]), 5).is_empty(), "only minimums of searches");
        assert_eq!(names(&compile(&["legendary=20+"]), 5), vec!["lord-of-legends"]);
    }

    #[test]
    fn single_count_filters_get_no_column() {
        let exact = |s: &str| cards_filter(s).exact();
        assert_eq!(exact("equipment=0"), Some(0));
        assert_eq!(exact("equipment=4"), Some(4));
        assert_eq!(exact("equipment=..0"), Some(0));
        assert_eq!(exact("equipment=3-3"), Some(3));
        assert_eq!(exact("equipment=3-4"), None);
        assert_eq!(exact("equipment=0+"), None);
        assert_eq!(exact("equipment=..1"), None);
        assert_eq!(exact("equipment"), None);

        let db = db();
        let compiled: Vec<CompiledFilter> = ["sagas=0", "equipment=2+", "goblin=..0", "o:\"treasure\""]
            .iter()
            .map(|x| cards_filter(x).compile(&db, None).unwrap())
            .collect();
        let cols = count_columns(&compiled);
        assert_eq!(
            cols.iter().map(|c| (c.filter, c.label.as_str())).collect::<Vec<_>>(),
            vec![(1, "equipment"), (3, "treasure")]
        );
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
