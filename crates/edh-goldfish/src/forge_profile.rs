//! What a card does for a deck's mana, read from its Forge script ([`edh_data::forge`]): what its
//! mana abilities make and take in, when a land enters tapped, what it fetches or bounces as it
//! enters, what a ramp spell or ability puts onto the battlefield.
//!
//! Only those fields are read, and only in the shapes Forge writes them in (surveyed across its
//! ~34,000 scripts). A shape the reader doesn't know on a line that matters makes the card
//! [`Fidelity::Unreadable`]: it's read from oracle text instead, and the deck's numbers say so.
//! A known simplification makes it [`Fidelity::Approximate`]. Card draw is still read from
//! oracle text.

use edh_data::cards::Card;
use edh_data::flags::{EnumSet, Flag};
use edh_data::forge::{Face, Script, param, params};
use edh_data::mana::{ColorSet, Mana, ManaSet};
use edh_data::tags::Role;
use edh_data::types::{BasicLandType, CardType, Layout, Supertype, Types};

use crate::profile::{
    Effect, Enters, Fidelity, Land, LandFace, LandFilter, LandKind, LeftOut, Makes, Profile, Spell, Unit, strongest,
};

/// Read `card` from its `script`; `oracle` is how its oracle text reads, for what the script
/// reader leaves to it (draw) or can't read.
pub(crate) fn read(card: &Card, script: &Script, identity: ColorSet, oracle: Profile) -> Profile {
    let mut r = Reader::new(identity);
    let profile = r.profile(card, script, &oracle);
    match (r.unreadable, profile) {
        // A land someone else searches for (Path to Exile) is ramp for them, by Tagger's
        // reckoning, not for this deck.
        (None, Some(mut p)) if r.theirs && p.unplayed == Some(Role::Ramp) => {
            p.unplayed = None;
            p
        }
        (None, Some(mut p)) => {
            let plays_as_ramp = !p.lands.is_empty()
                || matches!(p.spell.as_ref().map(|s| &s.effect), Some(Effect::Rock { .. } | Effect::LandRamp { .. }));
            if p.unplayed != Some(Role::Ramp) && !plays_as_ramp && reduces_costs(script) {
                // Tagger doesn't call a cost reducer ramp (Animar, Goreclaw), but it's mana every
                // turn the goldfish doesn't have.
                p.unplayed = Some(Role::Ramp);
            }
            if p.unplayed == Some(Role::Ramp) {
                p.left_out = Some(left_out(script));
            }
            p.undercounted = r.undercounted;
            p
        }
        (reason, _) => Profile { fidelity: Fidelity::Unreadable(reason.unwrap_or("no face to read")), ..oracle },
    }
}

struct Reader {
    /// One mana of any colour: the deck's colours (any colour when it has none).
    any: Unit,
    /// Kinds of mana the deck can use: its colours and colourless.
    allowed: ManaSet,
    /// Simplifications made, first one first.
    notes: Vec<&'static str>,
    unreadable: Option<&'static str>,
    /// It has a land search, but another player's.
    theirs: bool,
    /// It makes more mana than it's counted as making (see [`Profile::undercounted`]).
    undercounted: bool,
    /// The mana ability [`Reader::produced`] last read makes an amount that grows with the board.
    grows: bool,
}

impl Reader {
    fn new(identity: ColorSet) -> Reader {
        let colours: ManaSet = identity.iter().map(Mana::from).collect();
        let any = if colours.is_empty() { ManaSet::from_codes(["W", "U", "B", "R", "G"]) } else { colours };
        Reader {
            any,
            allowed: colours | ManaSet::of(Mana::C),
            notes: Vec::new(),
            unreadable: None,
            theirs: false,
            undercounted: false,
            grows: false,
        }
    }

    fn note(&mut self, why: &'static str) {
        if !self.notes.contains(&why) {
            self.notes.push(why);
        }
    }

    fn cannot(&mut self, why: &'static str) {
        self.unreadable.get_or_insert(why);
    }

    /// Mana outside the deck's colours stays as it is (a Talisman of other colours still makes
    /// mana); a choice is narrowed to the colours the deck can use.
    fn narrow(&self, u: Unit) -> Unit {
        if (u & self.allowed).is_empty() { u } else { u & self.allowed }
    }

    fn profile(&mut self, card: &Card, script: &Script, oracle: &Profile) -> Option<Profile> {
        let types: Vec<Types> = card.face_types().collect();
        let type_lines: Vec<&str> = card.type_line.split(" // ").collect();
        let front_is_land = types.first().is_some_and(|t| t.is(CardType::Land));
        let land_faces: Vec<usize> = match card.layout {
            Layout::ModalDfc => (0..types.len()).filter(|&i| types[i].is(CardType::Land)).collect(),
            _ if front_is_land => vec![0],
            _ => vec![],
        };
        let mut lands = Vec::new();
        for (n, i) in land_faces.into_iter().enumerate() {
            let face = script.faces.get(i)?;
            let kind = self.land(face, &types[i], type_lines.get(i).copied().unwrap_or(""));
            let makes_nothing =
                |k: &LandKind| matches!(k, LandKind::Mana(l) if l.units.is_empty() && l.with_lands.is_none());
            if makes_nothing(&kind) && oracle.lands.get(n).is_some_and(|o| !makes_nothing(&o.kind)) {
                // Urza's Saga: its mana comes from a chapter.
                self.cannot("a land whose mana it can't find");
            }
            lands.push(LandFace { kind, mdfc_back: i > 0 && !front_is_land });
        }
        let spell = if front_is_land {
            None
        } else {
            let cost = card.mana_cost.first().cloned().flatten();
            cost.map(|cost| {
                let face = script.front();
                let effect = self.effect(card, face, &types[0], type_lines[0], oracle);
                Spell::new(cost, effect, multikick(face))
            })
        };
        if script.faces.iter().any(gives_others_mana) {
            // Chromatic Lantern, Abundant Growth: its own mana is read, what it gives the deck's
            // lands (colours, mostly) isn't.
            self.note("mana it gives other permanents left out");
        }
        let fidelity = match self.notes.first() {
            Some(why) => Fidelity::Approximate(why),
            None => Fidelity::Exact,
        };
        Some(Profile::new(card, lands, spell, fidelity))
    }

