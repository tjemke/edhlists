//! One solitaire game, and the totals over many.

use edh_data::flags::EnumSet;
use edh_data::mana::{ColorSet, Mana, ManaSet, Pip};
use edh_data::types::BasicLandType;

use crate::pay::pay;
use crate::profile::{Effect, Enters, Land, LandFilter, LandKind, Profile, Unit};
use crate::{DeckEntry, ManaReport, TURNS};

/// SplitMix64: small, fast, and the same everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

/// How a land drop ranks, best last: mana spent this turn, mana it adds for good, not using up
/// a modal card's spell side, entering tapped (while that costs nothing), kinds of mana the hand
/// needs, kinds of mana.
type LandRank = (u32, usize, bool, bool, u32, u32);

/// Going first in a four-player pod.
const ON_THE_PLAY: f64 = 0.25;
/// The first mulligan is free; after the third, keep whatever comes.
const MULLIGANS: usize = 3;
/// Lands out after which a modal card's spell side is worth more than its land side.
const ENOUGH_LANDS: usize = 7;

/// A deck, read once and played many times.
pub struct Deck {
    pub profiles: Vec<Profile>,
    /// The 99 (or whatever the list has), as profile indices.
    library: Vec<usize>,
    commanders: Vec<usize>,
    identity: ColorSet,
}

impl Deck {
    pub fn new(entries: &[DeckEntry]) -> Self {
        let identity = entries.iter().filter(|e| e.commander).fold(ColorSet::empty(), |id, e| id | e.card.identity);
        let mut deck = Deck { profiles: Vec::new(), library: Vec::new(), commanders: Vec::new(), identity };
        for e in entries {
            let i = deck.profiles.len();
            deck.profiles.push(Profile::of(e.card, identity));
            if e.commander {
                deck.commanders.push(i);
            } else {
                deck.library.extend(std::iter::repeat_n(i, e.qty as usize));
            }
        }
        deck
    }

    pub fn play(&self, rng: Rng) -> Record {
        Game::new(self, rng).play()
    }

    fn identity_kinds(&self) -> ManaSet {
        self.identity.iter().map(Mana::from).collect()
    }
}

/// What one game came to.
pub struct Record {
    mulligan: bool,
    commander_turn: Option<usize>,
    colour_delayed: bool,
    capacity: [u32; 8],
    spent_by_t8: u32,
    /// Profiles stuck on colours at least once by turn 8.
    stuck: Vec<usize>,
}

struct OnBattlefield {
    units: Vec<Unit>,
    with_lands: Option<(usize, Vec<Unit>)>,
    basic: bool,
    land_types: EnumSet<BasicLandType>,
    tapped: bool,
    card: usize,
}

struct Rock {
    units: Vec<Unit>,
    sick: bool,
}

/// What a land played this turn does once the turn's mana is spent.
enum AfterSpending {
    /// A bounce land (at this position) returns a land to hand.
    Bounce(usize),
    /// Lotus Field (at this position) sacrifices `n` lands.
    Sacrifice { land: usize, n: usize },
}

/// Where a spell is cast from.
#[derive(Clone, Copy)]
enum Slot {
    Hand(usize),
    Command(usize),
}

struct Game<'d> {
    deck: &'d Deck,
    rng: Rng,
    /// Top of the library last.
    library: Vec<usize>,
    hand: Vec<usize>,
    command: Vec<usize>,
    lands: Vec<OnBattlefield>,
    rocks: Vec<Rock>,
    engines: u32,
    mulligans: usize,
}

impl<'d> Game<'d> {
    fn new(deck: &'d Deck, mut rng: Rng) -> Self {
        let mut library = deck.library.clone();
        let mut mulligans = 0;
        let hand = loop {
            rng.shuffle(&mut library);
            let mut hand = library.split_off(library.len().saturating_sub(7));
            if mulligans == MULLIGANS || keepable(deck, &hand) {
                // London mulligan: put back one card per mulligan after the free one: extra lands
                // if there are plenty, else the most expensive spells.
                let back = mulligans.saturating_sub(1);
                let lands = hand.iter().filter(|&&c| is_land_card(deck, c)).count();
                hand.sort_by_key(|&c| {
                    if lands > 4 { u32::from(!is_land_card(deck, c)) } else { u32::MAX - mana_value(deck, c) }
                });
                let bottom: Vec<usize> = hand.drain(..back.min(hand.len())).collect();
                library.splice(0..0, bottom);
                break hand;
            }
            library.append(&mut hand);
            mulligans += 1;
        };
        Game {
            deck,
            rng,
            library,
            hand,
            command: deck.commanders.clone(),
            lands: Vec::new(),
            rocks: Vec::new(),
            engines: 0,
            mulligans,
        }
    }

