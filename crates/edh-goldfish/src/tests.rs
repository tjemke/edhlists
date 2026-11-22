//! What the goldfish must get right, card by card and deck by deck.

use edh_data::cards::Card;
use edh_data::mana::{ColorSet, ManaSet};
use edh_data::tags::Role;
use edh_data::types::{BasicLandType, Layout};

use crate::profile::{Effect, Enters, Fidelity, Land, LandFilter, LandKind, Profile};
use crate::{DeckEntry, run, seed_for};

/// A card as the card database has it: `cost` and `produced` in Scryfall's notation ("{1}{U}",
/// "UB"); faces split with " // " in the name, type line and text.
fn card(name: &str, type_line: &str, cost: &str, produced: &str, text: &str) -> Card {
    let costs = cost.split(" // ").map(|c| c.parse().ok()).collect();
    Card {
        name: name.into(),
        faces: if name.contains(" // ") { name.split(" // ").map(String::from).collect() } else { vec![] },
        type_line: type_line.into(),
        oracle: text.split(" // ").map(String::from).collect(),
        mana_cost: costs,
        produced: ManaSet::from_codes(produced.split("").filter(|s| !s.is_empty())),
        ..Default::default()
    }
}

fn commander(name: &str, cost: &str, identity: &str) -> Card {
    Card {
        identity: ColorSet::from_letters(identity).unwrap(),
        ..card(name, "Legendary Creature — Spirit", cost, "", "")
    }
}

fn basic(kind: &str, color: &str) -> Card {
    card(kind, &format!("Basic Land — {kind}"), "", color, &format!("({{T}}: Add {{{color}}}.)"))
}

fn profile(c: &Card, identity: &str) -> Profile {
    Profile::of(c, None, ColorSet::from_letters(identity).unwrap())
}

fn land(p: &Profile) -> &Land {
    match &p.lands[0].kind {
        LandKind::Mana(l) => l,
        other => panic!("{} should be a mana land, is {other:?}", p.name),
    }
}

fn rock_units(p: &Profile) -> Vec<String> {
    match &p.spell.as_ref().expect("a spell").effect {
        Effect::Rock { units, .. } => units.iter().map(|u| u.to_string()).collect(),
        other => panic!("{} should be a rock, is {other:?}", p.name),
    }
}

/// A deck of `cmd` and these cards, `n` of each.
fn deck<'a>(cmd: &'a [Card], cards: &'a [(Card, u32)]) -> Vec<DeckEntry<'a>> {
    cmd.iter()
        .map(|c| DeckEntry { card: c, qty: 1, commander: true, script: None })
        .chain(cards.iter().map(|(c, n)| DeckEntry { card: c, qty: *n, commander: false, script: None }))
        .collect()
}

fn filler(cost: &str) -> Card {
    card("Vanilla", "Creature — Bear", cost, "", "")
}

// --- mana sources ---------------------------------------------------------------------------------

#[test]
fn signets_add_one_mana_of_their_colours_not_two() {
    let signet = card("Dimir Signet", "Artifact", "{2}", "UB", "{1}, {T}: Add {U}{B}.");
    assert_eq!(rock_units(&profile(&signet, "UB")), ["UB"]);
}

#[test]
fn talismans_make_one_mana_of_colourless_or_their_colours() {
    let talisman = card(
        "Talisman of Dominance",
        "Artifact",
        "{2}",
        "UBC",
        "{T}: Add {C}.\n{T}: Add {U} or {B}. Talisman of Dominance deals 1 damage to you.",
    );
    assert_eq!(rock_units(&profile(&talisman, "UB")), ["UBC"]);
}

#[test]
fn sol_ring_makes_two_colourless_and_dorks_wait_a_turn() {
    let sol = card("Sol Ring", "Artifact", "{1}", "C", "{T}: Add {C}{C}.");
    assert_eq!(rock_units(&profile(&sol, "G")), ["C", "C"]);
    let birds = card("Birds of Paradise", "Creature — Bird", "{G}", "WUBRG", "Flying\n{T}: Add one mana of any color.");
    let p = profile(&birds, "BG");
    assert_eq!(rock_units(&p), ["BG"], "any colour means any of the deck's");
    assert!(matches!(p.spell.unwrap().effect, Effect::Rock { next_turn: true, .. }));
}

#[test]
fn mana_for_life_or_after_choosing_a_colour_counts() {
    let translator = card(
        "Kozilek's Translator",
        "Creature — Eldrazi Drone",
        "{4}",
        "C",
        "Devoid\nPay 1 life: Add {C}. Activate only once each turn.",
    );
    assert_eq!(rock_units(&profile(&translator, "G")), ["C"]);
    let cornucopia = card(
        "Astral Cornucopia",
        "Artifact",
        "{X}{X}{X}",
        "WUBRG",
        "This artifact enters with X charge counters on it.\n{T}: Choose a color. Add one mana of that color for each charge counter on this artifact.",
    );
    assert_eq!(rock_units(&profile(&cornucopia, "BG")), ["BG"]);
}

#[test]
fn a_rock_with_x_in_its_cost_is_cast_with_x_as_one() {
    let cornucopia = card(
        "Astral Cornucopia",
        "Artifact",
        "{X}{X}{X}",
        "WUBRG",
        "This artifact enters with X charge counters on it.\n{T}: Choose a color. Add one mana of that color for each charge counter on this artifact.",
    );
    assert_eq!(profile(&cornucopia, "U").spell.unwrap().extra(), 3, "{{X}}{{X}}{{X}} with X = 1");
    let cmd = [commander("Teval", "{9}{U}", "U")];
    let plain = [(basic("Island", "U"), 38), (filler("{6}"), 61)];
    let rocks = [(basic("Island", "U"), 38), (cornucopia, 20), (filler("{6}"), 41)];
    let (a, b) = (run(&deck(&cmd, &plain), 1000, 1), run(&deck(&cmd, &rocks), 1000, 1));
    assert!(b.capacity[5] > a.capacity[5] + 0.5, "cast as rocks: {} vs {}", b.capacity[5], a.capacity[5]);
}

#[test]
fn mana_an_ability_takes_in_comes_off_what_it_makes() {
    let celebrant = card(
        "Blood Celebrant",
        "Creature — Human Cleric",
        "{B}",
        "WUBRG",
        "{B}, Pay 1 life: Add one mana of any color.",
    );
    assert_eq!(profile(&celebrant, "B").spell.unwrap().effect, Effect::Other, "{{B}} in, one mana out: no ramp");
    let bluffs = card("Cascade Bluffs", "Land", "", "URC", "{T}: Add {C}.\n{U/R}, {T}: Add {U}{U}, {U}{R}, or {R}{R}.");
    let bluffs_units = land(&profile(&bluffs, "UR")).units.iter().map(|u| u.to_string()).collect::<Vec<_>>();
    assert_eq!(bluffs_units, ["URC"], "a filter land is one mana a turn, of its colours or colourless");
    let shores = card("Unknown Shores", "Land", "", "WUBRGC", "{T}: Add {C}.\n{1}, {T}: Add one mana of any color.");
    let shores_units = land(&profile(&shores, "BG")).units.iter().map(|u| u.to_string()).collect::<Vec<_>>();
    assert_eq!(shores_units, ["BGC"], "{{1}} for any colour: no more mana, but it fixes");
    let selvala = card(
        "Selvala, Heart of the Wilds",
        "Legendary Creature — Elf Scout",
        "{1}{G}{G}",
        "WUBRG",
        "{G}, {T}: Add X mana in any combination of colors, where X is the greatest power among creatures you control.",
    );
    assert_eq!(rock_units(&profile(&selvala, "WG")).len(), 1, "X taken as 2, less the {{G}} it takes in");
    let pain = card("Llanowar Wastes", "Land", "", "BGC", "{T}: Add {C}.\n{T}, Pay 1 life: Add {B} or {G}.");
    assert_eq!(
        land(&profile(&pain, "BG")).units.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        ["BGC"],
        "tapping isn't mana taken in: a pain land makes its colours"
    );
}