    fn land(&mut self, face: &Face, types: &Types, type_line: &str) -> LandKind {
        let makes = self.makes(face, types, type_line);
        let entering = self.on_entering(face);
        let makes_nothing = makes.units.is_empty() && makes.with_lands.is_none();
        match fetch_cost(face) {
            // The Landscapes: {C}, or sacrificed for a basic land of one of three types. As
            // the fetch, it's the land it finds, which a deck plays it for.
            Some(0) if !makes_nothing => {
                if let Some(fetch) = self.fetch(face) {
                    self.note("a land that taps for mana or fetches counted as the fetch");
                    return fetch;
                }
            }
            // Myriad Landscape, Krosan Verge: two lands later, for mana.
            Some(_) if !makes_nothing => self.note("a land search it can pay for later left out"),
            _ => {}
        }
        if makes_nothing {
            if let Some(fetch) = self.fetch(face) {
                return fetch;
            }
            if let Some(Entering::Fetch(kind)) = entering {
                return kind;
            }
        }
        let (mut bounce, mut sacrifices) = (false, 0);
        match entering {
            Some(Entering::Bounce) => bounce = true,
            Some(Entering::Sacrifice(n)) => sacrifices = n,
            _ => {}
        }
        if doesnt_untap(face) {
            self.note("a land that doesn't untap counted as one that does");
        }
        LandKind::Mana(Land {
            units: makes.units,
            with_lands: makes.with_lands,
            sacrifices,
            // A land creature (Dryad Arbor) can't tap for mana the turn it comes.
            enters: if types.is(CardType::Creature) { Enters::Tapped } else { self.enters(face) },
            bounce,
            basic: types.supertypes.contains(Supertype::Basic),
            land_types: types.land_types,
        })
    }

    /// What the face's mana abilities make: a basic land type's, and its `AB$ Mana` abilities'
    /// that can be used every turn (tapping it, or life once a turn).
    fn makes(&mut self, face: &Face, types: &Types, type_line: &str) -> Makes {
        let mut always: Vec<Vec<Unit>> = Vec::new();
        let (mut gated, mut gate, mut lent) = (Vec::new(), 0, Unit::empty());
        if types.is(CardType::Land) && !types.land_types.is_empty() {
            always.push(vec![types.land_types.iter().map(|t| Mana::from(t.color())).collect()]);
        }
        if types.is(CardType::Land) {
            match takes_a_type(face) {
                Some(TakesType::Chosen) => {
                    // Multiversal Passage, Thran Portal: the basic land type chosen as it enters.
                    self.note("a colour chosen as it enters counted as any of the deck's");
                    always.push(vec![self.any]);
                }
                Some(TakesType::Copy) => {
                    // Vesuva: a copy of a land on the battlefield.
                    self.note("a copy of another land counted as making any of the deck's colours");
                    always.push(vec![self.any]);
                }
                None => {}
            }
        }
        let mut skipped = false;
        for line in mana_lines(face, types, type_line) {
            let merged;
            let ability = match mana_after(face, line.ability) {
                Some(m) => {
                    merged = m;
                    merged.as_str()
                }
                None => line.ability,
            };
            if line.granted {
                self.note("mana it gives other permanents left out");
                self.undercounted = true;
            }
            if line.needs_counter {
                self.note("mana it has once it has a counter counted from the start");
            }
            let p = params(ability);
            let reflected = match p.first() {
                Some(&("AB", "Mana")) => false,
                Some(&("AB", "ManaReflected")) => true,
                _ => continue,
            };
            let cost = Cost::parse(param(&p, "Cost").unwrap_or(""));
            let once_a_turn = cost.life && param(&p, "ActivationLimit") == Some("1");
            let uses_up = cost.other || cost.sacrifices_itself || cost.sacrifices_lands > 0;
            let waits = param(&p, "CheckSVar").is_some();
            if param(&p, "Planeswalker").is_some()
                || uses_up
                || waits
                || !(cost.taps || once_a_turn || cost.mana_in > 0)
            {
                // A loyalty ability, one that uses something up (itself, a land, counters, other
                // permanents, cards), one that waits for something (Sceptre of Eternal Glory: three
                // lands of a name), or one that costs nothing it pays each turn.
                skipped = true;
                continue;
            }
            if cost.charge_counter {
                // Vivid lands, Sphere of the Suns: two or three uses, then nothing.
                self.note("mana that uses up charge or depletion counters counted as lasting");
            }
            self.grows = false;
            let units = if reflected {
                self.note("mana another land makes counted as any of the deck's colours");
                vec![self.any]
            } else {
                match self.produced(&p, face) {
                    Some(Some(u)) => u,
                    Some(None) => {
                        skipped = true;
                        continue;
                    }
                    None => {
                        self.cannot("a mana ability making mana it can't read");
                        continue;
                    }
                }
            };
            if param(&p, "RestrictValid").is_some() {
                self.note("mana only some spells may spend counted like any other");
            }
            let kinds = units.iter().fold(Unit::empty(), |a, u| a | *u);
            let units = if cost.mana_in == 0 || self.grows {
                // Mana that grows with the board (Cabal Coffers' {2}, {T}: {B} for each Swamp) is
                // counted as one more than it takes in.
                units
            } else {
                // A filter ({1}, {T}: Add {W}{U}) turns mana into its colours: units of any of
                // them, less what it takes in; none when it makes no more than it takes.
                vec![kinds; units.len().saturating_sub(cost.mana_in)]
            };
            if units.is_empty() {
                lent |= kinds;
                continue;
            }
            match param(&p, "IsPresent") {
                // A land's filter taking in more than one mana (Castle Garenbrig's {2}{G}{G}) waits
                // for as many other lands to pay for it.
                None if cost.mana_in > 1 && types.is(CardType::Land) => {
                    gate = gate.max(cost.mana_in + 1);
                    gated.push(units);
                }
                None => always.push(units),
                Some("Land.YouCtrl") => {
                    let n = param(&p, "PresentCompare").and_then(|c| c.strip_prefix("GE")).and_then(|n| n.parse().ok());
                    match n {
                        Some(n) => {
                            gate = gate.max(n);
                            gated.push(units);
                        }
                        None => self.cannot("a mana ability waiting for lands in a way it can't read"),
                    }
                }
                Some(present)
                    if present
                        .split(',')
                        .all(|p| p.strip_suffix(".YouCtrl").and_then(BasicLandType::from_code).is_some()) =>
                {
                    // The Verges' second colour needs a land of a type, there soon enough.
                    self.note("a mana ability that waits for a land of a type counted as always there");
                    always.push(units);
                }
                Some(_) => {
                    // Only with an artifact (Spire of Industry), a Dragon (Mox Jasper), a creature
                    // with power 4 (Fanatic of Rhonas's {G}{G}{G}{G}): its colours, but no more
                    // than one mana.
                    self.note("mana that waits for something on the battlefield counted as one of its colours");
                    always.push(vec![kinds]);
                }
            }
        }
        if skipped && !(always.is_empty() && gated.is_empty()) {
            self.note("some of its mana (sacrificing it, counters, other permanents) left out");
        }
        let with_lands =
            (!gated.is_empty()).then(|| (gate, strongest(always.iter().cloned().chain(gated).collect(), lent)));
        Makes { units: strongest(always, lent), with_lands }
    }