    fn play(mut self) -> Record {
        let on_the_play = self.rng.chance(ON_THE_PLAY);
        let mut rec = Record {
            mulligan: self.mulligans > 0,
            commander_turn: None,
            colour_delayed: false,
            capacity: [0; 8],
            spent_by_t8: 0,
            stuck: Vec::new(),
        };
        for turn in 1..=TURNS {
            self.lands.iter_mut().for_each(|l| l.tapped = false);
            self.rocks.iter_mut().for_each(|r| r.sick = false);
            if !(turn == 1 && on_the_play) {
                self.draw(1);
            }
            self.draw(self.engines as usize);
            let then = self.land_drop();
            let mana = self.units();
            let (spent, cast_commander) = self.spend();
            if cast_commander && rec.commander_turn.is_none() {
                rec.commander_turn = Some(turn);
            }
            match then {
                Some(AfterSpending::Bounce(land)) => self.bounce(land),
                Some(AfterSpending::Sacrifice { land, n }) => {
                    let mut gone = self.least_useful_lands(n, Some(land));
                    gone.sort_unstable_by(|a, b| b.cmp(a));
                    gone.into_iter().for_each(|i| drop(self.lands.remove(i)));
                }
                None => {}
            }
            for c in self.stuck(&mana) {
                if self.deck.commanders.contains(&c) && rec.commander_turn.is_none() {
                    rec.colour_delayed = true;
                }
                if turn <= 8 && !rec.stuck.contains(&c) {
                    rec.stuck.push(c);
                }
            }
            if turn <= 8 {
                rec.spent_by_t8 += spent;
                rec.capacity[turn - 1] = self.capacity();
            }
        }
        rec
    }

    fn draw(&mut self, n: usize) {
        for _ in 0..n {
            if let Some(c) = self.library.pop() {
                self.hand.push(c);
            }
        }
    }