#[test]
fn some_land_mana_waits_for_enough_lands_and_max_speed_never_comes() {
    let temple = card(
        "Temple of the False God",
        "Land",
        "",
        "C",
        "{T}: Add {C}{C}. Activate only if you control five or more lands.",
    );
    let t = profile(&temple, "");
    assert!(land(&t).units.is_empty(), "nothing before five lands");
    assert_eq!(land(&t).with_lands.as_ref().map(|(n, u)| (*n, u.len())), Some((5, 2)));
    let shrine = card(
        "Shrine of the Forsaken Gods",
        "Land",
        "",
        "C",
        "{T}: Add {C}.\n{T}: Add {C}{C}. Spend this mana only to cast colorless spells. Activate only if you control seven or more lands.",
    );
    let sh = profile(&shrine, "");
    assert_eq!((land(&sh).units.len(), land(&sh).with_lands.as_ref().map(|(n, u)| (*n, u.len()))), (1, Some((7, 2))));
    let raceway =
        card("Muraganda Raceway", "Land", "", "C", "Start your engines!\n{T}: Add {C}.\nMax speed — {T}: Add {C}{C}.");
    let r = profile(&raceway, "");
    assert_eq!((land(&r).units.len(), land(&r).with_lands.is_none()), (1, true));
    // Played out: the Temple adds nothing on turn 4 with four lands, and two once five are out.
    let cmd = [commander("Big", "{20}", "")];
    let wastes = card("Wastes", "Basic Land", "", "C", "{T}: Add {C}.");
    let only_temples = [(temple, 40), (filler("{9}"), 59)];
    let rep = run(&deck(&cmd, &only_temples), 200, 1);
    assert_eq!(rep.capacity[3], 0.0, "four Temples make nothing");
    let mixed = [(wastes, 40), (filler("{9}"), 59)];
    assert!(run(&deck(&cmd, &mixed), 200, 1).capacity[3] > 3.0);
}

#[test]
fn rocks_that_enter_tapped_or_dont_untap_or_need_kicking_cost_what_they_do() {
    let diamond = card("Charcoal Diamond", "Artifact", "{2}", "B", "This artifact enters tapped.\n{T}: Add {B}.");
    assert!(matches!(profile(&diamond, "B").spell.unwrap().effect, Effect::Rock { next_turn: true, once: false, .. }));
    let grim = card(
        "Grim Monolith",
        "Artifact",
        "{2}",
        "C",
        "This artifact doesn't untap during your untap step.\n{T}: Add {C}{C}{C}.\n{4}: Untap this artifact.",
    );
    assert!(matches!(profile(&grim, "B").spell.unwrap().effect, Effect::Rock { once: true, .. }));
    let chalice = card(
        "Everflowing Chalice",
        "Artifact",
        "{0}",
        "C",
        "Multikicker {2}\nThis artifact enters with a charge counter on it for each time it was kicked.\n{T}: Add {C} for each charge counter on this artifact.",
    );
    assert_eq!(profile(&chalice, "B").spell.unwrap().extra(), 2, "kicked once: {{2}} for one mana");
    // A deck of Grim Monoliths gets nothing lasting from them.
    let cmd = [commander("Big", "{20}", "")];
    let wastes = card("Wastes", "Basic Land", "", "C", "{T}: Add {C}.");
    let monoliths = [(wastes.clone(), 38), (grim, 30), (filler("{9}"), 31)];
    let plain = [(wastes, 38), (filler("{9}"), 61)];
    let (a, b) = (run(&deck(&cmd, &monoliths), 500, 1), run(&deck(&cmd, &plain), 500, 1));
    assert!((a.capacity[5] - b.capacity[5]).abs() < 0.3, "{} vs {}", a.capacity[5], b.capacity[5]);
}

#[test]
fn one_shot_mana_isnt_a_rock() {
    let petal =
        card("Lotus Petal", "Artifact", "{0}", "WUBRG", "{T}, Sacrifice Lotus Petal: Add one mana of any color.");
    assert_eq!(profile(&petal, "G").spell.unwrap().effect, Effect::Other);
}

#[test]
fn bounce_lands_make_two_and_return_a_land() {
    let chamber = card(
        "Simic Growth Chamber",
        "Land",
        "",
        "GU",
        "Simic Growth Chamber enters tapped.\nWhen Simic Growth Chamber enters, return a land you control to its owner's hand.\n{T}: Add {G}{U}.",
    );
    let p = profile(&chamber, "GU");
    let l = land(&p);
    assert_eq!(l.units.iter().map(|u| u.to_string()).collect::<Vec<_>>(), ["G", "U"]);
    assert!(l.bounce && l.enters == Enters::Tapped);
}

#[test]
fn a_bounce_land_waits_for_a_land_to_return() {
    let chamber = card(
        "Simic Growth Chamber",
        "Land",
        "",
        "GU",
        "Simic Growth Chamber enters tapped.\nWhen Simic Growth Chamber enters, return a land you control to its owner's hand.\n{T}: Add {G}{U}.",
    );
    // Only bounce lands: played alone, each would return itself, and the deck would never have a land.
    let cmd = [commander("Teval", "{3}{U}", "U")];
    let only = [(chamber.clone(), 40), (filler("{6}"), 59)];
    assert_eq!(run(&deck(&cmd, &only), 200, 1).capacity[3], 0.0, "nothing to return, so never played");
    let mixed = [(chamber, 10), (basic("Island", "U"), 30), (filler("{6}"), 59)];
    let plain = [(basic("Island", "U"), 40), (filler("{6}"), 59)];
    let (with, without) = (run(&deck(&cmd, &mixed), 1000, 1), run(&deck(&cmd, &plain), 1000, 1));
    assert!(
        with.capacity[5] > without.capacity[5],
        "bounce lands add mana: {} vs {}",
        with.capacity[5],
        without.capacity[5]
    );
}

// --- entering tapped ------------------------------------------------------------------------------

#[test]
fn lands_read_their_enters_tapped_conditions() {
    let enters = |text: &str| land(&profile(&card("L", "Land", "", "G", text), "G")).enters.clone();
    let types = |t: &[BasicLandType]| Enters::UnlessControl(t.iter().copied().collect());
    use BasicLandType::*;
    assert_eq!(
        enters("Woodland Cemetery enters tapped unless you control a Swamp or a Forest.\n{T}: Add {B} or {G}."),
        types(&[Swamp, Forest])
    );
    assert_eq!(enters("This land enters tapped unless you control an Island.\n{T}: Add {U}."), types(&[Island]));
    assert_eq!(
        enters("As Breeding Pool enters, you may pay 2 life. If you don't, it enters tapped.\n{T}: Add {G} or {U}."),
        Enters::PayLife
    );
    assert_eq!(
        enters(
            "As this land enters, you may reveal a Swamp or Forest card from your hand. If you don't, this land enters tapped."
        ),
        Enters::UnlessReveal(vec!["swamp".into(), "forest".into()])
    );
    assert_eq!(
        enters("This land enters tapped unless you control two or more other lands."),
        Enters::UnlessLandsAtLeast(2)
    );
    assert_eq!(
        enters("This land enters tapped unless you control two or fewer other lands."),
        Enters::UnlessLandsAtMost(2)
    );
    assert_eq!(
        enters("Canopy Vista enters tapped unless you control two or more basic lands."),
        Enters::UnlessBasics(2)
    );
    assert_eq!(enters("This land enters tapped unless you have two or more opponents."), Enters::Untapped);
    assert_eq!(enters("Opulent Palace enters tapped.\n{T}: Add {B}, {G}, or {U}."), Enters::Tapped);
    assert_eq!(enters("{T}: Add {G}."), Enters::Untapped);
}

// --- finding lands --------------------------------------------------------------------------------

#[test]
fn fetches_read_what_they_find() {
    let misty = card(
        "Misty Rainforest",
        "Land",
        "",
        "",
        "{T}, Pay 1 life, Sacrifice Misty Rainforest: Search your library for a Forest or Island card, put it onto the battlefield, then shuffle.",
    );
    let wilds = card(
        "Evolving Wilds",
        "Land",
        "",
        "",
        "{T}, Sacrifice Evolving Wilds: Search your library for a basic land card, put it onto the battlefield tapped, then shuffle.",
    );
    let found = |c: &Card| match &profile(c, "GU").lands[0].kind {
        LandKind::Fetch { finds, tapped } => (*finds, *tapped),
        other => panic!("{} should fetch, is {other:?}", c.name),
    };
    let (misty_finds, misty_tapped) = found(&misty);
    assert_eq!(misty_finds.types, [BasicLandType::Forest, BasicLandType::Island].into_iter().collect());
    assert!(!misty_finds.basic && !misty_tapped);
    let (wilds_finds, wilds_tapped) = found(&wilds);
    assert!(wilds_finds.basic && wilds_finds.types.is_empty() && wilds_tapped);
}