    /// The units one activation makes (`Produced$` times `Amount$`); `Some(None)` when it's an
    /// amount the goldfish doesn't count (it depends on the board), `None` when it can't be read.
    fn produced(&mut self, p: &[(&str, &str)], face: &Face) -> Option<Option<Vec<Unit>>> {
        let produced = param(p, "Produced")?;
        let amount = match param(p, "Amount") {
            None => 1,
            Some(n) if n.chars().all(|c| c.is_ascii_digit()) => n.parse().ok()?,
            Some(var) => match face.svar(var) {
                // Everflowing Chalice, Astral Cornucopia: one counter, as one kick or X = 1.
                Some("Count$CardCounters.CHARGE") => 1,
                // Storage lands' X: what was saved up.
                Some("Count$xPaid") => return Some(None),
                // Urza's lands, Cabal Coffers, Priest of Titania, Selvala: it counts itself, or
                // the power of a creature.
                Some(x) => {
                    self.note("mana that grows with the board counted as one");
                    self.undercounted = true;
                    // The greatest power among your creatures is at least its own (Selvala, Heart
                    // of the Wilds): an amount, which what it takes in comes off.
                    let greatest_power = x == "Count$Valid Creature.YouCtrl$GreatestCardPower";
                    match face.power() {
                        Some(power) if greatest_power && face.types.contains("Creature") => power.max(1) as usize,
                        _ => {
                            // Not a count of what entered this turn, which may be nothing.
                            self.grows = !x.contains("ThisTurnEntered");
                            1
                        }
                    }
                }
                None => {
                    self.note("mana that grows with the board counted as one");
                    self.undercounted = true;
                    self.grows = true;
                    1
                }
            },
        };
        let token = |t: &str, r: &mut Reader| -> Option<Unit> {
            match t {
                "Any" | "AnyDifferent" | "ColorIdentity" | "ColorID" => Some(r.any),
                "Chosen" => {
                    r.note("a colour chosen as it enters counted as any of the deck's");
                    Some(r.any)
                }
                _ => Mana::from_code(t).map(ManaSet::of),
            }
        };
        if produced.starts_with("Special EachColorAmong") {
            // Bloom Tender, Faeburrow Elder: one of each colour among your permanents, itself
            // among them.
            self.note("mana of each colour among your permanents counted as its own colours");
            self.undercounted = true;
            let own: Vec<Unit> = ["W", "U", "B", "R", "G"]
                .into_iter()
                .filter(|c| face.cost.split([' ', '/']).any(|t| t == *c))
                .filter_map(Mana::from_code)
                .map(|m| self.narrow(ManaSet::of(m)))
                .collect();
            return Some((!own.is_empty()).then_some(own));
        }
        let units = if let Some(choice) = produced.strip_prefix("Combo ") {
            let mut kinds = Unit::empty();
            for t in choice.split_whitespace() {
                kinds |= token(t, self)?;
            }
            vec![self.narrow(kinds); amount]
        } else {
            let mut units = Vec::new();
            for t in produced.split_whitespace() {
                let u = token(t, self)?;
                units.push(self.narrow(u));
            }
            if units.is_empty() {
                return None;
            }
            units.iter().cycle().take(units.len() * amount).copied().collect()
        };
        Some(Some(units))
    }

    /// Whether and how the land enters tapped, from its replacement effect (`R:Event$ Moved |
    /// ValidCard$ Card.Self | ReplaceWith$ …`, the replacing SVar a `DB$ Tap` with conditions).
    fn enters(&mut self, face: &Face) -> Enters {
        if copies_tapped(face) {
            return Enters::Tapped;
        }
        for line in face.lines_with("R") {
            let p = params(line);
            if p.first() != Some(&("Event", "Moved")) || param(&p, "ValidCard") != Some("Card.Self") {
                continue;
            }
            let Some(with) = param(&p, "ReplaceWith").and_then(|w| face.svar(w)) else { continue };
            let mut revealed = None;
            for step in chain(face, with) {
                let t = params(step);
                match t.first() {
                    Some(&("DB", "Tap")) => return self.tap_unless(&t, face, revealed),
                    // Revealing a card for the condition (Temple of the Dragon Queen: untapped if
                    // a Dragon was revealed).
                    Some(&("DB", "Reveal")) => revealed = param(&t, "RevealValid"),
                    // Choosing a colour or a type; copying something; cleaning up.
                    Some(&("DB", "ChooseColor" | "ChooseType" | "Clone" | "ChooseCard" | "Cleanup")) => {}
                    _ => self.note("something done as it enters (sacrificing a land) left out"),
                }
            }
        }
        Enters::Untapped
    }