    fn profile(&self, c: usize) -> &'d Profile {
        &self.deck.profiles[c]
    }

    /// The mana available now: untapped lands and rocks that can tap.
    fn units(&self) -> Vec<Unit> {
        let n = self.lands.len();
        let lands = self.lands.iter().filter(|l| !l.tapped).flat_map(|l| land_units(&l.units, &l.with_lands, n));
        lands.chain(self.rocks.iter().filter(|r| !r.sick).flat_map(|r| r.units.iter())).copied().collect()
    }

    /// Every land's and rock's mana, tapped or not.
    fn capacity(&self) -> u32 {
        let n = self.lands.len();
        let lands: usize = self.lands.iter().map(|l| land_units(&l.units, &l.with_lands, n).len()).sum();
        (lands + self.rocks.iter().map(|r| r.units.len()).sum::<usize>()) as u32
    }

    /// Kinds of mana the hand and command zone ask for that nothing out makes yet; the deck's
    /// colours if none.
    fn needed(&self) -> ManaSet {
        let have = self.units().into_iter().fold(ManaSet::empty(), |a, u| a | u);
        let mut want = ManaSet::empty();
        for &c in self.hand.iter().chain(&self.command) {
            let Some(s) = &self.profile(c).spell else { continue };
            for pip in &s.cost.pips {
                want |= match *pip {
                    Pip::One(m) => m.into(),
                    Pip::Hybrid(a, b) => ManaSet::of(a) | b.into(),
                    Pip::TwoOr(c) | Pip::Phyrexian(c) => Mana::from(c).into(),
                    Pip::PhyrexianHybrid(a, b) => ManaSet::of(a.into()) | Mana::from(b).into(),
                    Pip::Snow | Pip::X => ManaSet::empty(),
                };
            }
        }
        let missing = ManaSet::from_iter(want.iter().filter(|m| !have.contains(*m)));
        if missing.is_empty() { self.deck.identity_kinds() } else { missing }
    }

    fn enters_tapped(&self, land: &Land, card: usize) -> bool {
        match &land.enters {
            Enters::Untapped | Enters::PayLife => false,
            Enters::Tapped => true,
            Enters::UnlessControl(types) => !self.lands.iter().any(|l| l.land_types.intersects(*types)),
            Enters::UnlessReveal(words) => {
                let mut others = self.hand.iter().filter(|&&c| c != card);
                !others.any(|&c| words.iter().any(|w| self.profile(c).type_words.contains(w.as_str())))
            }
            Enters::UnlessBasics(n) => self.lands.iter().filter(|l| l.basic).count() < *n,
            Enters::UnlessLandsAtLeast(n) => self.lands.len() < *n,
            Enters::UnlessLandsAtMost(n) => self.lands.len() > *n,
        }
    }

    /// The library's best land for `filter`: the one making most of the kinds needed.
    fn best_in_library(&self, filter: LandFilter) -> Option<(usize, &'d Land)> {
        let need = self.needed();
        let mut best: Option<(usize, &Land, usize)> = None;
        for (pos, &c) in self.library.iter().enumerate() {
            for face in &self.profile(c).lands {
                let LandKind::Mana(land) = &face.kind else { continue };
                if face.mdfc_back || !filter.allows(land) {
                    continue;
                }
                let kinds = land.units.iter().fold(ManaSet::empty(), |a, u| a | *u);
                let score = (kinds & need).len() as usize * 10 + kinds.len() as usize;
                if best.is_none_or(|(_, _, s)| score > s) {
                    best = Some((pos, land, score));
                }
            }
        }
        best.map(|(pos, land, _)| (pos, land))
    }

    fn fetch(&mut self, filter: LandFilter, tapped: bool) {
        if let Some((pos, land)) = self.best_in_library(filter) {
            let card = self.library.remove(pos);
            let tapped = tapped || self.enters_tapped(land, card);
            self.put(land, card, tapped);
        }
    }

    fn put(&mut self, land: &Land, card: usize, tapped: bool) {
        self.lands.push(OnBattlefield {
            units: land.units.clone(),
            with_lands: land.with_lands.clone(),
            basic: land.basic,
            land_types: land.land_types,
            tapped,
            card,
        });
    }

    /// Play the land (or land face) that lets the most mana be spent this turn; on a tie, keep a
    /// modal card's spell side, play a tapped land while it costs nothing, and prefer the colours
    /// the hand needs. A bounce land waits for another land to return, Lotus Field for two to give
    /// up. Returns what the land does
    /// once the turn's mana is spent: a bounce land returns a land, Lotus Field sacrifices two
    /// (their mana is used first, as a player taps them before the trigger resolves).
    fn land_drop(&mut self) -> Option<AfterSpending> {
        let need = self.needed();
        let base = self.units();
        // (what it ranks by, its position in hand, which face)
        let mut best: Option<(LandRank, usize, usize)> = None;
        for (pos, &c) in self.hand.iter().enumerate() {
            if self.hand[..pos].contains(&c) {
                continue; // another copy, already weighed
            }
            for (face_i, face) in self.profile(c).lands.iter().enumerate() {
                let out = self.lands.len() + 1;
                let makes = |land: &Land| land_units(&land.units, &land.with_lands, out).to_vec();
                let (tapped, new) = match &face.kind {
                    // With no other land out, a bounce land would return itself; without enough to
                    // give up, Lotus Field would go too.
                    LandKind::Mana(land) if land.bounce && self.lands.is_empty() => continue,
                    LandKind::Mana(land) if self.lands.len() < land.sacrifices => continue,
                    LandKind::Mana(land) => (self.enters_tapped(land, c), makes(land)),
                    LandKind::Fetch { finds, tapped } => match self.best_in_library(*finds) {
                        Some((_, land)) => (*tapped || self.enters_tapped(land, c), makes(land)),
                        None => (true, vec![]),
                    },
                };
                // What it adds to the deck's mana for good: a bounce land less the land it returns,
                // Lotus Field less the lands it sacrifices.
                let returned = match &face.kind {
                    LandKind::Mana(land) if land.bounce => {
                        self.lands.get(self.to_return(None)).map_or(0, |l| l.units.len())
                    }
                    LandKind::Mana(land) if land.sacrifices > 0 => {
                        let gone = self.least_useful_lands(land.sacrifices, None);
                        let lost: usize = gone.iter().map(|&i| self.lands[i].units.len()).sum();
                        // Without enough lands to give up, it goes itself.
                        if gone.len() < land.sacrifices { new.len() + lost } else { lost }
                    }
                    _ => 0,
                };
                let gain = new.len().saturating_sub(returned);
                let mut units = base.clone();
                if !tapped {
                    units.extend(&new);
                }
                let mut hand = self.hand.clone();
                hand.remove(pos);
                let spent = self.dry_spend(hand, units);
                let kinds = new.iter().fold(ManaSet::empty(), |a, u| a | *u);
                let key = (spent, gain, !face.mdfc_back, tapped, (kinds & need).len(), kinds.len());
                if best.as_ref().is_none_or(|(k, _, _)| key > *k) {
                    best = Some((key, pos, face_i));
                }
            }
        }
        let (_, pos, face_i) = best?;
        let c = self.hand.remove(pos);
        match &self.profile(c).lands[face_i].kind {
            LandKind::Mana(land) => {
                let tapped = self.enters_tapped(land, c);
                self.put(land, c, tapped);
                let new = self.lands.len() - 1;
                if land.sacrifices > 0 {
                    Some(AfterSpending::Sacrifice { land: new, n: land.sacrifices })
                } else {
                    land.bounce.then_some(AfterSpending::Bounce(new))
                }
            }
            LandKind::Fetch { finds, tapped } => {
                self.fetch(*finds, *tapped);
                None
            }
        }
    }

    /// The `n` lands least worth keeping (making the least mana, then the fewest kinds of it), by
    /// position; `spare` only if there aren't enough others (Lotus Field sacrificing itself).
    fn least_useful_lands(&self, n: usize, spare: Option<usize>) -> Vec<usize> {
        let worth = |i: usize| {
            let l = &self.lands[i];
            (l.units.len(), l.units.iter().fold(ManaSet::empty(), |a, u| a | *u).len())
        };
        let mut others: Vec<usize> = (0..self.lands.len()).filter(|&i| Some(i) != spare).collect();
        others.sort_by_key(|&i| worth(i));
        others.truncate(n);
        if others.len() < n
            && let Some(s) = spare
        {
            others.push(s);
        }
        others
    }

    /// The land a bounce land returns: the one making the least mana, then the fewest kinds of it;
    /// never `except` (the bounce land itself), unless it's alone.
    fn to_return(&self, except: Option<usize>) -> usize {
        (0..self.lands.len())
            .filter(|&i| Some(i) != except)
            .min_by_key(|&i| {
                let l = &self.lands[i];
                (l.units.len(), l.units.iter().fold(ManaSet::empty(), |a, u| a | *u).len())
            })
            .or(except)
            .unwrap_or(0)
    }

    fn bounce(&mut self, bounce_land: usize) {
        let land = self.lands.remove(self.to_return(Some(bounce_land)));
        self.hand.push(land.card);
    }

    /// The next spell to cast and the units left after paying for it: ramp first (cheapest
    /// first), then a commander, then card draw (cheapest first), then the biggest spell.
    /// X spells wait for [`Game::soak`]. A modal card with a land back stays a land while the
    /// deck still needs land drops: no other land in hand and fewer than [`ENOUGH_LANDS`] out.
    fn next_cast(&self, hand: &[usize], command: &[usize], units: &[Unit]) -> Option<(Slot, Vec<Unit>)> {
        let mut options: Vec<(u32, i64, Slot)> = Vec::with_capacity(hand.len() + command.len());
        let land_cards = hand.iter().filter(|&&c| is_land_card(self.deck, c)).count();
        let keep_lands = self.lands.len() < ENOUGH_LANDS;
        for (pos, &c) in hand.iter().enumerate() {
            let Some(s) = &self.profile(c).spell else { continue };
            if (s.has_x() && !s.x_rock()) || (keep_lands && is_land_card(self.deck, c) && land_cards < 2) {
                continue;
            }
            let mv = i64::from(s.mana_value());
            let (group, order) = match s.effect {
                Effect::Rock { .. } | Effect::LandRamp { .. } => (0, mv),
                Effect::Draw { .. } | Effect::Engine => (2, mv),
                Effect::Other => (3, -mv),
            };
            options.push((group, order, Slot::Hand(pos)));
        }
        for pos in 0..command.len() {
            options.push((1, 0, Slot::Command(pos)));
        }
        options.sort_by_key(|(g, o, _)| (*g, *o));
        options.into_iter().find_map(|(_, _, slot)| {
            let c = match slot {
                Slot::Hand(p) => hand[p],
                Slot::Command(p) => command[p],
            };
            let s = self.profile(c).spell.as_ref()?;
            pay(units, &s.cost, s.extra()).map(|left| (slot, left))
        })
    }

    /// How much this hand would spend with these units, rocks counted as they come in.
    fn dry_spend(&self, mut hand: Vec<usize>, mut units: Vec<Unit>) -> u32 {
        let mut command = self.command.clone();
        let mut spent = 0;
        while let Some((slot, left)) = self.next_cast(&hand, &command, &units) {
            let c = match slot {
                Slot::Hand(p) => hand.remove(p),
                Slot::Command(p) => command.remove(p),
            };
            units = left;
            let s = self.profile(c).spell.as_ref().expect("cast");
            spent += s.mana_value() + s.extra();
            if let Effect::Rock { units: made, next_turn: false, .. } = &s.effect {
                units.extend(made);
            }
        }
        spent + soak(self.deck, &hand, &units).map_or(0, |(_, n)| n)
    }

    /// Cast what can be cast, one spell at a time, each taking effect before the next. Returns
    /// the mana value cast and whether a commander was.
    fn spend(&mut self) -> (u32, bool) {
        let mut units = self.units();
        let mut spent = 0;
        let mut commander = false;
        while let Some((slot, left)) = self.next_cast(&self.hand, &self.command, &units) {
            units = left;
            let c = match slot {
                Slot::Hand(p) => self.hand.remove(p),
                Slot::Command(p) => {
                    commander = true;
                    self.command.remove(p)
                }
            };
            let s = self.profile(c).spell.as_ref().expect("cast");
            spent += s.mana_value() + s.extra();
            match &s.effect {
                Effect::Rock { units: made, next_turn, once } => {
                    if !once {
                        self.rocks.push(Rock { units: made.clone(), sick: *next_turn });
                    }
                    if !next_turn {
                        units.extend(made);
                    }
                }
                Effect::LandRamp { finds, to_battlefield, to_hand, tapped, sacrifice, .. } => {
                    for _ in 0..*sacrifice {
                        if let Some(i) = self.least_useful_lands(1, None).first() {
                            self.lands.remove(*i);
                        }
                    }
                    for _ in 0..*to_battlefield {
                        let before = self.lands.len();
                        self.fetch(*finds, *tapped);
                        if let Some(l) = self.lands.get(before).filter(|l| !l.tapped) {
                            units.extend(&l.units);
                        }
                    }
                    for _ in 0..*to_hand {
                        if let Some((pos, _)) = self.best_in_library(*finds) {
                            let card = self.library.remove(pos);
                            self.hand.push(card);
                        }
                    }
                }
                Effect::Draw { cards } => self.draw(usize::from(*cards)),
                Effect::Engine => self.engines += 1,
                Effect::Other => {}
            }
        }
        if let Some((pos, n)) = soak(self.deck, &self.hand, &units) {
            self.hand.remove(pos);
            spent += n;
        }
        (spent, commander)
    }

    /// Cards still in hand or the command zone that the turn's mana was enough for, but not its
    /// colours: judged on all of it, so mana spent on something else isn't taken for a colour
    /// problem.
    fn stuck(&self, units: &[Unit]) -> Vec<usize> {
        let mut out = Vec::new();
        for &c in self.hand.iter().chain(&self.command) {
            let Some(s) = &self.profile(c).spell else { continue };
            let total = s.mana_value() + s.extra();
            if total as usize <= units.len() && pay(units, &s.cost, s.extra()).is_none() {
                out.push(c);
            }
        }
        out
    }
}