#[test]
fn wastes_is_basic_but_no_fetch_for_a_land_type_finds_it() {
    let wastes = card("Wastes", "Basic Land", "", "C", "{T}: Add {C}.");
    let pool = card("Breeding Pool", "Land — Forest Island", "", "GU", "{T}: Add {G} or {U}.");
    let (wastes_p, pool_p) = (profile(&wastes, "G"), profile(&pool, "G"));
    let basic = LandFilter { basic: true, types: Default::default() };
    let forest = LandFilter { basic: false, types: BasicLandType::Forest.into() };
    assert!(basic.allows(land(&wastes_p)) && !forest.allows(land(&wastes_p)));
    assert!(forest.allows(land(&pool_p)) && !basic.allows(land(&pool_p)), "a Forest, but not basic");
}

#[test]
fn ramp_spells_read_how_many_lands_go_where() {
    let effect = |c: &Card| profile(c, "G").spell.unwrap().effect;
    let cultivate = card(
        "Cultivate",
        "Sorcery",
        "{2}{G}",
        "",
        "Search your library for up to two basic land cards, reveal those cards, put one onto the battlefield tapped and the other into your hand, then shuffle.",
    );
    assert!(matches!(effect(&cultivate), Effect::LandRamp { to_battlefield: 1, to_hand: 1, tapped: true, .. }));
    let bauble = card(
        "Wayfarer's Bauble",
        "Artifact",
        "{1}",
        "",
        "{2}, {T}, Sacrifice Wayfarer's Bauble: Search your library for a basic land card, put that card onto the battlefield tapped, then shuffle.",
    );
    assert!(matches!(effect(&bauble), Effect::LandRamp { to_battlefield: 1, to_hand: 0, activation: 2, .. }));
    let path = card(
        "Path to Exile",
        "Instant",
        "{W}",
        "",
        "Exile target creature. Its controller may search their library for a basic land card, put that card onto the battlefield tapped, then shuffle.",
    );
    assert_eq!(effect(&path), Effect::Other, "ramps the opponent, not us");
}

#[test]
fn draw_counts_what_a_card_nets() {
    let tagged = |c: Card| Card { tagged: true, roles: Role::Draw.into(), ..c };
    let effect = |c: &Card| profile(c, "UB").spell.unwrap().effect;
    let whisper = tagged(card("Night's Whisper", "Sorcery", "{1}{B}", "", "You draw two cards and you lose 2 life."));
    assert_eq!(effect(&whisper), Effect::Draw { cards: 2 });
    let study = tagged(card("Careful Study", "Sorcery", "{U}", "", "Draw two cards, then discard two cards."));
    assert_eq!(effect(&study), Effect::Draw { cards: 0 });
    let jace = tagged(card(
        "Jace Beleren",
        "Legendary Planeswalker — Jace",
        "{1}{U}{U}",
        "",
        "+2: Each player draws a card.\n−1: Target player draws a card.",
    ));
    assert_eq!(effect(&jace), Effect::Engine);
}

#[test]
fn a_draw_in_the_sentence_after_its_trigger_repeats() {
    let tagged = |c: Card| Card { tagged: true, roles: Role::Draw.into(), ..c };
    let effect = |c: &Card| profile(c, "W").spell.unwrap().effect;
    let mentor = tagged(card(
        "Mentor of the Meek",
        "Creature — Human Soldier",
        "{2}{W}",
        "",
        "Whenever another creature you control with power 2 or less enters, you may pay {1}. If you do, draw a card.",
    ));
    assert_eq!(effect(&mentor), Effect::Engine);
    let dawn = tagged(card(
        "Dawn of Hope",
        "Enchantment",
        "{1}{W}",
        "",
        "Whenever you gain life, you may pay {2}. If you do, draw a card.\n{3}{W}: Create a 1/1 white Soldier creature token with lifelink.",
    ));
    assert_eq!(effect(&dawn), Effect::Engine);
}

#[test]
fn planeswalkers_draw_every_turn_only_from_a_plus_or_zero_ability() {
    let tagged = |c: Card| Card { tagged: true, roles: Role::Draw.into(), ..c };
    let effect = |c: &Card| profile(c, "B").spell.unwrap().effect;
    let lolth = tagged(card(
        "Lolth, Spider Queen",
        "Legendary Planeswalker — Lolth",
        "{3}{B}{B}",
        "",
        "Whenever a creature you control dies, put a loyalty counter on Lolth.\n0: You draw a card and you lose 1 life.\n−3: Create two 2/1 black Spider creature tokens with reach and menace.",
    ));
    assert_eq!(effect(&lolth), Effect::Engine);
    let ugin = tagged(card(
        "Ugin, the Spirit Dragon",
        "Legendary Planeswalker — Ugin",
        "{8}",
        "",
        "+2: Ugin deals 3 damage to any target.\n−X: Exile each permanent with mana value X or less that's one or more colors.\n−10: You gain 7 life, draw seven cards, then put up to seven permanent cards from your hand onto the battlefield.",
    ));
    assert_eq!(effect(&ugin), Effect::Other, "the −10 is a long way off, and draws once");
}

#[test]
fn draws_that_need_a_death_an_attack_or_a_sacrifice_dont_repeat_in_a_goldfish() {
    let tagged = |c: Card| Card { tagged: true, roles: Role::Draw.into(), ..c };
    let effect = |c: &Card| profile(c, "UB").spell.unwrap().effect;
    let lantern = tagged(card(
        "Soul-Guide Lantern",
        "Artifact",
        "{1}",
        "",
        "When this artifact enters, exile target card from a graveyard.\n{T}, Sacrifice this artifact: Exile each opponent's graveyard.\n{1}, {T}, Sacrifice this artifact: Draw a card.",
    ));
    assert_eq!(effect(&lantern), Effect::Draw { cards: 1 }, "sacrificed for a card, once");
    let bauble = tagged(card(
        "Mishra's Bauble",
        "Artifact",
        "{0}",
        "",
        "{T}, Sacrifice this artifact: Look at the top card of target player's library. Draw a card at the beginning of the next turn's upkeep.",
    ));
    assert_eq!(effect(&bauble), Effect::Draw { cards: 1 });
    let post = tagged(card("Trading Post", "Artifact", "{4}", "", "{1}, {T}, Sacrifice an artifact: Draw a card."));
    assert_eq!(effect(&post), Effect::Other, "needs another artifact to sacrifice");
    let clamp = tagged(card(
        "Skullclamp",
        "Artifact — Equipment",
        "{1}",
        "",
        "Equipped creature gets +1/-1.\nWhenever equipped creature dies, draw two cards.\nEquip {1}",
    ));
    assert_eq!(effect(&clamp), Effect::Other);
    let bonny = tagged(card(
        "Bonny Pall, Clearcutter",
        "Legendary Creature — Giant Scout",
        "{3}{G}{U}",
        "",
        "Whenever you attack, draw a card, then you may put a land card from your hand or graveyard onto the battlefield.",
    ));
    assert_eq!(effect(&bonny), Effect::Other, "a goldfish doesn't attack");
    let greed = tagged(card("Greed", "Enchantment", "{3}{B}", "", "{B}, Pay 2 life: Draw a card."));
    assert_eq!(effect(&greed), Effect::Engine, "can be used every turn");
    let monitor = tagged(card(
        "Thought Monitor",
        "Artifact Creature — Construct",
        "{6}{U}",
        "",
        "Affinity for artifacts\nFlying\nWhen this creature enters, draw two cards.",
    ));
    assert_eq!(effect(&monitor), Effect::Draw { cards: 2 });
}

#[test]
fn cards_into_hand_count_however_the_text_says_it() {
    let tagged = |c: Card| Card { tagged: true, roles: Role::Draw.into(), ..c };
    let effect = |c: &Card| profile(c, "UB").spell.unwrap().effect;
    let stock = tagged(card(
        "Stock Up",
        "Sorcery",
        "{2}{U}",
        "",
        "Look at the top five cards of your library. Put two of them into your hand and the rest on the bottom of your library in any order.",
    ));
    assert_eq!(effect(&stock), Effect::Draw { cards: 2 });
    let peer = tagged(card(
        "Peer into the Abyss",
        "Sorcery",
        "{4}{B}{B}{B}",
        "",
        "Target player draws cards equal to half the number of cards in their library and loses half their life. Round up each time.",
    ));
    assert_eq!(effect(&peer), Effect::Draw { cards: 3 });
    let reclamation = tagged(card(
        "Phyrexian Reclamation",
        "Enchantment",
        "{B}",
        "",
        "{1}{B}, Pay 2 life: Return target creature card from your graveyard to your hand.",
    ));
    assert_eq!(effect(&reclamation), Effect::Other, "nothing in a goldfish's graveyard");
}

