//! What a card does for a deck's mana, read from its oracle text, types and mana cost.

use std::sync::LazyLock;

use edh_data::cards::Card;
use edh_data::flags::{EnumSet, Flag};
use edh_data::mana::{ColorSet, Mana, ManaCost, ManaSet, Pip};
use edh_data::tags::Role;
use edh_data::text::{front_face, strip_reminder};
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
    /// What it makes from the start.
    pub units: Vec<Unit>,
    /// What it makes instead once this many lands are out (Temple of the False God: nothing
    /// before five).
    pub with_lands: Option<(usize, Vec<Unit>)>,
    /// Lands sacrificed as it enters (Lotus Field: two).
    pub sacrifices: usize,
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
    /// Generic mana paid on top of the cost as printed ([`Spell::extra`]).
    extra: u32,
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
    /// Generic mana paid on top of the cost as printed: X = 1 for an X rock, one kick of a
    /// multikicker rock (Everflowing Chalice), and the activation of a land-ramp artifact
    /// (Wayfarer's Bauble).
    pub fn extra(&self) -> u32 {
        self.extra
    }

    fn new(cost: ManaCost, effect: Effect, text: &str) -> Spell {
        let mut spell = Spell { cost, effect, extra: 0 };
        if spell.x_rock() {
            spell.extra += spell.cost.pips.iter().filter(|p| **p == Pip::X).count() as u32;
        }
        if let Effect::Rock { .. } = spell.effect
            && let Some(k) = MULTIKICKER.captures(text)
        {
            spell.extra += k[1].parse::<u32>().unwrap_or(0);
        }
        if let Effect::LandRamp { activation, .. } = spell.effect {
            spell.extra += activation;
        }
        spell
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// A mana rock or dork: these units every turn; from the next one if it's a creature or enters
    /// tapped; only the turn it's cast if it doesn't untap by itself (Grim Monolith).
    Rock {
        units: Vec<Unit>,
        next_turn: bool,
        once: bool,
    },
    /// Puts lands from the library onto the battlefield (and maybe into hand); `activation` is
    /// mana to pay on top of the cost (Wayfarer's Bauble).
    LandRamp {
        finds: LandFilter,
        to_battlefield: u8,
        to_hand: u8,
        tapped: bool,
        activation: u32,
        /// Lands sacrificed to do it (Springbloom Druid: one, for two).
        sacrifice: u8,
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

/// A repeatable mana ability: the mana it takes in ("{1}, ", "{B}, ", "{U/R}, "; not {T}), then tapping
/// (with whatever else it costs) or life alone (Kozilek's Translator), then what it adds; the
/// colour may be chosen first (Astral Cornucopia).
static TAP_FOR_MANA: LazyLock<Regex> =
    LazyLock::new(|| re(r"((?:\{[^}t]+\}, )*)(\{t\}[^:.]*|pay \d+ life): (?:choose a color\. )?add ([^.]+)"));
/// A mana ability that waits for enough lands (Temple of the False God, Shrine of the Forsaken Gods).
static ONLY_WITH_LANDS: LazyLock<Regex> = LazyLock::new(|| re(r"activate only if you control (\w+) or more lands"));
/// An amount of mana in words ("three mana", "X mana").
static MANA_AMOUNT: LazyLock<Regex> = LazyLock::new(|| re(r"\b(two|three|four|five|x) mana\b"));
static MULTIKICKER: LazyLock<Regex> = LazyLock::new(|| re(r"multikicker \{(\d+)\}"));
/// A land search in a sentence that needs combat or a death first (Bitterthorn's "whenever
/// equipped creature attacks").
static NEEDS_COMBAT_OR_DEATH: LazyLock<Regex> =
    LazyLock::new(|| re(r"attacks|combat damage|\bdies\b|put into a graveyard"));
/// A draw that waits for something a goldfish doesn't have: combat, a death, an opponent's
/// spell (Mystic Remora), Treasure (Smaug, Wicked Worm).
static DRAW_WAITS: LazyLock<Regex> =
    LazyLock::new(|| re(r"attacks|combat damage|\bdies\b|put into a graveyard|opponent|treasure"));
/// Lands a land sacrifices as it enters (Lotus Field).
static SACRIFICE_ON_ENTERING: LazyLock<Regex> =
    LazyLock::new(|| re(r"when this land enters, sacrifice (an?|one|two|three) lands?"));
/// Mana an Aura adds to a land of ours (Wild Growth, Fertile Ground), not an opponent's
/// permanent turned into a land (Imprisoned in the Moon).
static ENCHANTED_LAND_MANA: LazyLock<Regex> =
    LazyLock::new(|| re(r"whenever enchanted land is tapped for mana, its controller adds"));
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
/// Cards put into hand without the word "draw" (Stock Up: "put two of them into your hand").
static TO_HAND: LazyLock<Regex> = LazyLock::new(|| re(r"put (an?|one|two|three) of them into your hand"));
/// A draw whose size depends on the game (Peer into the Abyss: "draws cards equal to half …").
static DRAWS_SOME: LazyLock<Regex> = LazyLock::new(|| re(r"\bdraws? cards equal to"));
/// Cards drawn for a variable count, as many as a typical one draws.
const VARIABLE_DRAW: u8 = 3;
static DISCARDS: LazyLock<Regex> = LazyLock::new(|| re(r"\bdiscards? (an?|one|two|three) cards?"));
/// A trigger that draws, in its own sentence or the next ("you may pay {1}. If you do, draw a
/// card"); group 1 is what it waits for.
static DRAW_TRIGGER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:at the beginning of|whenever)([^.]*)(?:\. if you do,[^.]*)?\bdraw"));
/// An activated ability that draws (tapping, mana or life); group 1 is its cost, group 2 what it
/// does before the draw.
static DRAW_ABILITY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^((?:\{[^}]+\}|pay \d+ life)[^:\n]*): ([^\n]*)\bdraw"));
/// A draw as the permanent enters (Thought Monitor).
static DRAW_ON_ENTERING: LazyLock<Regex> = LazyLock::new(|| re(r"when [^.]*\benters\b[^.]*\bdraw"));
/// Gets cards back from the graveyard, which a goldfish's is empty of early on (Phyrexian
/// Reclamation): Tagger counts it as card advantage, not a draw.
static REGROWTH: LazyLock<Regex> = LazyLock::new(|| re(r"from your graveyard to your hand"));
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
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
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
                .map(|cost| Spell::new(cost, effect(card, &faces[0], allowed), &text(card, 0)))
        };
        Profile { name: card.name.clone(), type_words: card.front_type_line().to_lowercase(), lands, spell }
    }
}

