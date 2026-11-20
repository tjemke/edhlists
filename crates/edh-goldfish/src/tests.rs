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
    assert!(matches!(p.spell.unwrap().effect, Effect::Rock { summoning_sick: true, .. }));
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