#[test]
fn ramp_that_waits_for_an_attack_isnt_ramp() {
    let bitterthorn = card(
        "Bitterthorn, Nissa's Animus",
        "Legendary Artifact — Equipment",
        "{3}",
        "",
        "Living weapon\nEquipped creature gets +1/+1.\nWhenever equipped creature attacks, you may search your library for a basic land card, put it onto the battlefield tapped, then shuffle.\nEquip {4}",
    );
    assert_eq!(profile(&bitterthorn, "G").spell.unwrap().effect, Effect::Other);
}

#[test]
fn an_aura_makes_mana_for_us_only_on_our_land() {
    let moon = card(
        "Imprisoned in the Moon",
        "Enchantment — Aura",
        "{2}{U}",
        "C",
        "Enchant creature, land, or planeswalker\nEnchanted permanent is a colorless land with \"{T}: Add {C}\" and loses all other card types and abilities.",
    );
    assert_eq!(
        profile(&moon, "U").spell.unwrap().effect,
        Effect::Other,
        "it's the opponent's commander that taps for {{C}}"
    );
    let growth = card(
        "Wild Growth",
        "Enchantment — Aura",
        "{G}",
        "G",
        "Enchant land\nWhenever enchanted land is tapped for mana, its controller adds an additional {G}.",
    );
    assert_eq!(rock_units(&profile(&growth, "G")), ["G"]);
}

#[test]
fn lands_given_up_for_ramp_come_off_it() {
    let effect = |c: &Card| profile(c, "G").spell.unwrap().effect;
    let rotation = card(
        "Crop Rotation",
        "Instant",
        "{G}",
        "",
        "As an additional cost to cast this spell, sacrifice a land.\nSearch your library for a land card, put that card onto the battlefield, then shuffle.",
    );
    assert_eq!(effect(&rotation), Effect::Other, "a land for a land");
    let springbloom = card(
        "Springbloom Druid",
        "Creature — Elf Druid",
        "{2}{G}",
        "",
        "When this creature enters, you may sacrifice a land. If you do, search your library for up to two basic land cards, put them onto the battlefield tapped, then shuffle.",
    );
    assert!(matches!(effect(&springbloom), Effect::LandRamp { to_battlefield: 2, sacrifice: 1, .. }));
    let lotus = card(
        "Lotus Field",
        "Land",
        "",
        "WUBRG",
        "Hexproof\nThis land enters tapped.\nWhen this land enters, sacrifice two lands.\n{T}: Add three mana of any one color.",
    );
    assert_eq!(land(&profile(&lotus, "G")).sacrifices, 2);
    // Played out, Lotus Field is a land for two: no more mana than the lands it replaces.
    let cmd = [commander("Big", "{20}", "G")];
    let with = [(lotus, 8), (basic("Forest", "G"), 32), (filler("{9}"), 59)];
    let without = [(basic("Forest", "G"), 40), (filler("{9}"), 59)];
    let (a, b) = (run(&deck(&cmd, &with), 1000, 1), run(&deck(&cmd, &without), 1000, 1));
    assert!(a.capacity[5] < b.capacity[5] + 0.6, "{} vs {}", a.capacity[5], b.capacity[5]);
    // The lands it sacrifices are tapped for mana first, and it waits for two to give up, so one
    // in place of a Forest costs next to nothing.
    let cmd = [commander("Four", "{3}{G}", "G")];
    let lotus_one = card(
        "Lotus Field",
        "Land",
        "",
        "WUBRG",
        "Hexproof\nThis land enters tapped.\nWhen this land enters, sacrifice two lands.\n{T}: Add three mana of any one color.",
    );
    let one = [(lotus_one, 1), (basic("Forest", "G"), 39), (filler("{9}"), 59)];
    let (a, b) = (run(&deck(&cmd, &one), 4000, 1), run(&deck(&cmd, &without), 4000, 1));
    let (a, b) = (a.commander_by_t4.unwrap(), b.commander_by_t4.unwrap());
    assert!(a > b - 0.02, "commander by T4 with a Lotus Field {a}, without {b}");
}

#[test]
fn draws_that_wait_for_opponents_treasure_or_combat_dont_repeat() {
    let tagged = |c: Card| Card { tagged: true, roles: Role::Draw.into(), ..c };
    let effect = |c: &Card| profile(c, "UBR").spell.unwrap().effect;
    let remora = tagged(card(
        "Mystic Remora",
        "Enchantment",
        "{U}",
        "",
        "Cumulative upkeep {1}\nWhenever an opponent casts a noncreature spell, you may draw a card unless that player pays {4}.",
    ));
    assert_eq!(effect(&remora), Effect::Other);
    let smaug = tagged(card(
        "Smaug, Wicked Worm",
        "Legendary Creature — Dragon",
        "{3}{B}{R}",
        "",
        "Flying\nWhenever you cast a spell, if mana from a Treasure was spent to cast it, you draw a card.",
    ));
    assert_eq!(effect(&smaug), Effect::Other);
    let spirit = tagged(card(
        "Ascendant Spirit",
        "Snow Creature — Spirit",
        "{U}",
        "",
        "{S}{S}{S}{S}: If this creature is an Angel, it becomes a 4/4 with \"Whenever this creature deals combat damage to a player, draw a card.\"",
    ));
    assert_eq!(effect(&spirit), Effect::Other);
    let loran = tagged(card(
        "Loran of the Third Path",
        "Legendary Creature — Human Artificer",
        "{2}{W}",
        "",
        "When Loran enters, destroy up to one target artifact or enchantment.\n{T}: You and target opponent each draw a card.",
    ));
    assert_eq!(effect(&loran), Effect::Engine, "draws for us too, every turn");
}

#[test]
fn ramp_and_draw_it_cant_play_are_marked_as_not_played() {
    let tagged = |c: Card, r: Role| Card { tagged: true, roles: r.into(), ..c };
    let ghast = tagged(
        card(
            "Crypt Ghast",
            "Creature — Spirit",
            "{3}{B}",
            "B",
            "Whenever you tap a Swamp for mana, add an additional {B}.",
        ),
        Role::Ramp,
    );
    assert_eq!(profile(&ghast, "B").unplayed, Some(Role::Ramp));
    let sol = tagged(card("Sol Ring", "Artifact", "{1}", "C", "{T}: Add {C}{C}."), Role::Ramp);
    assert_eq!(profile(&sol, "B").unplayed, None, "a rock is played");
    let clamp = tagged(
        card(
            "Skullclamp",
            "Artifact — Equipment",
            "{1}",
            "",
            "Whenever equipped creature dies, draw two cards.\nEquip {1}",
        ),
        Role::Draw,
    );
    assert_eq!(profile(&clamp, "B").unplayed, Some(Role::Draw));
    let cmd = [commander("Big", "{20}", "B")];
    let cards = [(ghast, 1), (clamp, 1), (sol, 1), (basic("Swamp", "B"), 40), (filler("{9}"), 56)];
    let r = run(&deck(&cmd, &cards), 10, 1);
    assert_eq!(r.unplayed, [("Crypt Ghast".to_string(), Role::Ramp), ("Skullclamp".to_string(), Role::Draw)]);
}

// --- read from Forge's scripts --------------------------------------------------------------------
//
// The scripts below are written for these tests in the format of Forge's card scripts (see
// `edh_data::forge`), with only the lines the reader looks at.

/// `c` read with this script (`Key:value` lines, faces split by `ALTERNATE`).
fn scripted(c: &Card, script: &str, identity: &str) -> Profile {
    let script = edh_data::forge::Script::parse(script).expect("a script");
    Profile::of(c, Some(&script), ColorSet::from_letters(identity).unwrap())
}

fn effect(p: &Profile) -> &Effect {
    &p.spell.as_ref().expect("a spell").effect
}

