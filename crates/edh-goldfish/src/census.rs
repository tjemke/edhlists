//! A census of how the goldfish reads every card that matters for mana or draw, for audits: what
//! each card's script and oracle text come to, how faithfully, and which of Forge's constructs the
//! reader meets and never looks at. Not a check; it writes files to compare and read:
//!
//! ```sh
//! CENSUS_OUT=/some/dir cargo test --release -p edh-goldfish census -- --ignored --nocapture
//! ```
//!
//! Needs the card database (`edh-scout sync`) and Forge's scripts (`sync --forge` or
//! `EDH_SCOUT_FORGE_CARDS`). What it writes is derived from Forge's GPL scripts, so it stays out of
//! the repo:
//!
//! - `cards.jsonl`: per card, its rank, roles, oracle text, script, both readings (structured),
//!   fidelity and the constructs in its script.
//! - `readings.tsv`: one line per card, name and script reading, sorted: diff two runs to see
//!   every reading a change moves, across the whole pool rather than the corpus.
//! - `fidelity.txt`: cards by how they were read, most popular first.
//! - `apis.tsv`: each effect, trigger, static, replacement and keyword in their scripts, with how
//!   many cards use it, how many popular ones, how many of those aren't read exactly, and the
//!   params on it, `!` marking those the reader's source never names (its blind spots).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use edh_data::cards::{Card, CardDb};
use edh_data::config::Paths;
use edh_data::forge::{ForgeSource, Script, params};
use edh_data::mana::ColorSet;
use edh_data::tags::Role;
use serde_json::{Value, json};

use crate::profile::{Effect, Enters, Fidelity, LandKind, LeftOut, Profile, Unit};

/// Cards ranked this high or better count as popular (EDHREC's rank; about the top 1,250 mana
/// cards).
const POPULAR: u32 = 4000;

/// Where the reader reads scripts: a param named in neither is one it never looks at.
const READER_SOURCES: [&str; 2] = [include_str!("forge_profile.rs"), include_str!("../../edh-data/src/forge.rs")];