/// An X spell in hand that the units left can cast, with X as large as they allow: its position
/// and the mana it takes (all of it).
fn soak(deck: &Deck, hand: &[usize], units: &[Unit]) -> Option<(usize, u32)> {
    hand.iter().enumerate().find_map(|(pos, &c)| {
        let s = deck.profiles[c].spell.as_ref().filter(|s| s.has_x() && !s.x_rock())?;
        pay(units, &s.cost, 0).map(|_| (pos, units.len() as u32))
    })
}

/// What a land makes with `lands` lands out.
fn land_units<'a>(units: &'a [Unit], with_lands: &'a Option<(usize, Vec<Unit>)>, lands: usize) -> &'a [Unit] {
    match with_lands {
        Some((n, more)) if lands >= *n => more,
        _ => units,
    }
}

fn is_land_card(deck: &Deck, c: usize) -> bool {
    !deck.profiles[c].lands.is_empty()
}

fn mana_value(deck: &Deck, c: usize) -> u32 {
    deck.profiles[c].spell.as_ref().map_or(0, |s| s.mana_value())
}

/// Worth keeping: three to five lands that make at least two of the deck's colours (or its one),
/// or two lands and a cheap rock (one that keeps making mana) or ramp spell.
fn keepable(deck: &Deck, hand: &[usize]) -> bool {
    let lands = hand.iter().filter(|&&c| is_land_card(deck, c)).count();
    let cheap_ramp = hand.iter().any(|&c| {
        deck.profiles[c].spell.as_ref().is_some_and(|s| {
            matches!(s.effect, Effect::Rock { once: false, .. } | Effect::LandRamp { .. }) && s.mana_value() <= 2
        })
    });
    let mut kinds = ManaSet::empty();
    for &c in hand {
        for face in &deck.profiles[c].lands {
            kinds |= match &face.kind {
                LandKind::Mana(l) => l.units.iter().fold(ManaSet::empty(), |a, u| a | *u),
                LandKind::Fetch { .. } => deck.identity_kinds(),
            };
        }
    }
    let colours = (kinds & deck.identity_kinds()).len();
    let enough = colours >= deck.identity.len().min(2);
    ((3..=5).contains(&lands) && enough) || (lands == 2 && cheap_ramp)
}