#[test]
fn a_scripted_land_enters_tapped_as_its_replacement_says() {
    let land_with = |tap: &str| {
        let c = card("Some Land", "Land", "", "UB", "");
        let script = format!(
            "Name:Some Land\nTypes:Land\nR:Event$ Moved | ValidCard$ Card.Self | Destination$ Battlefield | ReplaceWith$ LandTapped\n\
             SVar:LandTapped:DB$ Tap | Defined$ Self | ETB$ True{tap}\nA:AB$ Mana | Cost$ T | Produced$ Combo U B\n"
        );
        land(&scripted(&c, &script, "UB")).enters.clone()
    };
    let check = " | ConditionPresent$ Island.YouCtrl,Swamp.YouCtrl | ConditionCompare$ EQ0";
    let types = [BasicLandType::Island, BasicLandType::Swamp].into_iter().collect();
    assert_eq!(land_with(check), Enters::UnlessControl(types), "a check land");
    let fast = " | ConditionPresent$ Land.YouCtrl | ConditionCompare$ GT2";
    assert_eq!(land_with(fast), Enters::UnlessLandsAtMost(2), "a fast land: tapped with three other lands");
    let slow = " | ConditionPresent$ Land.YouCtrl | ConditionCompare$ LT2";
    assert_eq!(land_with(slow), Enters::UnlessLandsAtLeast(2), "a slow land: tapped with fewer than two");
    let battle = " | ConditionPresent$ Land.Basic+YouCtrl | ConditionCompare$ LE1";
    assert_eq!(land_with(battle), Enters::UnlessBasics(2), "a battle land: tapped with one basic or none");
    assert_eq!(land_with(" | UnlessCost$ PayLife<2> | UnlessPayer$ You"), Enters::PayLife, "a shock land");
    let snarl = " | UnlessCost$ Reveal<1/Island;Swamp/Island or Swamp> | UnlessPayer$ You";
    assert_eq!(land_with(snarl), Enters::UnlessReveal(vec!["island".into(), "swamp".into()]), "a snarl");
    let opponents = " | ConditionCheckSVar$ Foes | ConditionSVarCompare$ LT2\nSVar:Foes:PlayerCountOpponents$Amount";
    assert_eq!(land_with(opponents), Enters::Untapped, "untapped with two or more opponents: a pod has three");
    assert_eq!(land_with(""), Enters::Tapped);
}

#[test]
fn a_scripted_land_with_basic_land_types_makes_their_colours_besides_its_abilities() {
    // A Forest with "{T}: Add {W} or {B}" (Murmuring Bosk): its Forest type taps for {G} too.
    let c = card("Bosk", "Land — Forest Treefolk", "", "WBG", "");
    let script = "Name:Bosk\nTypes:Land Forest Treefolk\nA:AB$ Mana | Cost$ T | Produced$ Combo W B\n";
    let p = scripted(&c, script, "WBG");
    assert_eq!(land(&p).units.iter().map(|u| u.to_string()).collect::<Vec<_>>(), ["WBG"]);
    assert_eq!(p.fidelity, Fidelity::Exact);
}

#[test]
fn mana_for_sacrificing_a_source_isnt_mana_every_turn() {
    // Crystal Vein: {T}: Add {C}; {T}, sacrifice it: Add {C}{C}.
    let c = card("Vein", "Land", "", "C", "");
    let script = "Name:Vein\nTypes:Land\nA:AB$ Mana | Cost$ T | Produced$ C\n\
                  A:AB$ Mana | Cost$ T Sac<1/CARDNAME> | Produced$ C | Amount$ 2\n";
    let p = scripted(&c, script, "G");
    assert_eq!(land(&p).units.len(), 1, "one {{C}} a turn, not the two of sacrificing it");
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)), "the sacrifice is left out: {:?}", p.fidelity);
    // A dork that sacrifices a land for {R}{R} each time is no rock.
    let c = card("Razer", "Creature — Insect", "{2}{R}", "R", "");
    let script = "Name:Razer\nTypes:Creature Insect\nA:AB$ Mana | Cost$ T Sac<1/Land> | Produced$ R R\n";
    assert_eq!(effect(&scripted(&c, script, "R")), &Effect::Other);
}

#[test]
fn a_scripted_filter_turns_other_mana_into_its_colours() {
    // A Signet: {1}, {T}: Add {W}{U}, one more than it takes in.
    let c = card("Signet", "Artifact", "{2}", "WU", "");
    let script = "Name:Signet\nTypes:Artifact\nA:AB$ Mana | Cost$ 1 T | Produced$ W U\n";
    assert_eq!(rock_units(&scripted(&c, script, "WU")), ["WU"]);
}

#[test]
fn a_scripted_mana_ability_waiting_for_lands_waits() {
    let c = card("Temple", "Land", "", "C", "");
    let script = "Name:Temple\nTypes:Land\nA:AB$ Mana | Cost$ T | Produced$ C | Amount$ 2 | IsPresent$ Land.YouCtrl | PresentCompare$ GE5\n";
    let l = land(&scripted(&c, script, "G")).clone();
    assert!(l.units.is_empty(), "nothing before five lands: {:?}", l.units);
    assert_eq!(l.with_lands.map(|(n, u)| (n, u.len())), Some((5, 2)));
}

#[test]
fn mana_that_waits_for_a_creature_counts_as_one_of_its_colours() {
    // {T}: Add {G}; {T}: Add {G}{G}{G}{G}, only with a creature of power 4 or greater.
    let c = card("Fanatic", "Creature — Snake Druid", "{1}{G}", "G", "");
    let script = "Name:Fanatic\nTypes:Creature Snake Druid\nA:AB$ Mana | Cost$ T | Produced$ G\n\
                  A:AB$ Mana | Cost$ T | Produced$ G | Amount$ 4 | IsPresent$ Creature.YouCtrl+powerGE4\n";
    assert_eq!(rock_units(&scripted(&c, script, "G")), ["G"], "one {{G}}, not four");
}

#[test]
fn a_scripted_rock_that_doesnt_untap_is_used_once() {
    let c = card("Monolith", "Artifact", "{2}", "C", "");
    let script = "Name:Monolith\nTypes:Artifact\nR:Event$ Untap | ValidCard$ Card.Self | ValidStepTurnToController$ You | Layer$ CantHappen\n\
                  A:AB$ Mana | Cost$ T | Produced$ C | Amount$ 3\n";
    assert!(matches!(effect(&scripted(&c, script, "G")), Effect::Rock { once: true, units, .. } if units.len() == 3));
}

#[test]
fn a_scripted_dork_with_haste_taps_the_turn_it_arrives() {
    let script = |kw: &str| format!("Name:Dork\nTypes:Creature Elf\n{kw}A:AB$ Mana | Cost$ T | Produced$ G\n");
    let c = card("Dork", "Creature — Elf", "{G}", "G", "");
    assert!(matches!(effect(&scripted(&c, &script(""), "G")), Effect::Rock { next_turn: true, .. }));
    assert!(matches!(effect(&scripted(&c, &script("K:Haste\n"), "G")), Effect::Rock { next_turn: false, .. }));
}

#[test]
fn a_scripted_aura_adds_mana_to_the_land_it_enchants() {
    let c = card("Growth", "Enchantment — Aura", "{G}", "G", "");
    let script = "Name:Growth\nTypes:Enchantment Aura\nK:Enchant:Land\n\
                  T:Mode$ TapsForMana | ValidCard$ Card.AttachedBy | Execute$ TrigMana | Static$ True\n\
                  SVar:TrigMana:DB$ Mana | Produced$ G G | Defined$ TriggeredCardController\n";
    assert_eq!(rock_units(&scripted(&c, script, "G")), ["G", "G"]);
}

#[test]
fn mana_a_creature_gives_its_kind_counts_for_itself() {
    // "Creatures you control have {T}: Add one mana of any color": it's a creature too.
    let c = card("Gods", "Legendary Creature — God", "{2}{G}{U}", "", "");
    let script = "Name:Gods\nTypes:Legendary Creature God\n\
                  S:Mode$ Continuous | Affected$ Creature.YouCtrl | AddAbility$ Mana\n\
                  SVar:Mana:AB$ Mana | Cost$ T | Produced$ Any\n";
    let p = scripted(&c, script, "GU");
    assert_eq!(rock_units(&p), ["UG"]);
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)), "other creatures' mana is left out: {:?}", p.fidelity);
}

#[test]
fn a_colour_chosen_as_it_enters_counts_as_any_of_the_decks() {
    // A Thriving land: {T}: Add {G} or one mana of the chosen color.
    let c = card("Thriving", "Land", "", "G", "");
    let script = "Name:Thriving\nTypes:Land\nK:ETBReplacement:Other:ChooseColor\nSVar:ChooseColor:DB$ ChooseColor | Defined$ You\n\
                  A:AB$ Mana | Cost$ T | Produced$ Combo G Chosen\n";
    let p = scripted(&c, script, "BG");
    assert_eq!(land(&p).units.iter().map(|u| u.to_string()).collect::<Vec<_>>(), ["BG"]);
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)));
}

