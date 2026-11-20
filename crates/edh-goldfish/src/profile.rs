//! What a card does for a deck's mana, read from its oracle text, types and mana cost.

use std::sync::LazyLock;

use edh_data::cards::Card;
use edh_data::flags::{EnumSet, Flag};
use edh_data::mana::{ColorSet, Mana, ManaCost, ManaSet, Pip};
use edh_data::tags::Role;
use edh_data::text::strip_reminder;
use edh_data::types::{BasicLandType, CardType, Layout, Supertype, Types};
use regex::Regex;

/// One unit of mana a source makes: any one of these kinds.
pub type Unit = ManaSet;

/// How a card can be played: as a land (one way per face that is a land: a pathway has two, a
/// spell with a land back one) and as a spell.
#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    /// The front face's type line, lowercased: what a snarl's "reveal a Forest or Plains card"
    /// looks for.
    pub type_words: String,
    pub lands: Vec<LandFace>,
    pub spell: Option<Spell>,
}

#[derive(Clone, Debug)]
pub struct LandFace {
    pub kind: LandKind,
    /// The back of a modal double-faced card whose front is a spell.
    pub mdfc_back: bool,
}

#[derive(Clone, Debug)]
pub enum LandKind {
    Mana(Land),
    /// Sacrificed for a land from the library (Evolving Wilds, Misty Rainforest).
    Fetch {
        finds: LandFilter,
        tapped: bool,
    },
}

/// A land as it sits on the battlefield.
#[derive(Clone, Debug)]
pub struct Land {
    pub units: Vec<Unit>,
    pub enters: Enters,
    /// Returns a land to its owner's hand as it enters (Simic Growth Chamber).
    pub bounce: bool,
    pub basic: bool,
    pub land_types: EnumSet<BasicLandType>,
}

/// Whether a land enters tapped.
#[derive(Clone, Debug, PartialEq)]
pub enum Enters {
    Untapped,
    Tapped,
    /// Shock lands and the like: untapped for life, which a goldfish always pays.
    PayLife,
    /// Check lands: untapped if a land of one of these types is already out.
    UnlessControl(EnumSet<BasicLandType>),
    /// Snarls and the like: untapped if a card with one of these types is in hand.
    UnlessReveal(Vec<String>),
    /// Untapped with at least this many basic lands out.
    UnlessBasics(usize),
    /// Slow lands: untapped with at least this many other lands out.
    UnlessLandsAtLeast(usize),
    /// Fast lands: untapped with at most this many other lands out.
    UnlessLandsAtMost(usize),
}

/// Which lands a fetch or a ramp spell can find.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandFilter {
    pub basic: bool,
    /// Any of these types; empty means any land.
    pub types: EnumSet<BasicLandType>,
}

impl LandFilter {
    pub fn allows(&self, land: &Land) -> bool {
        (!self.basic || land.basic) && (self.types.is_empty() || land.land_types.intersects(self.types))
    }
}

#[derive(Clone, Debug)]
pub struct Spell {
    pub cost: ManaCost,
    pub effect: Effect,
}

impl Spell {
    pub fn mana_value(&self) -> u32 {
        self.cost.mana_value()
    }
    pub fn has_x(&self) -> bool {
        self.cost.pips.contains(&Pip::X)
    }
    /// A rock with X in its cost (Astral Cornucopia) is cast with X = 1, among the ramp; other X
    /// spells take whatever mana is left at the end of the turn.
    pub fn x_rock(&self) -> bool {
        self.has_x() && matches!(self.effect, Effect::Rock { .. })
    }
    /// Generic mana paid on top of the cost as printed: X = 1 for an X rock, and the activation of
    /// a land-ramp artifact (Wayfarer's Bauble).
    pub fn extra(&self) -> u32 {
        let x = if self.x_rock() { self.cost.pips.iter().filter(|p| **p == Pip::X).count() as u32 } else { 0 };
        match self.effect {
            Effect::LandRamp { activation, .. } => x + activation,
            _ => x,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// A mana rock or dork: these units every turn, from the next one if it's a creature.
    Rock {
        units: Vec<Unit>,
        summoning_sick: bool,
    },
    /// Puts lands from the library onto the battlefield (and maybe into hand); `activation` is
    /// mana to pay on top of the cost (Wayfarer's Bauble).
    LandRamp {
        finds: LandFilter,
        to_battlefield: u8,
        to_hand: u8,
        tapped: bool,
        activation: u32,
    },
    /// Draws this many cards (less any it discards), once.
    Draw {
        cards: u8,
    },
    /// Draws a card every turn from the next one on.
    Engine,
    Other,
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

/// A repeatable mana ability: tapping (with whatever else it costs), or life alone (Kozilek's
/// Translator); the colour may be chosen first (Astral Cornucopia).
static TAP_FOR_MANA: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:\{(\d+)\}, )?(\{t\}[^:.]*|pay \d+ life): (?:choose a color\. )?add ([^.]+)"));
static FETCH: LazyLock<Regex> = LazyLock::new(|| {
    re(r"sacrifice [^:]*: search your library for an? ([^.]*?) card,? (?:and )?put it onto the battlefield( tapped)?")
});
static RAMP_SEARCH: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"search your library for (?:up to )?(an?|one|two|three) ([^.]*?) cards?(?:,| and| then)[^.]*?onto the battlefield( tapped)?",
    )
});
static ACTIVATION: LazyLock<Regex> = LazyLock::new(|| re(r"\{(\d+)\}, \{t\}, sacrifice"));
static DRAWS: LazyLock<Regex> = LazyLock::new(|| re(r"\bdraws? (an?|one|two|three|four|x) (?:additional )?cards?"));
static DISCARDS: LazyLock<Regex> = LazyLock::new(|| re(r"\bdiscards? (an?|one|two|three) cards?"));
/// A card drawn again and again: a trigger that draws (in its own sentence or the next, "you may
/// pay {1}. If you do, draw a card"), or a tap ability that does.
static ENGINE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:at the beginning of|whenever)[^.]*(?:\. if you do,[^.]*)?\bdraw|\{t\}[^:]*:[^.]*\bdraw"));
/// A planeswalker ability that draws and can be used every turn: + or 0 loyalty.
static LOYALTY_DRAW: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(?:\+\d+|0): [^\n]*\bdraw"));
static UNLESS_CONTROL: LazyLock<Regex> =
    LazyLock::new(|| re(r"enters tapped unless you control an? (\w+)(?: or an? (\w+))?\."));