    /// A `DB$ Tap` as a land enters: tapped, unless its condition says otherwise. `revealed`:
    /// the kind of card revealed from hand just before, which the condition counts.
    fn tap_unless(&mut self, t: &[(&str, &str)], face: &Face, revealed: Option<&str>) -> Enters {
        if let Some(cost) = param(t, "UnlessCost") {
            if cost.starts_with("PayLife<") {
                return Enters::PayLife;
            }
            if let Some(what) = cost.strip_prefix("Reveal<1/") {
                // `Reveal<1/Island;Swamp/Island or Swamp>`, `Reveal<1/Elf>`
                let kinds = what.trim_end_matches('>').split('/').next().unwrap_or("");
                return Enters::UnlessReveal(kinds.split(';').map(str::to_lowercase).collect());
            }
            self.note("a land that enters untapped for a price counted as tapped");
            return Enters::Tapped;
        }
        if let (Some(present), Some(compare)) = (param(t, "ConditionPresent"), param(t, "ConditionCompare")) {
            // Tapped when the count of `present` compares so.
            let Some((op, n)) = comparison(compare) else {
                self.cannot("an enters-tapped condition it can't read");
                return Enters::Tapped;
            };
            let checked: Vec<&str> = present.split(',').collect();
            let types: EnumSet<BasicLandType> = checked
                .iter()
                .filter_map(|c| c.strip_suffix(".YouCtrl"))
                .filter_map(BasicLandType::from_code)
                .collect();
            return match (present, op, n) {
                _ if types.len() as usize == checked.len() && op == "EQ" && n == 0 => Enters::UnlessControl(types),
                ("Land.YouCtrl", "GT", n) => Enters::UnlessLandsAtMost(n),
                ("Land.YouCtrl", "GE", n) => Enters::UnlessLandsAtMost(n.saturating_sub(1)),
                ("Land.YouCtrl", "LT", n) => Enters::UnlessLandsAtLeast(n),
                ("Land.YouCtrl", "LE", n) => Enters::UnlessLandsAtLeast(n + 1),
                ("Land.Basic+YouCtrl", "LE", n) => Enters::UnlessBasics(n + 1),
                ("Land.Basic+YouCtrl", "LT", n) => Enters::UnlessBasics(n),
                ("Land.Basic+YouCtrl", "EQ", 0) => Enters::UnlessBasics(1),
                _ => {
                    // Unless you control a legendary creature, a planeswalker, three other
                    // Forests; unless an opponent controls eight lands.
                    self.note(
                        "a land that enters untapped under a condition the goldfish doesn't track counted as tapped",
                    );
                    Enters::Tapped
                }
            };
        }
        if let (Some(var), Some(compare)) = (param(t, "ConditionCheckSVar"), param(t, "ConditionSVarCompare")) {
            let value = face.svar(var).unwrap_or("");
            if value.contains("PlayerCountOpponents") {
                // Unless you have two or more opponents: a Commander pod has three.
                let opponents = 3;
                return match comparison(compare) {
                    Some((op, n)) if compares(opponents, op, n) => Enters::Tapped,
                    Some(_) => Enters::Untapped,
                    None => {
                        self.cannot("an enters-tapped condition it can't read");
                        Enters::Tapped
                    }
                };
            }
            if let Some(kind) = revealed {
                // Or one is out already, which the goldfish leaves out.
                return Enters::UnlessReveal(vec![kind.to_lowercase()]);
            }
            // Unless a player has 13 or less life (never, early on), unless you've gained life.
            self.note("a land that enters untapped under a condition the goldfish doesn't track counted as tapped");
        }
        Enters::Tapped
    }