#[test]
fn a_scripted_search_puts_one_land_out_and_one_in_hand_like_cultivate() {
    let c = card("Cultivation", "Sorcery", "{2}{G}", "", "");
    let script = "Name:Cultivation\nTypes:Sorcery\n\
        A:SP$ ChangeZone | Origin$ Library | Destination$ Library | ChangeType$ Land.Basic | ChangeNum$ 2 | RememberChanged$ True | SubAbility$ ToPlay\n\
        SVar:ToPlay:DB$ ChangeZone | Origin$ Library | Destination$ Battlefield | ChangeType$ Land.IsRemembered | ChangeNum$ 1 | Tapped$ True | SubAbility$ ToHand\n\
        SVar:ToHand:DB$ ChangeZone | Origin$ Library | Destination$ Hand | ChangeType$ Land.IsRemembered\n";
    let wanted = Effect::LandRamp {
        finds: LandFilter { basic: true, types: Default::default() },
        to_battlefield: 1,
        to_hand: 1,
        tapped: true,
        activation: 0,
        sacrifice: 0,
    };
    assert_eq!(effect(&scripted(&c, script, "G")), &wanted);
}

#[test]
fn a_land_someone_else_searches_for_isnt_ramp() {
    // Exile target creature; its controller may search for a basic land.
    let c = card("Exile", "Instant", "{W}", "", "");
    let script = "Name:Exile\nTypes:Instant\n\
        A:SP$ ChangeZone | Origin$ Battlefield | Destination$ Exile | ValidTgts$ Creature | SubAbility$ Theirs\n\
        SVar:Theirs:DB$ ChangeZone | Origin$ Library | Destination$ Battlefield | Tapped$ True | ChangeType$ Land.Basic | DefinedPlayer$ TargetedController\n";
    assert_eq!(effect(&scripted(&c, script, "W")), &Effect::Other);
}

#[test]
fn a_land_sacrificed_to_search_comes_off_what_it_finds() {
    // As it enters, sacrifice a land to put two basics onto the battlefield.
    let c = card("Druid", "Creature — Elf Druid", "{2}{G}", "", "");
    let script = "Name:Druid\nTypes:Creature Elf Druid\n\
        T:Mode$ ChangesZone | Origin$ Any | Destination$ Battlefield | ValidCard$ Card.Self | Execute$ Ramp\n\
        SVar:Ramp:AB$ ChangeZone | Cost$ Sac<1/Land> | Origin$ Library | Destination$ Battlefield | Tapped$ True | ChangeType$ Land.Basic | ChangeNum$ 2\n";
    assert!(matches!(effect(&scripted(&c, script, "G")), Effect::LandRamp { to_battlefield: 2, sacrifice: 1, .. }));
}

#[test]
fn a_land_that_sacrifices_itself_as_it_enters_for_a_basic_is_a_fetch() {
    // The New Capenna lands: "When this land enters, sacrifice it. When you do, search your
    // library for a basic Swamp, Mountain, or Forest card, put it onto the battlefield tapped": the
    // search is a trigger of its own, run only if the sacrifice was.
    let c = card("Overlook", "Land", "", "", "");
    let script = "Name:Overlook\nTypes:Land\n\
        T:Mode$ ChangesZone | Origin$ Any | Destination$ Battlefield | ValidCard$ Card.Self | Execute$ DBSacrifice\n\
        SVar:DBSacrifice:DB$ Sacrifice | RememberSacrificed$ True | SubAbility$ DBImmediateTrigger\n\
        SVar:DBImmediateTrigger:DB$ ImmediateTrigger | ConditionDefined$ Remembered | ConditionPresent$ Card | Execute$ DBChangeZone | SubAbility$ DBCleanup\n\
        SVar:DBChangeZone:DB$ ChangeZone | Origin$ Library | Destination$ Battlefield | Tapped$ True | ChangeType$ Swamp.Basic,Mountain.Basic,Forest.Basic | SubAbility$ DBGainLife\n\
        SVar:DBGainLife:DB$ GainLife | Defined$ You | LifeAmount$ 1\nSVar:DBCleanup:DB$ Cleanup | ClearRemembered$ True\n";
    let p = scripted(&c, script, "BRG");
    let types = [BasicLandType::Swamp, BasicLandType::Mountain, BasicLandType::Forest].into_iter().collect();
    match &p.lands[0].kind {
        LandKind::Fetch { finds, tapped } => {
            assert_eq!((finds, *tapped), (&LandFilter { basic: true, types }, true), "a basic of its types, tapped")
        }
        other => panic!("a fetch, not a land that makes nothing: {other:?}"),
    }
    assert_eq!(p.fidelity, Fidelity::Exact);
}

#[test]
fn a_land_filter_taking_in_more_than_one_mana_waits_for_the_lands_to_pay_for_it() {
    // Castle Garenbrig: {T}: Add {G}; {2}{G}{G}, {T}: Add six {G} — two more than tapping it and
    // four other lands would make, but only once there are four other lands.
    let c = card("Castle", "Land", "", "G", "");
    let script = "Name:Castle\nTypes:Land\nA:AB$ Mana | Cost$ T | Produced$ G\n\
                  A:AB$ Mana | Cost$ 2 G G T | Produced$ G | Amount$ 6\n";
    let l = land(&scripted(&c, script, "G")).clone();
    assert_eq!(l.units.len(), 1, "one {{G}} with fewer than five lands");
    assert_eq!(l.with_lands.map(|(n, u)| (n, u.len())), Some((5, 2)), "two from five lands on");
}

#[test]
fn a_dork_making_the_greatest_power_among_your_creatures_makes_at_least_its_own() {
    // Selvala, Heart of the Wilds: {G}, {T}: Add X mana in any combination of colors, X the greatest
    // power among creatures you control; she has power 2, so one more than she takes in.
    let c = card("Selvala", "Legendary Creature — Elf Scout", "{1}{G}{G}", "WUBRG", "");
    let script = "Name:Selvala\nTypes:Legendary Creature Elf Scout\nPT:2/3\n\
                  A:AB$ Mana | Cost$ G T | Produced$ Combo Any | Amount$ X\n\
                  SVar:X:Count$Valid Creature.YouCtrl$GreatestCardPower\n";
    let p = scripted(&c, script, "G");
    assert_eq!(rock_units(&p), ["G"], "she nets one mana, not none");
    assert!(p.undercounted, "with bigger creatures she makes more");
    // The greatest power among creatures that entered this turn may be none at all.
    let script = script.replace("Creature.YouCtrl$", "Creature.YouCtrl+ThisTurnEntered$");
    assert_eq!(effect(&scripted(&c, &script, "G")), &Effect::Other, "{{G}} in, one counted out: nothing gained");
}

#[test]
fn a_search_for_a_kind_of_land_it_cant_tell_apart_says_it_finds_more() {
    // Into the North: a snow land, read as any land.
    let c = card("North", "Sorcery", "{1}{G}", "", "");
    let script = "Name:North\nTypes:Sorcery\n\
        A:SP$ ChangeZone | Origin$ Library | Destination$ Battlefield | ChangeType$ Land.Snow | Tapped$ True | ChangeNum$ 1\n";
    let p = scripted(&c, script, "G");
    assert!(matches!(effect(&p), Effect::LandRamp { finds: LandFilter { basic: false, .. }, .. }));
    match p.fidelity {
        Fidelity::Approximate(why) => assert!(why.contains("counted as one for any land"), "{why}"),
        other => panic!("a snow search isn't exact: {other:?}"),
    }
}

#[test]
fn a_mana_ability_that_makes_a_choice_first_is_read_and_one_that_needs_something_isnt() {
    // Nykthos: {2}, {T}: Choose a color, add mana of it equal to your devotion to it.
    let c = card("Shrine", "Legendary Land", "", "WUBRGC", "");
    let script = "Name:Shrine\nTypes:Legendary Land\nA:AB$ Mana | Cost$ T | Produced$ C\n\
                  A:AB$ ChooseColor | Cost$ 2 T | SubAbility$ DBMana\n\
                  SVar:DBMana:DB$ Mana | Produced$ Chosen | Amount$ X\nSVar:X:Count$Devotion.Chosen\n";
    let p = scripted(&c, script, "G");
    assert!(p.undercounted, "its devotion mana is read, as more than it's counted");
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)), "{:?}", p.fidelity);
    // Deathrite Shaman: {T}: Exile target land card from a graveyard, then add one mana of any colour.
    let c = card("Shaman", "Creature — Elf Shaman", "{B/G}", "WUBRG", "");
    let script = "Name:Shaman\nTypes:Creature Elf Shaman\n\
                  A:AB$ ChangeZone | Cost$ T | Origin$ Graveyard | Destination$ Exile | ValidTgts$ Land | SubAbility$ DBMana\n\
                  SVar:DBMana:DB$ Mana | Produced$ Any\n";
    assert_eq!(effect(&scripted(&c, script, "BG")), &Effect::Other, "no land in a graveyard, no mana");
}