static REVEAL: LazyLock<Regex> =
    LazyLock::new(|| re(r"you may reveal an? (\w+)(?: or an? (\w+)| or (\w+))? card from your hand"));

fn number(word: &str) -> u8 {
    match word {
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "x" => 2,
        _ => 1,
    }
}

/// The face's rules text, lowercased, without reminder text.
fn text(card: &Card, face: usize) -> String {
    strip_reminder(card.oracle.get(face).map(String::as_str).unwrap_or("")).to_lowercase()
}

/// Basic land types named in some text ("a forest or island").
fn land_types_in(text: &str) -> EnumSet<BasicLandType> {
    BasicLandType::ALL.iter().copied().filter(|t| text.contains(&t.code().to_lowercase())).collect()
}

impl Profile {
    /// `identity`: the deck's colours, which "any colour" mana is narrowed to.
    pub fn of(card: &Card, identity: ColorSet) -> Profile {
        let faces: Vec<Types> = card.face_types().collect();
        let allowed: ManaSet = identity.iter().map(Mana::from).chain([Mana::C]).collect();
        let mut lands = Vec::new();
        let front_is_land = faces.first().is_some_and(|t| t.is(CardType::Land));
        let land_faces: Vec<usize> = match card.layout {
            // Both faces of a modal card can be played; a transforming card's back can't.
            Layout::ModalDfc => (0..faces.len()).filter(|&i| faces[i].is(CardType::Land)).collect(),
            _ if front_is_land => vec![0],
            _ => vec![],
        };
        for i in land_faces {
            lands.push(LandFace { kind: land_kind(card, i, &faces[i], allowed), mdfc_back: i > 0 && !front_is_land });
        }
        let spell = if front_is_land {
            None
        } else {
            card.mana_cost
                .first()
                .cloned()
                .flatten()
                .map(|cost| Spell { cost, effect: effect(card, &faces[0], allowed) })
        };
        Profile { name: card.name.clone(), type_words: card.front_type_line().to_lowercase(), lands, spell }
    }
}

fn land_kind(card: &Card, face: usize, types: &Types, allowed: ManaSet) -> LandKind {
    let t = text(card, face);
    let units = mana_units(&t, card.produced, allowed);
    if units.is_empty()
        && let Some(m) = FETCH.captures(&t)
    {
        let what = &m[1];
        let finds = LandFilter { basic: what.contains("basic"), types: land_types_in(what) };
        return LandKind::Fetch { finds, tapped: m.get(2).is_some() };
    }
    LandKind::Mana(Land {
        units,
        enters: enters(&t),
        bounce: t.contains("return a land you control to its owner's hand"),
        basic: types.supertypes.contains(Supertype::Basic),
        land_types: types.land_types,
    })
}

/// The mana one tap makes, from its "{T}: Add …" abilities (not ones that sacrifice it): the
/// ability making the most mana, or, when each makes one, one unit of any of theirs (a
/// Talisman: {C}, or {U} or {B} for 1 life). A card Scryfall says makes mana but whose ability
/// doesn't read that way (a basic land, whose ability is reminder text) makes one of what it can.
fn mana_units(text: &str, produced: ManaSet, allowed: ManaSet) -> Vec<Unit> {
    if produced.is_empty() {
        return vec![];
    }
    let narrow = |u: Unit| if (u & allowed).is_empty() { u } else { u & allowed };
    let any = narrow(produced);
    let found: Vec<_> = TAP_FOR_MANA.captures_iter(text).collect();
    if found.is_empty() {
        return vec![any];
    }
    // Not one-shot mana (sacrificing it). Mana only some spells may use counts like any other.
    let abilities: Vec<Vec<Unit>> = found
        .iter()
        .filter(|m| !m[2].contains("sacrifice"))
        .map(|m| ability_units(m.get(1).and_then(|g| g.as_str().parse().ok()).unwrap_or(0), &m[3], any, &narrow))
        .collect();
    match abilities.iter().map(Vec::len).max() {
        None => vec![], // only one-shot mana (Lotus Petal)
        Some(1) => vec![abilities.iter().flatten().fold(Unit::empty(), |a, u| a | *u)],
        Some(n) => abilities.into_iter().find(|a| a.len() == n).expect("the longest"),
    }
}