/// Sums over many games.
pub struct Totals {
    games: u32,
    mulligans: u32,
    commander_by_t4: u32,
    colour_delayed: u32,
    capacity: [u64; 8],
    spent_by_t8: u64,
    stuck: Vec<u32>,
}

impl Totals {
    pub fn new(profiles: usize) -> Self {
        Totals {
            games: 0,
            mulligans: 0,
            commander_by_t4: 0,
            colour_delayed: 0,
            capacity: [0; 8],
            spent_by_t8: 0,
            stuck: vec![0; profiles],
        }
    }

    pub fn add(&mut self, r: &Record) {
        self.games += 1;
        self.mulligans += u32::from(r.mulligan);
        self.commander_by_t4 += u32::from(r.commander_turn.is_some_and(|t| t <= 4));
        self.colour_delayed += u32::from(r.colour_delayed);
        for (t, c) in self.capacity.iter_mut().zip(r.capacity) {
            *t += u64::from(c);
        }
        self.spent_by_t8 += u64::from(r.spent_by_t8);
        for &c in &r.stuck {
            self.stuck[c] += 1;
        }
    }

    pub fn merge(mut self, other: Totals) -> Totals {
        self.games += other.games;
        self.mulligans += other.mulligans;
        self.commander_by_t4 += other.commander_by_t4;
        self.colour_delayed += other.colour_delayed;
        for (a, b) in self.capacity.iter_mut().zip(other.capacity) {
            *a += b;
        }
        self.spent_by_t8 += other.spent_by_t8;
        for (a, b) in self.stuck.iter_mut().zip(other.stuck) {
            *a += b;
        }
        self
    }

    pub fn report(&self, deck: &Deck) -> ManaReport {
        let n = f64::from(self.games.max(1));
        let share = |k: u32| f64::from(k) / n;
        let has_commander = !deck.commanders.is_empty();
        // A card in the list more than once is stuck as one card.
        let mut stuck: Vec<(String, f64)> = Vec::new();
        for (i, &k) in self.stuck.iter().enumerate() {
            if k > 0 {
                stuck.push((deck.profiles[i].name.clone(), share(k)));
            }
        }
        stuck.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ManaReport {
            games: self.games,
            mulligan: share(self.mulligans),
            commander_by_t4: has_commander.then(|| share(self.commander_by_t4)),
            colour_delayed: has_commander.then(|| share(self.colour_delayed)),
            capacity: self.capacity.map(|c| c as f64 / n),
            spent_by_t8: self.spent_by_t8 as f64 / n,
            stuck,
        }
    }
}