fn land_kind(card: &Card, face: usize, types: &Types, allowed: ManaSet) -> LandKind {
    let t = text(card, face);
    let makes = mana_units(&t, card.produced, allowed);
    if makes.units.is_empty()
        && makes.with_lands.is_none()
        && let Some(m) = FETCH.captures(&t)
    {
        let what = &m[1];
        let finds = LandFilter { basic: what.contains("basic"), types: land_types_in(what) };
        return LandKind::Fetch { finds, tapped: m.get(2).is_some() };
    }
    LandKind::Mana(Land {
        units: makes.units,
        with_lands: makes.with_lands,
        sacrifices: SACRIFICE_ON_ENTERING.captures(&t).map_or(0, |m| usize::from(number(&m[1]))),
        enters: enters(&t),
        bounce: t.contains("return a land you control to its owner's hand"),
        basic: types.supertypes.contains(Supertype::Basic),
        land_types: types.land_types,
    })
}

/// What a source's mana abilities make: from the start, and once enough lands are out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Makes {
    pub units: Vec<Unit>,
    pub with_lands: Option<(usize, Vec<Unit>)>,
}

/// The mana one activation makes, from its mana abilities (not ones that sacrifice it, nor "max
/// speed" ones a goldfish never reaches): the ability making the most mana, or, when each makes
/// one, one unit of any of theirs (a Talisman: {C}, or {U} or {B} for 1 life). Mana an ability
/// takes in comes off what it makes, so a filter (Blood Celebrant: {B} for any colour) makes
/// none. Abilities that wait for enough lands make up `with_lands`. A card Scryfall says makes
/// mana but whose ability doesn't read that way (a basic land, whose ability is reminder text)
/// makes one of what it can. Mana only some spells may use counts like any other.
fn mana_units(text: &str, produced: ManaSet, allowed: ManaSet) -> Makes {
    if produced.is_empty() {
        return Makes::default();
    }
    let narrow = |u: Unit| if (u & allowed).is_empty() { u } else { u & allowed };
    let any = narrow(produced);
    let found: Vec<_> = TAP_FOR_MANA.captures_iter(text).collect();
    if found.is_empty() {
        return Makes { units: vec![any], with_lands: None };
    }
    let (mut always, mut gated, mut gate, mut lent) = (Vec::new(), Vec::new(), 0, Unit::empty());
    for m in &found {
        let whole = m.get(0).expect("whole match");
        let line_start = text[..whole.start()].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[whole.end()..].find('\n').map_or(text.len(), |i| whole.end() + i);
        let line = &text[line_start..line_end];
        if m[2].contains("sacrifice") || line.starts_with("max speed") {
            continue;
        }
        let (units, kinds) = ability_units(mana_in(&m[1]), &m[3], any, &narrow);
        if units.is_empty() {
            // It turns other mana into its colours (Unknown Shores: {1} for any colour): no more
            // mana, but the land's own can be those colours.
            lent |= kinds;
            continue;
        }
        match ONLY_WITH_LANDS.captures(line) {
            Some(g) => {
                gate = gate.max(usize::from(number(&g[1])));
                gated.push(units);
            }
            None => always.push(units),
        }
    }
    let with_lands =
        (!gated.is_empty()).then(|| (gate, strongest(always.iter().cloned().chain(gated).collect(), lent)));
    Makes { units: strongest(always, lent), with_lands }
}

