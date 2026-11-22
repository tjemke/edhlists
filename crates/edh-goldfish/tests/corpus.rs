//! The goldfish on a fixed set of decks (`tests/data/decks`: EDHREC lists and precons picked for
//! their mana: snow, Phyrexian, colourless, five colours, lands, artifacts, exile-and-play, elves,
//! mana Auras, Treasure, landfall, bounce lands and extra land drops, cost reducers, rituals, a
//! dork commander), to see what a change does before keeping it, and how far each deck's numbers
//! can be trusted:
//!
//! ```sh
//! cargo test --release -p edh-goldfish --test corpus -- --ignored --nocapture
//! ```
//!
//! Needs the card database `edh-scout sync` builds, and reads Forge's card scripts from where
//! edh-scout would (`EDH_SCOUT_FORGE_CARDS`, or the ones `sync` downloads); `CORPUS_FORGE=0`
//! reads oracle text only. `CORPUS_GAMES` sets the games per deck (default 100,000: shares to
//! about ±0.3 points).

use edh_data::cards::CardDb;
use edh_data::config::Paths;
use edh_data::forge::ForgeSource;
use edh_data::tags::Role;
use edh_goldfish::{DeckEntry, run, seed_for};

#[test]
#[ignore = "needs the card database; prints numbers rather than checking them"]
fn corpus() {
    let paths = Paths::new(None, None).expect("edh-scout's directories");
    let cards = CardDb::load(&paths.cards_db()).expect("the card database: run `edh-scout sync` first");
    let forge = match std::env::var("CORPUS_FORGE").as_deref() {
        Ok("0") => None,
        _ => Some(
            ForgeSource::find(None)
                .load(&paths.forge_db(), &paths.forge_raw())
                .expect("Forge's scripts")
                .expect("Forge's scripts: run `edh-scout sync --forge` or set EDH_SCOUT_FORGE_CARDS"),
        ),
    };
    let games = std::env::var("CORPUS_GAMES").ok().and_then(|g| g.parse().ok()).unwrap_or(100_000);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/decks");
    let mut files: Vec<_> = std::fs::read_dir(dir).expect("the corpus").map(|e| e.expect("an entry").path()).collect();
    files.sort();
    println!("cards read from {}", forge.as_ref().map_or("oracle text", |db| db.source.as_str()));
    println!(
        "{games} games a deck\n{:30} {:>9} {:>8} {:>7} {:>7} {:>6}  {:5}  not played",
        "deck", "cmd T4", "colours", "mana T4", "mana T8", "mull", "trust"
    );
    for path in files {
        let deck = edh_data::deck::parse_decklist(&std::fs::read_to_string(&path).expect("a decklist"), &[]);
        let entries: Vec<DeckEntry> = deck
            .entries()
            .filter_map(|(name, qty, commander)| {
                cards.get(name).map(|card| DeckEntry {
                    card,
                    qty,
                    commander,
                    script: forge.as_ref().and_then(|db| db.get(&card.name)),
                })
            })
            .collect();
        let name = path.file_stem().expect("a name").to_string_lossy().to_string();
        let r = run(&entries, games, seed_for(&name));
        let pct = |x: Option<f64>| x.map_or("–".to_string(), |x| format!("{:.1}%", x * 100.0));
        let ramp = r.unplayed.iter().filter(|(_, role)| *role == Role::Ramp).count();
        println!(
            "{name:30} {:>9} {:>8} {:>7.2} {:>7.2} {:>5.1}%  {:5}  {ramp} ramp, {} draw; doubts: {}{}",
            pct(r.commander_by_t4),
            pct(r.colour_delayed),
            r.capacity[3],
            r.capacity[7],
            r.mulligan * 100.0,
            format!("{:?}", r.doubts.confidence),
            r.unplayed.len() - ramp,
            if r.doubts.commander_ramp.is_empty() { String::new() } else { "commander, ".into() },
            [
                ("engines", &r.doubts.engines),
                ("one-shot", &r.doubts.one_shot),
                ("other", &r.doubts.other_ramp),
                ("growing", &r.doubts.growing),
                ("unread", &r.doubts.unread)
            ]
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k} {}", v.join(", ")))
            .collect::<Vec<_>>()
            .join("; ")
        );
    }
}