#[test]
#[ignore = "needs the card database and Forge's scripts; writes files for an audit"]
fn census() {
    let out = PathBuf::from(std::env::var("CENSUS_OUT").expect("CENSUS_OUT: a directory to write the census to"));
    std::fs::create_dir_all(&out).expect("the census directory");
    let paths = Paths::new(None, None).expect("edh-scout's directories");
    let cards = CardDb::load(&paths.cards_db()).expect("the card database: run `edh-scout sync` first");
    let forge = ForgeSource::find(None)
        .load(&paths.forge_db(), &paths.forge_raw())
        .expect("Forge's scripts")
        .expect("Forge's scripts: run `edh-scout sync --forge` or set EDH_SCOUT_FORGE_CARDS");
    let all = ColorSet::from_letters("WUBRG").expect("five colours");
    let named = Named::new();

    let mut relevant: Vec<&Card> = cards.cards.iter().filter(|c| matters(c)).collect();
    relevant.sort_by_key(|c| (c.rank.unwrap_or(u32::MAX), c.name.clone()));
    let (mut jsonl, mut readings) = (String::new(), Vec::new());
    let mut fidelity = BTreeMap::<String, Vec<&str>>::new();
    let mut apis = BTreeMap::<String, Api>::new();
    for card in &relevant {
        let script = forge.get(&card.name);
        let oracle = Profile::from_oracle(card, all);
        let read = Profile::of(card, script, all);
        let mut constructs = BTreeMap::<String, BTreeSet<String>>::new();
        for (api, keys) in script.map(self::constructs).unwrap_or_default() {
            constructs.entry(api).or_default().extend(keys);
        }
        let popular = card.rank.is_some_and(|r| r <= POPULAR);
        for (api, keys) in &constructs {
            let a = apis.entry(api.clone()).or_default();
            a.cards += 1;
            if popular {
                a.popular += 1;
                a.popular_inexact += usize::from(read.fidelity != Fidelity::Exact);
                if a.examples.len() < 4 {
                    a.examples.push(card.name.clone());
                }
            }
            for k in keys {
                *a.keys.entry(k.clone()).or_default() += 1;
            }
        }
        let reading = reading(&read);
        readings.push(format!("{}\t{}\t{}", card.name, fidelity_label(&read.fidelity), reading));
        fidelity.entry(fidelity_label(&read.fidelity)).or_default().push(&card.name);
        let record = json!({
            "name": card.name,
            "rank": card.rank,
            "roles": card.roles,
            "type_line": card.type_line,
            "layout": format!("{:?}", card.layout),
            "mana_cost": card.mana_cost.iter().map(|c| c.as_ref().map(|c| c.to_string())).collect::<Vec<_>>(),
            "oracle": card.oracle,
            "produced": card.produced.to_string(),
            "script": script.map(|s| s.faces.iter().map(|f| f.lines.join("\n")).collect::<Vec<_>>()),
            "fidelity": fidelity_label(&read.fidelity),
            "reading": reading,
            "oracle_reading": self::reading(&oracle),
            "constructs": constructs.keys().collect::<Vec<_>>(),
        });
        jsonl.push_str(&record.to_string());
        jsonl.push('\n');
    }
    readings.sort();

    let mut fid = String::new();
    for (label, names) in &fidelity {
        let _ = writeln!(fid, "{} {label}: {}\n", names.len(), names.join("; "));
    }
    let mut table =
        String::from("api\tcards\tpopular\tpopular not exact\tparams (! never named by the reader)\texamples\n");
    let mut by_use: Vec<_> = apis.iter().collect();
    by_use.sort_by_key(|(name, a)| (std::cmp::Reverse(a.popular), std::cmp::Reverse(a.cards), (*name).clone()));
    for (name, a) in by_use {
        let keys: Vec<String> =
            a.keys.iter().map(|(k, n)| format!("{}{k}:{n}", if named.names(k) { "" } else { "!" })).collect();
        let _ = writeln!(
            table,
            "{name}\t{}\t{}\t{}\t{}\t{}",
            a.cards,
            a.popular,
            a.popular_inexact,
            keys.join(" "),
            a.examples.join("; ")
        );
    }
    let write = |file: &str, text: &str| std::fs::write(out.join(file), text).expect("a census file");
    write("cards.jsonl", &jsonl);
    write("readings.tsv", &(readings.join("\n") + "\n"));
    write("fidelity.txt", &fid);
    write("apis.tsv", &table);
    println!("{} cards read from {} into {}", relevant.len(), forge.source, out.display());
}

/// Cards that matter for a deck's mana or draw: lands, anything that makes mana, and what
/// Scryfall Tagger calls ramp or draw. Not joke cards nobody sells.
fn matters(card: &Card) -> bool {
    let relevant = card.is_land()
        || !card.produced.is_empty()
        || card.roles.contains(Role::Ramp)
        || card.roles.contains(Role::Draw);
    relevant && !(card.banned && card.usd == 0.0)
}

#[derive(Default)]
struct Api {
    cards: usize,
    popular: usize,
    popular_inexact: usize,
    keys: BTreeMap<String, usize>,
    examples: Vec<String>,
}

/// Each line's construct in a script and the params on it: `AB$ Mana`, `DB$ ChangeZone`,
/// `T:ChangesZone`, `S:ReduceCost`, `R:Moved`, `K:Multikicker`, `SVar:Count$` (a computed amount).
fn constructs(script: &Script) -> Vec<(String, BTreeSet<String>)> {
    let mut found = Vec::new();
    for face in &script.faces {
        for line in &face.lines {
            let Some((key, rest)) = line.split_once(':') else { continue };
            let (key, ability) = match key {
                "A" | "T" | "S" | "R" => (key, rest),
                "SVar" => match rest.split_once(':') {
                    Some((_, body)) => ("SVar", body),
                    None => continue,
                },
                "K" => {
                    found.push((format!("K:{}", rest.split(':').next().unwrap_or(rest)), BTreeSet::new()));
                    continue;
                }
                _ => continue,
            };
            let p = params(ability);
            let Some(&(first, value)) = p.first() else { continue };
            let api = match (key, first) {
                (_, "AB" | "SP" | "DB") => format!("{first}$ {value}"),
                ("SVar", _) if ability.starts_with("Count$") => "SVar:Count$".to_string(),
                ("SVar", _) => continue,
                _ => format!("{key}:{value}"),
            };
            found.push((api, p.iter().skip(1).map(|(k, _)| (*k).to_string()).collect()));
        }
    }
    found
}

