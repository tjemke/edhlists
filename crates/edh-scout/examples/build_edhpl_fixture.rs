//! Rebuild `crates/edhpl/tests/data/decks.json`: the test decks resolved against today's card and
//! combo data, in the input shape both the Rust port and the JavaScript reference read.
//!
//!     cargo run --example build_edhpl_fixture
//!     node crates/edhpl/reference/edhpl_reference.js crates/edhpl/tests/data/decks.json \
//!         > crates/edhpl/tests/data/golden.json
//!
//! Prices and ranks move daily, which is why the fixture is pinned rather than read live.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use edh_data::config::Paths;
use edh_data::deck::parse_decklist;
use edh_data::http::Http;
use edh_data::sync::ensure_databases;
use edh_scout::analysis::{edhpl_card, edhpl_combo, included_combos, resolve};
use serde_json::json;

fn main() -> Result<()> {
    let paths = Paths::new(None)?;
    let http = Http::new(paths.http(), false);
    let (cards, combos) = ensure_databases(&http, &paths)?;
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edhpl/tests/data");
    let mut files: Vec<PathBuf> = fs::read_dir(data.join("decks"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let deck = parse_decklist(&fs::read_to_string(&path)?, &[]);
        let (entries, unknown) = resolve(&deck, &cards);
        let included = included_combos(&entries, &combos);
        let name = path.file_name().context("file name")?.to_string_lossy().to_string();
        eprintln!("{name}: {} cards, {} combos, unknown: {unknown:?}", entries.len(), included.len());
        out.push(json!({
            "file": name,
            "cards": entries.iter().map(|e| edhpl_card(e.card, e.qty, e.commander)).collect::<Vec<_>>(),
            "combos": included.iter().map(|c| edhpl_combo(c)).collect::<Vec<_>>(),
        }));
    }
    let dest = data.join("decks.json");
    fs::write(&dest, serde_json::to_string_pretty(&out)? + "\n")?;
    eprintln!("wrote {}", dest.display());
    Ok(())
}