/// The ability making the most mana; if each makes one, one unit of any of theirs, or of the
/// colours `lent` by abilities that only turn mana into others.
fn strongest(abilities: Vec<Vec<Unit>>, lent: Unit) -> Vec<Unit> {
    match abilities.iter().map(Vec::len).max() {
        None => vec![],
        Some(1) => vec![abilities.iter().flatten().fold(lent, |a, u| a | *u)],
        Some(n) => abilities.into_iter().find(|a| a.len() == n).expect("the longest"),
    }
}

/// Mana a cost takes in: "{2}, " is 2, "{B}, " and "{U/R}, " 1 each.
fn mana_in(costs: &str) -> usize {
    costs.split('}').filter_map(|s| s.split('{').nth(1)).map(|sym| sym.parse::<usize>().unwrap_or(1)).sum()
}

/// The units one "Add …" makes, less `taken_in` mana the ability costs ({1}, {T}: Add {U}{B}),
/// and every kind it can make.
fn ability_units(taken_in: usize, what: &str, any: Unit, narrow: &impl Fn(Unit) -> Unit) -> (Vec<Unit>, Unit) {
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
    let named: Unit = symbols(what).into_iter().collect();
    let units: Vec<Unit> = if let Some(m) = MANA_AMOUNT.captures(what) {
        // "three mana of any one color", "X mana in any combination of colors"
        vec![if named.is_empty() { any } else { narrow(named) }; usize::from(number(&m[1]))]
    } else if what.contains(" or ") || what.contains("any") {
        // A choice: "{U} or {B}", "one mana of any color", "{U}{U}, {U}{R}, or {R}{R}" (two of them).
        let most = what.split(" or ").flat_map(|a| a.split(", ")).map(|alt| symbols(alt).len()).max().unwrap_or(0);
        vec![if named.is_empty() { any } else { narrow(named) }; most.max(1)]
    } else {
        let syms = symbols(what);
        if syms.is_empty() { vec![any] } else { syms.into_iter().map(|k| narrow(k.into())).collect() }
    };
    let kinds = units.iter().fold(Unit::empty(), |a, u| a | *u);
    if taken_in == 0 {
        return (units, kinds);
    }
    // A filter ({1}, {T}: Add {U}{B}) turns mana into its colours: units of any of them, less
    // what it takes in; none when it makes no more than it takes (Blood Celebrant).
    (vec![kinds; units.len().saturating_sub(taken_in)], kinds)
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
    let aura = card.front_type_line().contains("Aura");
    if aura && ENCHANTED_LAND_MANA.is_match(&t) && !card.produced.is_empty() {
        let any = card.produced & allowed;
        return Effect::Rock {
            units: vec![if any.is_empty() { card.produced } else { any }],
            next_turn: false,
            once: false,
        };
    }
    if can_tap && !aura && !card.produced.is_empty() && TAP_FOR_MANA.is_match(&t) {
        let units = mana_units(&t, card.produced, allowed).units;
        if !units.is_empty() {
            return Effect::Rock {
                units,
                next_turn: types.is(CardType::Creature) || t.contains("enters tapped"),
                once: t.contains("doesn't untap during your untap step"),
            };
        }
    }
    if let Some(m) = RAMP_SEARCH.captures(&t) {
        let what = &m[2];
        let types_wanted = land_types_in(what);
        let start = t[..m.get(0).expect("whole match").start()].rfind(['.', '\n']).map_or(0, |i| i + 1);
        let waits = NEEDS_COMBAT_OR_DEATH.is_match(&t[start..m.get(0).expect("whole match").start()]);
        if (what.contains("land") || !types_wanted.is_empty()) && !waits {
            let n = number(&m[1]);
            let to_hand = u8::from(t.contains("other into your hand"));
            let to_battlefield = n.saturating_sub(to_hand).max(1);
            // Swapping a land for one (Crop Rotation, Elvish Reclaimer) is no ramp.
            let sacrifice = u8::from(t.contains("sacrifice a land"));
            if to_battlefield > sacrifice {
                return Effect::LandRamp {
                    finds: LandFilter { basic: what.contains("basic"), types: types_wanted },
                    to_battlefield,
                    to_hand,
                    tapped: m.get(3).is_some(),
                    activation: ACTIVATION.captures(&t).and_then(|a| a[1].parse().ok()).unwrap_or(0),
                    sacrifice,
                };
            }
        }
    }
    let draws = if card.tagged { card.roles.contains(Role::Draw) } else { DRAWS.is_match(&t) };
    if !draws {
        return Effect::Other;
    }
    if types.is(CardType::Planeswalker) {
        // A draw only in a minus ability (Ugin's −10) is a long way off.
        return if LOYALTY_DRAW.is_match(&t) { Effect::Engine } else { Effect::Other };
    }
    if permanent && draws_again(&t) {
        return Effect::Engine;
    }
    let drawn = DRAWS
        .captures_iter(&t)
        .chain(TO_HAND.captures_iter(&t))
        .map(|c| number(&c[1]))
        .chain(DRAWS_SOME.is_match(&t).then_some(VARIABLE_DRAW))
        .max();
    let Some(drawn) = drawn else {
        // Tagged as card advantage without drawing: getting cards back from an empty graveyard
        // finds nothing; anything else is taken as a card.
        return if REGROWTH.is_match(&t) || permanent { Effect::Other } else { Effect::Draw { cards: 1 } };
    };
    // A permanent draws once as it enters, or by sacrificing itself (Mishra's Bauble, Aether
    // Spellbomb); its other draws need something that doesn't happen in a goldfish (a death, an
    // attack, another permanent to sacrifice).
    let name = card.name.to_lowercase();
    let sacrifices_itself = DRAW_ABILITY
        .captures_iter(&t)
        .any(|c| c[1].contains("sacrifice this") || c[1].contains(&format!("sacrifice {}", front_face(&name))));
    if permanent && !DRAW_ON_ENTERING.is_match(&t) && !sacrifices_itself {
        return Effect::Other;
    }
    let discarded = DISCARDS.captures_iter(&t).map(|c| number(&c[1])).max().unwrap_or(0);
    Effect::Draw { cards: drawn.saturating_sub(discarded) }
}

/// Does a permanent draw again and again: a trigger a goldfish sets off (not a death, an attack,
/// combat damage, an opponent or Treasure), or an ability it can use every turn (not one that
/// sacrifices, removes counters, discards or exiles something, nor one that grants a draw on
/// combat damage: Ascendant Spirit)?
fn draws_again(t: &str) -> bool {
    DRAW_TRIGGER.captures_iter(t).any(|c| !DRAW_WAITS.is_match(&c[1]))
        || DRAW_ABILITY.captures_iter(t).any(|c| {
            !["sacrifice", "remove", "discard", "exile"].iter().any(|w| c[1].contains(w))
                && !NEEDS_COMBAT_OR_DEATH.is_match(&c[2])
        })
}