#[test]
fn a_card_that_makes_your_other_spells_cheaper_is_ramp_the_goldfish_doesnt_play() {
    // Goreclaw: creature spells you cast with power 4 or greater cost {2} less. Tagger doesn't call
    // that ramp.
    let c = card("Bear Lord", "Legendary Creature — Bear", "{3}{G}", "", "");
    let script = "Name:Bear Lord\nTypes:Legendary Creature Bear\n\
                  S:Mode$ ReduceCost | ValidCard$ Creature.powerGE4 | Type$ Spell | Activator$ You | Amount$ 2\n";
    let p = scripted(&c, script, "G");
    assert_eq!(p.unplayed, Some(Role::Ramp));
    assert_eq!(p.left_out, Some(crate::profile::LeftOut::Engine), "cheaper spells every turn");
    // Making itself cheaper isn't ramp: it's just a cheaper spell.
    let c = card("Monitor", "Artifact Creature — Construct", "{6}{U}", "", "");
    let script = "Name:Monitor\nTypes:Artifact Creature Construct\nK:Affinity:Artifact\n\
                  S:Mode$ ReduceCost | ValidCard$ Card.Self | Type$ Spell | Amount$ X | EffectZone$ All\n";
    assert_eq!(scripted(&c, script, "U").unplayed, None);
}

#[test]
fn a_commander_that_makes_spells_cheaper_makes_the_numbers_rough() {
    // Animar: creature spells you cast cost {1} less for each +1/+1 counter on it.
    let cmd = Card {
        identity: ColorSet::from_letters("G").unwrap(),
        ..card("Animar", "Legendary Creature — Elemental", "{G}", "", "")
    };
    let reducer = edh_data::forge::Script::parse(
        "Name:Animar\nTypes:Legendary Creature Elemental\n\
         S:Mode$ ReduceCost | ValidCard$ Creature | Type$ Spell | Activator$ You | Amount$ X\nSVar:X:Count$CardCounters.P1P1\n",
    )
    .unwrap();
    let forest = basic("Forest", "G");
    let forest_script = edh_data::forge::Script::parse("Name:Forest\nTypes:Basic Land Forest\n").unwrap();
    let bear = filler("{2}");
    let entries = [
        DeckEntry { card: &cmd, qty: 1, commander: true, script: Some(&reducer) },
        DeckEntry { card: &forest, qty: 40, commander: false, script: Some(&forest_script) },
        DeckEntry { card: &bear, qty: 59, commander: false, script: None },
    ];
    let r = run(&entries, 200, 1);
    assert_eq!(r.doubts.commander_ramp, ["Animar"], "its cost reduction isn't played");
    assert_eq!(r.doubts.confidence, crate::Confidence::Rough);
}

#[test]
fn a_script_it_cant_read_leaves_the_card_to_its_oracle_text_and_says_so() {
    let c = card("Odd Rock", "Artifact", "{3}", "C", "{T}: Add {C}{C}.");
    let script = "Name:Odd Rock\nTypes:Artifact\nA:AB$ Mana | Cost$ T | Produced$ Special DoubleManaInPool\n";
    let p = scripted(&c, script, "G");
    assert_eq!(rock_units(&p), ["C", "C"], "read from oracle text");
    assert!(matches!(p.fidelity, Fidelity::Unreadable(_)), "{:?}", p.fidelity);
    assert_eq!(profile(&c, "G").fidelity, Fidelity::NoScript, "without a script, it says there's none");
}

// --- whole games ----------------------------------------------------------------------------------

#[test]
fn a_deck_on_its_colours_casts_a_cheap_commander_on_time_nearly_always() {
    let cmd = [commander("Teval", "{1}{U}", "U")];
    let cards = [(basic("Island", "U"), 40), (filler("{6}"), 59)];
    let r = run(&deck(&cmd, &cards), 2000, 1);
    let by_t4 = r.commander_by_t4.unwrap();
    assert!(by_t4 > 0.95, "commander by T4 in {by_t4}");
    assert_eq!(r.colour_delayed, Some(0.0));
    assert!(r.stuck.is_empty(), "nothing needs a colour the deck lacks: {:?}", r.stuck);
}

#[test]
fn a_commander_without_sources_of_its_colour_is_never_cast() {
    let cmd = [commander("Blue One", "{1}{U}", "U")];
    let cards = [(basic("Forest", "G"), 40), (filler("{6}"), 59)];
    let r = run(&deck(&cmd, &cards), 500, 1);
    assert_eq!(r.commander_by_t4, Some(0.0));
    assert!(r.colour_delayed.unwrap() > 0.95, "colours held it up in {:?}", r.colour_delayed);
    assert_eq!(r.stuck.first().map(|(n, _)| n.as_str()), Some("Blue One"), "{:?}", r.stuck);
}

#[test]
fn the_first_partner_counts_as_the_commander_landing() {
    let cmd = [commander("Cheap Partner", "{G}", "G"), commander("Dear Partner", "{9}{G}", "G")];
    let cards = [(basic("Forest", "G"), 40), (filler("{6}"), 59)];
    let r = run(&deck(&cmd, &cards), 500, 1);
    assert!(r.commander_by_t4.unwrap() > 0.95, "{:?}", r.commander_by_t4);
}

#[test]
fn a_spell_with_a_land_back_is_played_as_a_land_when_the_deck_needs_one() {
    let mut mdfc = card(
        "Bala Ged Recovery // Bala Ged Sanctuary",
        "Sorcery // Land",
        "{2}{G} // ",
        "G",
        "Return target card from your graveyard to your hand. // As this land enters, you may pay 3 life. If you don't, it enters tapped.\n{T}: Add {G}.",
    );
    mdfc.layout = Layout::ModalDfc;
    let p = profile(&mdfc, "G");
    assert!(p.lands[0].mdfc_back && p.spell.is_some(), "both a land and a spell: {p:?}");
    let cmd = [commander("Green One", "{3}{G}", "G")];
    let cards = [(mdfc, 40), (filler("{6}"), 59)];
    let r = run(&deck(&cmd, &cards), 500, 1);
    assert!(r.commander_by_t4.unwrap() > 0.8, "with only MDFCs for lands: {:?}", r.commander_by_t4);
}

#[test]
fn rocks_put_a_deck_ahead_of_a_land_a_turn() {
    let cmd = [commander("Teval", "{5}{U}", "U")];
    let sol = card("Sol Ring", "Artifact", "{1}", "C", "{T}: Add {C}{C}.");
    let plain = [(basic("Island", "U"), 38), (filler("{6}"), 61)];
    let rocks = [(basic("Island", "U"), 38), (sol, 10), (filler("{6}"), 51)];
    let (a, b) = (run(&deck(&cmd, &plain), 1000, 1), run(&deck(&cmd, &rocks), 1000, 1));
    assert!(a.ahead_on_t4().abs() < 0.6, "lands alone stay near a land a turn: {}", a.ahead_on_t4());
    assert!(b.ahead_on_t4() > a.ahead_on_t4() + 1.0, "rocks: {} vs {}", b.ahead_on_t4(), a.ahead_on_t4());
}

#[test]
fn a_phyrexian_commander_comes_down_for_life() {
    let cmd = [commander("K'rrik", "{4}{B/P}{B/P}{B/P}", "B")];
    let cards = [(basic("Swamp", "B"), 40), (filler("{6}"), 59)];
    let r = run(&deck(&cmd, &cards), 1000, 1);
    assert!(r.commander_by_t4.unwrap() > 0.8, "four mana and 6 life: {:?}", r.commander_by_t4);
}

#[test]
fn colourless_mana_left_over_isnt_a_colour_problem() {
    // Mono-blue with Sol Rings: blue spells cast with the Islands aren't stuck because the
    // turn ends with only colourless mana left.
    let cmd = [commander("Blue", "{U}", "U")];
    let sol = card("Sol Ring", "Artifact", "{1}", "C", "{T}: Add {C}{C}.");
    let cards = [(basic("Island", "U"), 36), (sol, 15), (card("Counterspell", "Instant", "{U}{U}", "", ""), 48)];
    let r = run(&deck(&cmd, &cards), 1000, 1);
    let counterspell = r.stuck.iter().find(|(n, _)| n == "Counterspell").map_or(0.0, |(_, s)| *s);
    assert!(counterspell < 0.02, "stuck in {counterspell} of games");
}

