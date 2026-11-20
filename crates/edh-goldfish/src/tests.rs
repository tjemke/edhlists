//! What the goldfish must get right, card by card and deck by deck.

use edh_data::cards::Card;
use edh_data::mana::{ColorSet, ManaSet};
use edh_data::tags::Role;
use edh_data::types::{BasicLandType, Layout};

use crate::profile::{Effect, Enters, Land, LandFilter, LandKind, Profile};
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
    Profile::of(c, ColorSet::from_letters(identity).unwrap())
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
        .map(|c| DeckEntry { card: c, qty: 1, commander: true })
        .chain(cards.iter().map(|(c, n)| DeckEntry { card: c, qty: *n, commander: false }))
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