    /// What the land does as it enters (`T:Mode$ ChangesZone | ValidCard$ Card.Self |
    /// Destination$ Battlefield`), when that's about lands.
    fn on_entering(&mut self, face: &Face) -> Option<Entering> {
        for exec in self_entering(face) {
            let e = params(exec);
            match e.first() {
                Some(&("DB", "ChangeZone")) if param(&e, "Destination") == Some("Hand") => {
                    if param(&e, "ChangeType").is_some_and(|c| c.starts_with("Land")) {
                        return Some(Entering::Bounce);
                    }
                }
                Some(&("DB", "Sacrifice")) => {
                    let what = param(&e, "SacValid");
                    match (what, param(&e, "UnlessCost")) {
                        (Some(v), None) if v.starts_with("Land") => {
                            let n = param(&e, "Amount").and_then(|n| n.parse().ok()).unwrap_or(1);
                            return Some(Entering::Sacrifice(n));
                        }
                        (_, Some(c)) if c.starts_with("Return<1/") => {
                            // Sacrificed unless a land goes back to hand: a bounce land.
                            self.note("a land kept by returning another counted as a bounce land");
                            return Some(Entering::Bounce);
                        }
                        (_, Some(_)) => {
                            self.note("a land kept by paying for it counted as free");
                        }
                        (None, None) => {
                            // Sacrificed for a land from the library (Riveteers Overlook).
                            let fetched = chain(face, exec).into_iter().find_map(|step| {
                                let s = params(step);
                                (s.first() == Some(&("DB", "ChangeZone")) && param(&s, "Origin") == Some("Library"))
                                    .then(|| fetch_kind(&s))
                                    .flatten()
                            });
                            if let Some(kind) = fetched {
                                return Some(Entering::Fetch(kind));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// A fetch land: sacrificed (`Sac<1/CARDNAME>`) for a land from the library.
    fn fetch(&mut self, face: &Face) -> Option<LandKind> {
        for line in face.lines_with("A") {
            let p = params(line);
            if p.first() != Some(&("AB", "ChangeZone")) || param(&p, "Origin") != Some("Library") {
                continue;
            }
            let cost = Cost::parse(param(&p, "Cost").unwrap_or(""));
            if !cost.sacrifices_itself || param(&p, "Destination") != Some("Battlefield") {
                continue;
            }
            if cost.mana_in > 0 {
                self.note("mana paid to fetch left out");
            }
            if param(&p, "ChangeNum").is_some_and(|n| n != "1") {
                self.note("a fetch finding more than one land counted as finding one");
            }
            match fetch_kind(&p) {
                Some(kind) => return Some(kind),
                None => self.cannot("a fetch finding lands it can't read"),
            }
        }
        None
    }

    fn effect(&mut self, card: &Card, face: &Face, types: &Types, type_line: &str, oracle: &Profile) -> Effect {
        let aura = card.front_type_line().contains("Aura");
        if aura {
            if let Some(units) = self.aura_mana(face) {
                return Effect::Rock { units, next_turn: false, once: false };
            }
        } else if types.is_permanent() && !types.is(CardType::Planeswalker) {
            let makes = self.makes(face, types, type_line);
            if !makes.units.is_empty() {
                let creature = types.is(CardType::Creature);
                let haste = face.keywords().any(|k| k == "Haste");
                return Effect::Rock {
                    units: makes.units,
                    next_turn: (creature && !haste) || enters_tapped(face),
                    once: doesnt_untap(face),
                };
            }
        }
        if let Some(ramp) = self.land_ramp(face, types) {
            return ramp;
        }
        // Card draw: from oracle text, unless it reads as mana the script doesn't make.
        match oracle.spell.as_ref().map(|s| &s.effect) {
            Some(e @ (Effect::Draw { .. } | Effect::Engine)) => e.clone(),
            _ => Effect::Other,
        }
    }

    /// An Aura's extra mana from the land it enchants (Wild Growth, Utopia Sprawl).
    fn aura_mana(&mut self, face: &Face) -> Option<Vec<Unit>> {
        for line in face.lines_with("T") {
            let p = params(line);
            if p.first() != Some(&("Mode", "TapsForMana")) || param(&p, "ValidCard") != Some("Card.AttachedBy") {
                continue;
            }
            let exec = params(face.svar(param(&p, "Execute")?)?);
            if exec.first() != Some(&("DB", "Mana")) {
                continue;
            }
            return match self.produced(&exec, face) {
                Some(Some(units)) => Some(units),
                _ => {
                    self.cannot("an Aura's mana it can't read");
                    None
                }
            };
        }
        None
    }

    /// Lands put onto the battlefield from the library: by the spell, as the permanent enters,
    /// or by sacrificing it (Sakura-Tribe Elder, Wayfarer's Bauble).
    fn land_ramp(&mut self, face: &Face, types: &Types) -> Option<Effect> {
        let permanent = types.is_permanent();
        // Where a search starts, the mana paid to activate it, and whether it waits for a
        // Saga's chapter.
        let mut starts: Vec<(&str, u32, bool)> = Vec::new();
        for line in face.lines_with("A") {
            let p = params(line);
            let cost = Cost::parse(param(&p, "Cost").unwrap_or(""));
            match p.first() {
                Some(("SP", _)) if !permanent => starts.push((line, 0, false)),
                // Sacrificing it (Sakura-Tribe Elder) or tapping it once (Hithlain Rope); not
                // one that needs something else.
                Some(&("AB", "ChangeZone")) if permanent && (cost.sacrifices_itself || cost.taps) && !cost.other => {
                    starts.push((line, cost.mana_in as u32, false));
                }
                _ => {}
            }
        }
        starts.extend(self_entering(face).into_iter().chain(on_casting(face)).map(|exec| (exec, 0, false)));
        for chapters in face.keywords().filter_map(|k| k.strip_prefix("Chapter:")) {
            // Chapter I comes as the Saga enters; the later ones a turn apart.
            let names = chapters.split(':').nth(1).unwrap_or("");
            starts.extend(names.split(',').enumerate().filter_map(|(i, n)| Some((face.svar(n)?, 0, i > 0))));
        }
        for (start, activation, chapter) in starts {
            let mut found = Found::default();
            for step in chain(face, start) {
                found.step(&params(step), face, self);
            }
            let sacrifice = found.sacrificed;
            let Some(finds) = found.finds else { continue };
            let to_battlefield = found.to_battlefield;
            if to_battlefield == 0 || u32::from(to_battlefield) <= sacrifice {
                continue;
            }
            if chapter {
                self.note("a land from a Saga's later chapter counted as coming at once");
            }
            return Some(Effect::LandRamp {
                finds,
                to_battlefield,
                to_hand: found.to_hand,
                tapped: found.tapped,
                activation,
                sacrifice: sacrifice as u8,
            });
        }
        None
    }
}

const BASIC_TYPES: &[&str] = &["Plains", "Island", "Swamp", "Mountain", "Forest"];

/// What ramp the goldfish doesn't play does, by the lines of its script.
fn left_out(script: &Script) -> LeftOut {
    let lines = || script.faces.iter().flat_map(|f| f.lines.iter().map(String::as_str));
    let engine = lines().any(|l| {
        (l.contains("Mode$ TapsForMana") && !l.contains("ValidCard$ Card.AttachedBy"))
            || l.contains("AdjustLandPlays$")
            || l.contains("Mode$ ReduceCost")
            || l.contains("Mode$ UnspentMana")
            || (l.contains("AB$ Untap") && ["Land", "Snow"].iter().chain(BASIC_TYPES).any(|t| l.contains(t)))
            || ["Improvise", "Convoke", "Affinity"].iter().any(|k| l.contains(&format!("AddKeyword$ {k}")))
            // Mana from an ability or trigger that isn't a mana ability (Frontier Siege, an
            // Autogenerator), or given to other permanents (Cryptolith Rite; not an opponent's
            // permanent an Aura turns into a land, Imprisoned in the Moon).
            || l.contains("DB$ Mana")
            || (l.starts_with("S:") && l.contains("AddAbility$") && !l.contains("EnchantedBy") && !l.contains("AttachedBy"))
    });
    let one_shot = || {
        lines().any(|l| {
            ["treasure", "eldrazi_spawn", "eldrazi_scion", "gold"].iter().any(|t| l.contains(&format!("_{t}")))
                || l.starts_with("A:SP$ Mana")
                || (l.contains("AB$ Mana") && (l.contains("Sac<") || l.contains("SubCounter<")))
                || l.starts_with("K:Suspend")
        })
    };
    if engine {
        LeftOut::Engine
    } else if one_shot() {
        LeftOut::OneShot
    } else {
        LeftOut::Other
    }
}

/// What a land does about lands as it enters.
enum Entering {
    /// Returns a land to hand.
    Bounce,
    /// Sacrifices this many lands.
    Sacrifice(usize),
    /// Sacrifices itself for a land from the library.
    Fetch(LandKind),
}

/// A land search's steps, walked: `Library → Library` (Cultivate's two, remembered), then
/// `Library → Battlefield` and `Library → Hand`.
#[derive(Default)]
struct Found {
    finds: Option<LandFilter>,
    /// Lands looked for in a first step that sets them aside (Cultivate: two).
    set_aside: Option<u8>,
    to_battlefield: u8,
    to_hand: u8,
    tapped: bool,
    sacrificed: u32,
}

impl Found {
    fn step(&mut self, s: &[(&str, &str)], face: &Face, r: &mut Reader) {
        let kind = s.first().map(|(_, v)| *v);
        if kind == Some("Sacrifice") && param(s, "SacValid").is_some_and(|v| v.starts_with("Land")) {
            self.sacrificed += param(s, "Amount").and_then(|n| n.parse().ok()).unwrap_or(1);
            return;
        }
        // A land sacrificed to do it (Springbloom Druid's trigger, Crop Rotation).
        self.sacrificed += Cost::parse(param(s, "Cost").unwrap_or("")).sacrifices_lands;
        if param(s, "UnlessSwitched").is_some() {
            self.sacrificed += Cost::parse(param(s, "UnlessCost").unwrap_or("")).sacrifices_lands;
        }
        if kind != Some("ChangeZone") || param(s, "Origin") != Some("Library") {
            return;
        }
        // Someone else's search (Path to Exile's, Tempt with Discovery's opponents'); each
        // player's (`Player`: Collective Voyage) is ours too.
        if param(s, "DefinedPlayer").is_some_and(|p| p != "You" && p != "Player") {
            r.theirs = true;
            return;
        }
        if s.iter().any(|(k, _)| k.starts_with("Condition")) {
            // Only with a Wastes out (Ruin in Their Wake), or if an opponent has more lands.
            r.note("a search that waits for something counted as always happening");
        }
        let change_type = param(s, "ChangeType").unwrap_or("");
        let remembered = change_type.contains("IsRemembered") || param(s, "Defined") == Some("Remembered");
        if !remembered {
            match land_filter(change_type) {
                Some(found) => {
                    if found.leaves_out {
                        r.note("lands of other kinds a search could find left out");
                    }
                    if found.wider {
                        r.note("a search for lands of a kind the goldfish doesn't track (snow, nonbasic) counted as one for any land");
                    }
                    self.finds.get_or_insert(found.filter);
                }
                None => return,
            }
        }
        let n = match param(s, "ChangeNum") {
            None => None,
            Some(n) => match n.parse() {
                Ok(n) => Some(n),
                Err(_) => match face.svar(n).and_then(unkicked) {
                    // Kicked or with spell mastery, more (Grow from the Ashes, Nissa's
                    // Pilgrimage): a goldfish's first turns have neither.
                    Some(n) => Some(n),
                    None if n == "X" || face.svar(n).is_some() => {
                        r.note("a search for X lands counted as two");
                        Some(2)
                    }
                    None => {
                        // As many as opponents searched for (Tempt with Discovery).
                        r.note("lands a search finds depending on the game left out");
                        return;
                    }
                },
            },
        };
        match param(s, "Destination") {
            Some("Library") => self.set_aside = Some(n.unwrap_or(1)),
            Some("Battlefield") => {
                self.to_battlefield += n.unwrap_or(1);
                self.tapped |= param(s, "Tapped") == Some("True");
            }
            Some("Hand") => {
                let rest = self.set_aside.map(|all| all.saturating_sub(self.to_battlefield));
                self.to_hand += n.or(rest).unwrap_or(1);
            }
            _ => {}
        }
    }
}

/// What an ability costs, as far as mana goes.
#[derive(Default)]
struct Cost {
    /// Mana it takes in: `1`, `W`, `UR` (hybrid), `2` …
    mana_in: usize,
    taps: bool,
    life: bool,
    sacrifices_itself: bool,
    /// Lands it sacrifices (`Sac<1/Land>`).
    sacrifices_lands: u32,
    /// Removes a charge counter (Vivid lands, Sphere of the Suns).
    charge_counter: bool,
    /// Something else: counters, other permanents, cards, energy, X.
    other: bool,
}

impl Cost {
    fn parse(cost: &str) -> Cost {
        let mut c = Cost::default();
        for t in cost.split_whitespace() {
            match t {
                "T" => c.taps = true,
                _ if t.starts_with("PayLife<") => c.life = true,
                _ if t.starts_with("Sac<1/CARDNAME") => c.sacrifices_itself = true,
                // Milling a card costs a goldfish nothing (Millikin).
                _ if t.starts_with("Mill<") => {}
                // Depletion counters too (Hickory Woodlot: two uses, then it's sacrificed).
                "SubCounter<1/CHARGE>" | "SubCounter<1/DEPLETION>" => c.charge_counter = true,
                _ if t.starts_with("Sac<") && t.contains("/Land") => {
                    c.sacrifices_lands += t[4..].split('/').next().and_then(|n| n.parse().ok()).unwrap_or(1);
                }
                _ if t.chars().all(|ch| ch.is_ascii_digit()) => c.mana_in += t.parse::<usize>().unwrap_or(0),
                // W, U, B, R, G, C, S; hybrid UR, W/U, 2/W; Phyrexian B/P.
                _ if t.chars().all(|ch| "WUBRGCS/P2".contains(ch)) => c.mana_in += 1,
                _ => c.other = true,
            }
        }
        c
    }
}

/// The value of a count that's one thing when kicked (or a condition holds) and another when not:
/// `Count$Kicked.2.1` and `Count$Compare Y GE2.3.2` are 1 and 2.
fn unkicked(svar: &str) -> Option<u8> {
    let rest = svar.strip_prefix("Count$Kicked.").or_else(|| svar.strip_prefix("Count$Compare "))?;
    rest.rsplit('.').next()?.parse().ok()
}

/// `GE5` → `("GE", 5)`.
fn comparison(c: &str) -> Option<(&str, usize)> {
    let (op, n) = c.split_at(c.len().min(2));
    Some((op, n.parse().ok()?))
}

fn compares(value: usize, op: &str, n: usize) -> bool {
    match op {
        "EQ" => value == n,
        "NE" => value != n,
        "LT" => value < n,
        "LE" => value <= n,
        "GT" => value > n,
        "GE" => value >= n,
        _ => false,
    }
}

/// A mana ability, its own or given to it by itself.
struct ManaLine<'a> {
    ability: &'a str,
    /// Given by a static ability to permanents of a kind, itself among them (Inga and Esika:
    /// creatures you control tap for mana).
    granted: bool,
    /// Given to those with a counter, which it can give itself (Rishkar, Peema Renegade).
    needs_counter: bool,
}

/// What a land's own fetch costs in mana (`A:AB$ ChangeZone | Cost$ 2 T Sac<1/CARDNAME> | Origin$
/// Library | Destination$ Battlefield …`), if it has one.
fn fetch_cost(face: &Face) -> Option<usize> {
    face.lines_with("A").find_map(|line| {
        let p = params(line);
        let fetch = p.first() == Some(&("AB", "ChangeZone"))
            && param(&p, "Origin") == Some("Library")
            && param(&p, "Destination") == Some("Battlefield");
        // `EACH Forest & Plains`: one of each (Krosan Verge).
        let finds_lands = param(&p, "ChangeType")
            .is_some_and(|t| t.strip_prefix("EACH ").unwrap_or(t).split(" & ").any(|t| land_filter(t).is_some()));
        let cost = Cost::parse(param(&p, "Cost").unwrap_or(""));
        (fetch && finds_lands && cost.sacrifices_itself).then_some(cost.mana_in)
    })
}

/// How a land takes a land type as it enters, which its mana comes from.
enum TakesType {
    /// A basic land type chosen (`S:Mode$ Continuous | AffectedDefined$ Self | AddType$ ChosenType`).
    Chosen,
    /// Another land's, as a copy of it (`K:ETBReplacement:Copy:…` with `DB$ Clone | Choices$ Land…`).
    Copy,
}

fn takes_a_type(face: &Face) -> Option<TakesType> {
    let chosen = face.lines_with("S").any(|line| {
        let p = params(line);
        p.first() == Some(&("Mode", "Continuous"))
            && param(&p, "AffectedDefined") == Some("Self")
            && param(&p, "AddType") == Some("ChosenType")
    });
    if chosen {
        return Some(TakesType::Chosen);
    }
    land_copy(face).map(|_| TakesType::Copy)
}

/// A land's copy of another land as it enters (Vesuva): its `DB$ Clone` step.
fn land_copy(face: &Face) -> Option<&str> {
    face.keywords().find_map(|k| {
        let svar = k.strip_prefix("ETBReplacement:Copy:")?.split(':').next()?;
        let step = face.svar(svar)?;
        let p = params(step);
        // Not one in a graveyard (Echoing Deeps), empty in a goldfish.
        let on_battlefield = param(&p, "ChoiceZone").is_none_or(|z| z == "Battlefield");
        let copies_land = param(&p, "Choices").is_some_and(|c| c.starts_with("Land"));
        (p.first() == Some(&("DB", "Clone")) && copies_land && on_battlefield).then_some(step)
    })
}

/// Whether it enters tapped as a copy of another land (Vesuva's `IntoPlayTapped$ True`).
fn copies_tapped(face: &Face) -> bool {
    land_copy(face).is_some_and(|step| param(&params(step), "IntoPlayTapped") == Some("True"))
}

/// Whether a static ability gives a mana ability to other permanents (Chromatic Lantern: lands you
/// control; Abundant Growth: the enchanted land), or to itself only while something holds (The
/// World Tree, with six lands).
fn gives_others_mana(face: &Face) -> bool {
    face.lines_with("S").any(|line| {
        let p = params(line);
        let Some(added) = param(&p, "AddAbility") else { return false };
        let others = param(&p, "Affected").is_some_and(|a| a != "Card.Self")
            || param(&p, "AffectedDefined").is_some_and(|a| a != "Self");
        p.first() == Some(&("Mode", "Continuous"))
            && others
            && added.split('&').filter_map(|n| face.svar(n.trim())).any(|a| a.starts_with("AB$ Mana"))
    })
}

/// The face's mana abilities: its `A:` lines, and those its `S:Mode$ Continuous` abilities give
/// to permanents it is one of.
fn mana_lines<'a>(face: &'a Face, types: &Types, type_line: &str) -> Vec<ManaLine<'a>> {
    let own = |ability| ManaLine { ability, granted: false, needs_counter: false };
    let mut lines: Vec<ManaLine> = face.lines_with("A").map(own).collect();
    let words: Vec<&str> = type_line.split_whitespace().collect();
    for line in face.lines_with("S") {
        let p = params(line);
        if p.first() != Some(&("Mode", "Continuous"))
            || p.iter().any(|(k, _)| k.starts_with("Condition") || matches!(*k, "CheckSVar" | "IsPresent"))
        {
            continue;
        }
        let (Some(affected), Some(added)) = (param(&p, "Affected"), param(&p, "AddAbility")) else { continue };
        // `Creature.YouCtrl`, `Ally.YouCtrl`, `Sliver`, `Creature.YouCtrl+HasCounters`, `Card.Self`.
        let mut covers_itself = None;
        for a in affected.split(',') {
            let (kind, props) = a.split_once('.').unwrap_or((a, ""));
            let is_one = matches!(kind, "Card" | "Permanent")
                || CardType::from_code(kind).is_some_and(|t| types.is(t))
                || words.contains(&kind);
            let props: Vec<&str> = props.split('+').filter(|p| !p.is_empty()).collect();
            let counter = props.iter().any(|p| *p == "HasCounters" || p.starts_with("counters_GE1"));
            let fits =
                props.iter().all(|p| matches!(*p, "YouCtrl" | "Self" | "HasCounters") || p.starts_with("counters_GE1"));
            if is_one && fits {
                covers_itself = Some(counter);
                break;
            }
        }
        if let Some(needs_counter) = covers_itself {
            let granted = |ability| ManaLine { ability, granted: true, needs_counter };
            lines.extend(added.split('&').filter_map(|n| face.svar(n.trim())).map(granted));
        }
    }
    lines
}

/// What this face's triggers do as it enters the battlefield.
fn self_entering(face: &Face) -> Vec<&str> {
    face.lines_with("T")
        .filter_map(|line| {
            let p = params(line);
            let entering = p.first() == Some(&("Mode", "ChangesZone"))
                && param(&p, "ValidCard") == Some("Card.Self")
                && param(&p, "Destination") == Some("Battlefield");
            entering.then(|| face.svar(param(&p, "Execute")?)).flatten()
        })
        .collect()
}

/// What this face's triggers do when it's cast (Sowing Mycospawn), unkicked.
fn on_casting(face: &Face) -> Vec<&str> {
    face.lines_with("T")
        .filter_map(|line| {
            let p = params(line);
            let cast = p.first() == Some(&("Mode", "SpellCast")) && param(&p, "ValidCard") == Some("Card.Self");
            cast.then(|| face.svar(param(&p, "Execute")?)).flatten()
        })
        .collect()
}

/// An ability and the steps after it, in order (at most 12): its `SubAbility$` chain, and what an
/// immediate trigger in it does (`DB$ ImmediateTrigger | Execute$ …`: "sacrifice it. When you do,
/// search…", the New Capenna lands) before the steps after that trigger.
fn chain<'a>(face: &'a Face, start: &'a str) -> Vec<&'a str> {
    let mut steps = Vec::new();
    let mut next = vec![start];
    while let Some(at) = next.pop() {
        if steps.len() == 12 {
            break;
        }
        steps.push(at);
        let p = params(at);
        next.extend(param(&p, "SubAbility").and_then(|s| face.svar(s)));
        if p.first() == Some(&("DB", "ImmediateTrigger")) {
            next.extend(param(&p, "Execute").and_then(|s| face.svar(s)));
        }
    }
    steps
}

/// A mana ability that makes a choice first (Nykthos: `AB$ ChooseColor | Cost$ 2 T | SubAbility$
/// DBMana`, then `DB$ Mana | Produced$ Chosen | Amount$ X`), as one `AB$ Mana` with the first
/// step's cost and the mana step's mana. Not one that does something first (Deathrite Shaman
/// exiles a land from a graveyard), which may not be there to do.
fn mana_after(face: &Face, ability: &str) -> Option<String> {
    let p = params(ability);
    if !matches!(p.first(), Some(("AB", "ChooseColor" | "ChoosePlayer"))) {
        return None;
    }
    let mana = chain(face, ability).into_iter().skip(1).find(|step| params(step).first() == Some(&("DB", "Mana")))?;
    let own = p.iter().skip(1).filter(|(k, _)| *k != "SubAbility").map(|(k, v)| format!("{k}$ {v}"));
    let made = params(mana).into_iter().skip(1).filter(|(k, _)| *k != "SubAbility").map(|(k, v)| format!("{k}$ {v}"));
    Some(std::iter::once("AB$ Mana".to_string()).chain(own).chain(made).collect::<Vec<_>>().join(" | "))
}

/// A fetch from `ChangeZone` params.
fn fetch_kind(p: &[(&str, &str)]) -> Option<LandKind> {
    let finds = land_filter(param(p, "ChangeType")?)?.filter;
    Some(LandKind::Fetch { finds, tapped: param(p, "Tapped") == Some("True") })
}

/// The lands a search finds, as near as a [`LandFilter`] says it.
struct FoundLands {
    filter: LandFilter,
    /// It finds other cards too, which the filter leaves out (`Land.Basic,Card.Gate`: Gates).
    leaves_out: bool,
    /// It finds only lands of a kind the filter can't say (`Land.Snow`, `Land.nonBasic`), so the
    /// filter finds more.
    wider: bool,
}

/// Which lands a `ChangeType$` finds. `None` if it finds no lands.
fn land_filter(change_type: &str) -> Option<FoundLands> {
    let (mut basic, mut types, mut any_type, mut lands) = (true, EnumSet::empty(), false, 0);
    let (mut leaves_out, mut wider) = (false, false);
    for alt in change_type.split(',') {
        let words: Vec<&str> = alt.split(['.', '+']).collect();
        let these: EnumSet<BasicLandType> = words.iter().filter_map(|w| BasicLandType::from_code(w)).collect();
        if !words.contains(&"Land") && these.is_empty() {
            leaves_out = true;
            continue;
        }
        lands += 1;
        basic &= words.contains(&"Basic");
        if these.is_empty() {
            any_type = true;
        }
        types |= these;
        if words.iter().any(|w| {
            !matches!(*w, "Land" | "Card" | "Basic" | "YouCtrl" | "YouOwn") && BasicLandType::from_code(w).is_none()
        }) {
            // Land.Snow, Land.nonBasic, Land.hasABasicLandType, …
            wider = true;
        }
    }
    let filter = LandFilter { basic, types: if any_type { EnumSet::empty() } else { types } };
    (lands > 0).then_some(FoundLands { filter, leaves_out, wider })
}

/// It makes your other spells cheaper (Animar, Goreclaw, the Medallions): `S:Mode$ ReduceCost` for
/// spells you cast, while it's on the battlefield or in the command zone; not one that makes
/// itself cheaper (Affinity) or that taxes what targets it.
fn reduces_costs(script: &Script) -> bool {
    script.front().lines_with("S").any(|line| {
        let p = params(line);
        p.first() == Some(&("Mode", "ReduceCost"))
            && param(&p, "Type") == Some("Spell")
            && !param(&p, "ValidCard").unwrap_or("").contains("Card.Self")
            && param(&p, "ValidTarget").is_none()
            && param(&p, "Activator").is_none_or(|a| a == "You" || a == "Player")
            && param(&p, "EffectZone").is_none_or(|z| z.split(',').any(|z| z == "Battlefield" || z == "Command"))
    })
}

/// One multikick's generic mana (`K:Multikicker:2`).
fn multikick(face: &Face) -> u32 {
    face.keywords().find_map(|k| k.strip_prefix("Multikicker:")).and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// Doesn't untap during its controller's untap step (Grim Monolith, Mana Vault).
fn doesnt_untap(face: &Face) -> bool {
    face.lines_with("R").any(|line| {
        let p = params(line);
        p.first() == Some(&("Event", "Untap"))
            && param(&p, "ValidCard") == Some("Card.Self")
            && param(&p, "Layer") == Some("CantHappen")
    })
}

/// A permanent that always enters tapped.
fn enters_tapped(face: &Face) -> bool {
    face.lines_with("R").any(|line| {
        let p = params(line);
        p.first() == Some(&("Event", "Moved"))
            && param(&p, "ValidCard") == Some("Card.Self")
            && param(&p, "ReplaceWith").and_then(|w| face.svar(w)).is_some_and(|s| {
                let t = params(s);
                t.first() == Some(&("DB", "Tap"))
                    && !t.iter().any(|(k, _)| k.starts_with("Condition") || k.starts_with("Unless"))
            })
    })
}