/// Which params the reader's source names, as `"Key"` or as a prefix it checks with
/// `starts_with("Key")`.
struct Named {
    exact: BTreeSet<String>,
    prefixes: Vec<String>,
}

impl Named {
    fn new() -> Named {
        let quoted = regex::Regex::new(r#"(starts_with\()?"([A-Za-z]+)"#).expect("a valid pattern");
        let (mut exact, mut prefixes) = (BTreeSet::new(), Vec::new());
        for src in READER_SOURCES {
            for c in quoted.captures_iter(src) {
                exact.insert(c[2].to_string());
                if c.get(1).is_some() {
                    prefixes.push(c[2].to_string());
                }
            }
        }
        Named { exact, prefixes }
    }

    fn names(&self, key: &str) -> bool {
        self.exact.contains(key) || self.prefixes.iter().any(|p| key.starts_with(p.as_str()))
    }
}

fn fidelity_label(f: &Fidelity) -> String {
    match f {
        Fidelity::Exact => "exact".into(),
        Fidelity::NoScript => "no script".into(),
        Fidelity::Approximate(why) => format!("approximate ({why})"),
        Fidelity::Unreadable(why) => format!("unreadable ({why})"),
    }
}

fn units(units: &[Unit]) -> Vec<String> {
    units.iter().map(|u| u.to_string()).collect()
}

fn enters(e: &Enters) -> Value {
    match e {
        Enters::Untapped => json!("untapped"),
        Enters::Tapped => json!("tapped"),
        Enters::PayLife => json!("untapped for life"),
        Enters::UnlessControl(types) => json!({ "untapped if you control": types }),
        Enters::UnlessReveal(types) => json!({ "untapped if you reveal": types }),
        Enters::UnlessBasics(n) => json!({ "untapped with basics": n }),
        Enters::UnlessLandsAtLeast(n) => json!({ "untapped with other lands at least": n }),
        Enters::UnlessLandsAtMost(n) => json!({ "untapped with other lands at most": n }),
    }
}

/// A reading as data, in the order the goldfish uses it.
fn reading(p: &Profile) -> Value {
    let lands: Vec<Value> = p
        .lands
        .iter()
        .map(|face| {
            let mut land = match &face.kind {
                LandKind::Mana(l) => json!({
                    "makes": units(&l.units),
                    "with_lands": l.with_lands.as_ref().map(|(n, u)| json!({ "lands": n, "makes": units(u) })),
                    "enters": enters(&l.enters),
                    "bounce": l.bounce,
                    "sacrifices_lands": l.sacrifices,
                }),
                LandKind::Fetch { finds, tapped } => json!({
                    "fetch": { "basic": finds.basic, "types": finds.types, "tapped": tapped },
                }),
            };
            if face.mdfc_back {
                land["back_face"] = json!(true);
            }
            land
        })
        .collect();
    let spell = p.spell.as_ref().map(|s| {
        let effect = match &s.effect {
            Effect::Rock { units: u, next_turn, once } => {
                json!({ "rock": { "makes": units(u), "from_next_turn": next_turn, "once": once } })
            }
            Effect::LandRamp { finds, to_battlefield, to_hand, tapped, activation, sacrifice } => json!({
                "land_ramp": {
                    "basic": finds.basic, "types": finds.types, "to_battlefield": to_battlefield,
                    "to_hand": to_hand, "tapped": tapped, "activation": activation, "sacrifices_lands": sacrifice,
                }
            }),
            Effect::Draw { cards } => json!({ "draw": cards }),
            Effect::Engine => json!("draw engine"),
            Effect::Refill { to } => json!({ "refill": to }),
            Effect::Other => json!("other"),
        };
        json!({ "cost": s.cost.to_string(), "extra": s.extra(), "effect": effect })
    });
    let left_out = p.left_out.map(|l| match l {
        LeftOut::Engine => "engine",
        LeftOut::OneShot => "one-shot",
        LeftOut::Other => "other",
    });
    json!({
        "lands": lands,
        "spell": spell,
        "not_played": p.unplayed.map(|r| r.label()),
        "left_out": left_out,
        "undercounted": p.undercounted,
    })
}
