//! The Rust port must agree with the JavaScript transcription of the site (reference/), on the same
//! inputs. Regenerate with the steps in this crate's README.

use std::collections::BTreeMap;

use edhpl::{Combo, DeckCard, bracket, curve, power_level};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    file: String,
    cards: Vec<DeckCard>,
    combos: Vec<Combo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Golden {
    level: f64,
    score: f64,
    impact: f64,
    efficiency: f64,
    avg_cost: f64,
    curve_cmc: usize,
    lands: u32,
    nonlands: u32,
    bracket: u8,
    floor: u8,
    from_level: u8,
    turns: Vec<String>,
    turns_chainable: Vec<String>,
    denial: Vec<String>,
    denial_listed: Vec<String>,
    game_changers: Vec<String>,
    early: Vec<String>,
    late: Vec<String>,
}

const TOLERANCE: f64 = 1e-9;

fn load() -> (Vec<Fixture>, BTreeMap<String, Golden>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let decks = serde_json::from_str(&std::fs::read_to_string(format!("{dir}decks.json")).unwrap()).unwrap();
    let golden = serde_json::from_str(&std::fs::read_to_string(format!("{dir}golden.json")).unwrap()).unwrap();
    (decks, golden)
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn power_level_matches_reference() {
    let (decks, golden) = load();
    assert!(decks.len() >= 6);
    for d in &decks {
        let g = &golden[&d.file];
        let pl = power_level(&d.cards);
        for (what, got, want) in [
            ("level", pl.level, g.level),
            ("score", pl.score, g.score),
            ("impact", pl.impact, g.impact),
            ("efficiency", pl.efficiency, g.efficiency),
            ("avg_cmc", pl.avg_cmc, g.avg_cost),
        ] {
            assert!((got - want).abs() < TOLERANCE, "{}: {what} {got} != {want}", d.file);
        }
        assert_eq!((pl.curve_cmc, pl.lands, pl.nonlands), (g.curve_cmc, g.lands, g.nonlands), "{}", d.file);
    }
}

#[test]
fn bracket_matches_reference() {
    let (decks, golden) = load();
    for d in &decks {
        let g = &golden[&d.file];
        let r = bracket(&d.cards, &d.combos, g.level);
        assert_eq!((r.bracket, r.floor, r.from_level), (g.bracket, g.floor, g.from_level), "{}", d.file);
        assert_eq!(sorted(r.extra_turns), sorted(g.turns.clone()), "{} turns", d.file);
        assert_eq!(sorted(r.extra_turns_chainable), sorted(g.turns_chainable.clone()), "{}", d.file);
        assert_eq!(sorted(r.land_denial), sorted(g.denial.clone()), "{} denial", d.file);
        assert_eq!(sorted(r.land_denial_listed), sorted(g.denial_listed.clone()), "{}", d.file);
        assert_eq!(sorted(r.game_changers), sorted(g.game_changers.clone()), "{} GCs", d.file);
        let ids = |v: Vec<edhpl::ComboHit>| sorted(v.into_iter().map(|h| h.id).collect());
        assert_eq!(ids(r.early_combos), sorted(g.early.clone()), "{} early", d.file);
        assert_eq!(ids(r.late_combos), sorted(g.late.clone()), "{} late", d.file);
    }
}

#[test]
fn fixture_covers_every_rule() {
    let (_, golden) = load();
    let any = |f: fn(&Golden) -> bool| golden.values().any(f);
    assert!(any(|g| !g.early.is_empty()) && any(|g| !g.late.is_empty()));
    assert!(any(|g| !g.turns.is_empty()) && any(|g| !g.turns_chainable.is_empty()));
    assert!(any(|g| !g.denial.is_empty()) && any(|g| g.game_changers.len() > 3));
}

#[test]
fn curve_matches_reference() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/curve_golden.json");
    let cases: Vec<(String, f64, f64)> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (name, value, want) in cases {
        let got = match name.as_str() {
            "price" => curve(value, &edhpl::PRICE_CURVE, 1.0 + edhpl::FAVOR_PRICE),
            "pop" => curve(value, &edhpl::POP_CURVE, 1.0 - edhpl::FAVOR_PRICE),
            "power" => curve(value, &edhpl::POWER_CURVE, 1.0),
            other => panic!("unknown curve {other}"),
        };
        assert!((got - want).abs() < TOLERANCE, "{name}({value}) = {got}, want {want}");
    }
}