/// The units one "Add …" makes; `filter` is generic mana it costs ({1}, {T}: Add {U}{B}).
fn ability_units(filter: usize, what: &str, any: Unit, narrow: &impl Fn(Unit) -> Unit) -> Vec<Unit> {
    let symbols = |s: &str| -> Vec<Mana> {
        let mut v = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            let Some(j) = rest[i..].find('}') else { break };
            if let Some(k) = Mana::from_code(&rest[i + 1..i + j].to_uppercase()) {
                v.push(k);
            }
            rest = &rest[i + j + 1..];
        }
        v
    };
    let units: Vec<Unit> = if what.contains("three mana") {
        vec![any; 3]
    } else if what.contains("two mana") {
        vec![any; 2]
    } else if what.contains(" or ") || what.contains("any") {
        // A choice ("{U} or {B}", "one mana of any color"): one unit of whichever.
        let named: Unit = symbols(what).into_iter().collect();
        vec![if named.is_empty() { any } else { narrow(named) }]
    } else {
        let syms = symbols(what);
        if syms.is_empty() { vec![any] } else { syms.into_iter().map(|k| narrow(k.into())).collect() }
    };
    if filter == 0 {
        return units;
    }
    // A filter ({1}, {T}: Add {U}{B}) turns one mana into its colours: a unit of them, less the input.
    let union = units.iter().fold(Unit::empty(), |a, u| a | *u);
    vec![union; units.len().saturating_sub(filter).max(1)]
}

fn enters(t: &str) -> Enters {
    if t.contains("two or more opponents") {
        return Enters::Untapped;
    }
    if !t.contains("enters tapped") && !t.contains("enters the battlefield tapped") {
        return Enters::Untapped;
    }
    if t.contains("you may pay") && t.contains("life") {
        return Enters::PayLife;
    }
    if let Some(m) = REVEAL.captures(t) {
        let words = (1..=3).filter_map(|i| m.get(i)).map(|g| g.as_str().to_string()).collect();
        return Enters::UnlessReveal(words);
    }
    if let Some(m) = UNLESS_CONTROL.captures(t) {
        let types = land_types_in(&m[0]);
        if !types.is_empty() {
            return Enters::UnlessControl(types);
        }
    }
    if t.contains("unless you control two or more basic lands") {
        return Enters::UnlessBasics(2);
    }
    if t.contains("unless you control two or more other lands") {
        return Enters::UnlessLandsAtLeast(2);
    }
    if t.contains("unless you control two or fewer other lands") {
        return Enters::UnlessLandsAtMost(2);
    }
    Enters::Tapped
}

fn effect(card: &Card, types: &Types, allowed: ManaSet) -> Effect {
    let t = text(card, 0);
    let permanent = types.is_permanent();
    let can_tap = permanent && !types.is(CardType::Planeswalker);
    if can_tap && !card.produced.is_empty() && TAP_FOR_MANA.is_match(&t) {
        let units = mana_units(&t, card.produced, allowed);
        if !units.is_empty() {
            return Effect::Rock { units, summoning_sick: types.is(CardType::Creature) };
        }
    }
    if let Some(m) = RAMP_SEARCH.captures(&t) {
        let what = &m[2];
        let types_wanted = land_types_in(what);
        if what.contains("land") || !types_wanted.is_empty() {
            let n = number(&m[1]);
            let to_hand = u8::from(t.contains("other into your hand"));
            return Effect::LandRamp {
                finds: LandFilter { basic: what.contains("basic"), types: types_wanted },
                to_battlefield: n.saturating_sub(to_hand).max(1),
                to_hand,
                tapped: m.get(3).is_some(),
                activation: ACTIVATION.captures(&t).and_then(|a| a[1].parse().ok()).unwrap_or(0),
            };
        }
    }
    let draws = if card.tagged { card.roles.contains(Role::Draw) } else { DRAWS.is_match(&t) };
    if draws && types.is(CardType::Planeswalker) {
        // A draw only in a minus ability (Ugin's −10) is a long way off.
        return if LOYALTY_DRAW.is_match(&t) { Effect::Engine } else { Effect::Other };
    }
    if draws {
        if permanent && ENGINE.is_match(&t) {
            return Effect::Engine;
        }
        let drawn = DRAWS.captures_iter(&t).map(|c| number(&c[1])).max().unwrap_or(1);
        let discarded = DISCARDS.captures_iter(&t).map(|c| number(&c[1])).max().unwrap_or(0);
        return Effect::Draw { cards: drawn.saturating_sub(discarded) };
    }
    Effect::Other
}