#[test]
fn the_same_deck_gets_the_same_report() {
    let cmd = [commander("Teval", "{1}{B}{G}{U}", "BGU")];
    let cards = [
        (basic("Island", "U"), 12),
        (basic("Swamp", "B"), 12),
        (basic("Forest", "G"), 12),
        (filler("{2}{B}"), 30),
        (filler("{3}{U}{U}"), 33),
    ];
    let entries = deck(&cmd, &cards);
    let seed = seed_for("FRS3ZQXQufA6UvihCuYMOA");
    assert_eq!(run(&entries, 1000, seed), run(&entries, 1000, seed));
    assert_ne!(run(&entries, 1000, seed), run(&entries, 1000, seed + 1), "another seed plays other games");
}

fn units(u: &[crate::profile::Unit]) -> Vec<String> {
    u.iter().map(|u| u.to_string()).collect()
}

#[test]
fn mana_that_grows_with_the_board_counts_one_more_than_it_takes_in() {
    // Cabal Coffers: {2}, {T}: Add {B} for each Swamp you control. Counted as one, less the two it
    // takes in, it made nothing; it makes one once the lands are there to pay for it.
    let c = card("Coffers", "Land", "", "B", "");
    let script = "Name:Coffers\nTypes:Land\nA:AB$ Mana | Cost$ 2 T | Produced$ B | Amount$ X\n\
                  SVar:X:Count$Valid Swamp.YouCtrl\n";
    let p = scripted(&c, script, "B");
    let l = land(&p);
    assert!(l.units.is_empty(), "nothing before there are lands to pay for it, made {:?}", units(&l.units));
    let (lands, makes) = l.with_lands.clone().expect("mana once enough lands are out");
    assert_eq!((lands, units(&makes)), (3, vec!["B".to_string()]), "one black with three lands");
    assert!(p.undercounted);
}

#[test]
fn a_land_that_takes_a_basic_land_type_as_it_enters_makes_any_of_the_decks_colours() {
    // Multiversal Passage: choose a basic land type as it enters; it's that type.
    let c = card("Passage", "Land", "", "", "");
    let script = "Name:Passage\nTypes:Land\n\
                  R:Event$ Moved | ValidCard$ Card.Self | Destination$ Battlefield | ReplaceWith$ DBChooseBasic | ReplacementResult$ Updated\n\
                  SVar:DBChooseBasic:DB$ ChooseType | ETB$ True | Type$ Basic Land | SubAbility$ DBTap\n\
                  SVar:DBTap:DB$ Tap | ETB$ True | Defined$ Self | UnlessCost$ PayLife<2> | UnlessPayer$ You\n\
                  S:Mode$ Continuous | AffectedDefined$ Self | AddType$ ChosenType | RemoveLandTypes$ True\n";
    let p = scripted(&c, script, "UR");
    assert_eq!(units(&land(&p).units), ["UR"], "one mana of either of the deck's colours");
    assert_eq!(land(&p).enters, Enters::PayLife);
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)), "the type is chosen: {:?}", p.fidelity);
}

#[test]
fn a_land_that_enters_as_a_copy_of_another_makes_any_colour_tapped() {
    // Vesuva copies a land on the battlefield; Echoing Deeps one in a graveyard, which a
    // goldfish's is empty of.
    let copy = |zone: &str| {
        let c = card("Copy Land", "Land", "", "", "");
        let script = format!(
            "Name:Copy Land\nTypes:Land\nK:ETBReplacement:Copy:DBCopy:Optional\n\
             SVar:DBCopy:DB$ Clone | Choices$ Land.Other{zone} | IntoPlayTapped$ True\n"
        );
        scripted(&c, &script, "WB")
    };
    let vesuva = copy("");
    assert_eq!(units(&land(&vesuva).units), ["WB"]);
    assert_eq!(land(&vesuva).enters, Enters::Tapped, "the copy enters tapped");
    assert!(land(&copy(" | ChoiceZone$ Graveyard")).units.is_empty(), "nothing to copy in a graveyard");
}

#[test]
fn mana_of_each_colour_among_permanents_is_at_least_its_own_colours() {
    // Faeburrow Elder ({1}{G}{W}): {T}: Add one mana of each color among permanents you control.
    let c = card("Elder", "Creature — Treefolk Druid", "{1}{G}{W}", "WUBRG", "");
    let script = "Name:Elder\nManaCost:1 G W\nTypes:Creature Treefolk Druid\nPT:0/0\n\
                  A:AB$ Mana | Cost$ T | Produced$ Special EachColorAmong_Valid Permanent.YouCtrl\n";
    let p = scripted(&c, script, "GW");
    assert_eq!(rock_units(&p), ["W", "G"], "a green and a white mana, from itself");
    assert!(p.undercounted, "the deck's other permanents add more");
}

#[test]
fn a_land_that_taps_for_colourless_or_fetches_for_free_plays_as_the_fetch() {
    // Twisted Landscape: {T}: Add {C}; {T}, sacrifice it: a basic Swamp, Mountain or Forest, tapped.
    // Myriad Landscape's search costs {2}: it stays a land for {C}, with the search marked as left out.
    let landscape = |cost: &str| {
        let c = card("Landscape", "Land", "", "C", "");
        let script = format!(
            "Name:Landscape\nTypes:Land\nA:AB$ Mana | Cost$ T | Produced$ C\n\
             A:AB$ ChangeZone | Cost$ {cost}T Sac<1/CARDNAME> | Origin$ Library | Destination$ Battlefield | Tapped$ True | ChangeType$ Swamp.Basic,Mountain.Basic,Forest.Basic\n"
        );
        scripted(&c, &script, "BRG")
    };
    let free = landscape("");
    let LandKind::Fetch { finds, tapped } = &free.lands[0].kind else { panic!("a fetch, is {:?}", free.lands[0].kind) };
    assert!(finds.basic && *tapped);
    assert_eq!(finds.types.len(), 3);
    let paid = landscape("2 ");
    assert_eq!(units(&land(&paid).units), ["C"]);
    assert!(matches!(paid.fidelity, Fidelity::Approximate(_)), "its search is left out: {:?}", paid.fidelity);
}

#[test]
fn mana_given_to_other_lands_is_marked_but_not_counted_as_more_mana() {
    // Chromatic Lantern: lands you control have "{T}: Add one mana of any color"; {T}: Add one mana
    // of any color. The World Tree gives that to its lands only with six of them.
    let c = card("Lantern", "Artifact", "{3}", "WUBRG", "");
    let script = "Name:Lantern\nManaCost:3\nTypes:Artifact\n\
                  S:Mode$ Continuous | Affected$ Land.YouCtrl | AddAbility$ AnyMana\n\
                  SVar:AnyMana:AB$ Mana | Cost$ T | Produced$ Any | Amount$ 1\nA:AB$ Mana | Cost$ T | Produced$ Any\n";
    let p = scripted(&c, script, "WUB");
    assert_eq!(rock_units(&p), ["WUB"]);
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)), "the lands' colours are left out: {:?}", p.fidelity);
    assert!(!p.undercounted, "colours aren't more mana");
    let tree = card("Tree", "Land", "", "G", "");
    let script = "Name:Tree\nTypes:Land\nA:AB$ Mana | Cost$ T | Produced$ G\n\
                  S:Mode$ Continuous | Affected$ Land.YouCtrl | AddAbility$ AnyMana | IsPresent$ Land.YouCtrl | PresentCompare$ GE6\n\
                  SVar:AnyMana:AB$ Mana | Cost$ T | Produced$ Any | Amount$ 1\n";
    assert_eq!(units(&land(&scripted(&tree, script, "WUBG")).units), ["G"], "green until there are six lands");
}

#[test]
fn a_land_creature_enters_as_if_tapped() {
    // Dryad Arbor: a Forest that's a creature can't tap for mana the turn it's played.
    let c = card("Arbor", "Land Creature — Forest Dryad", "", "G", "");
    let script = "Name:Arbor\nTypes:Land Creature Forest Dryad\nPT:1/1\n";
    assert_eq!(land(&scripted(&c, script, "G")).enters, Enters::Tapped);
}

#[test]
fn a_land_using_up_depletion_counters_is_read_like_one_using_charge_counters() {
    // Hickory Woodlot: {T}, remove a depletion counter: Add {G}{G}; two uses, then it's sacrificed.
    let c = card("Woodlot", "Land", "", "G", "");
    let script = "Name:Woodlot\nTypes:Land\nA:AB$ Mana | Cost$ T SubCounter<1/DEPLETION> | Produced$ G | Amount$ 2\n";
    let p = scripted(&c, script, "G");
    assert_eq!(units(&land(&p).units), ["G", "G"]);
    assert!(matches!(p.fidelity, Fidelity::Approximate(_)), "its uses are limited: {:?}", p.fidelity);
}
