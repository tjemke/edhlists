//! Scryfall Tagger oracle tags ("otags"): community-curated labels for what a card *does*.
//!
//! We only need four questions answered per card — is it ramp, card draw, removal/interaction, a
//! board wipe — so the tag hierarchy is folded into a bitmask at build time.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::storage::open_maybe_gz;

pub const RAMP: u8 = 1;
pub const DRAW: u8 = 2;
pub const REMOVAL: u8 = 4;
pub const WIPE: u8 = 8;

/// (role bit, root tag labels, labels whose subtrees are excluded)
const ROLES: &[(u8, &[&str], &[&str])] = &[
    (RAMP, &["ramp"], &["ritual"]),
    (
        DRAW,
        &["card advantage"],
        &["loot", "rummage", "loot to library", "rummage to library", "loot to exile", "regrowth"],
    ),
    (REMOVAL, &["removal", "counterspell"], &["sweeper", "removal-land"]),
    (WIPE, &["sweeper"], &[]),
];

#[derive(Deserialize)]
struct RawTag {
    id: String,
    label: String,
    #[serde(default)]
    child_ids: Vec<String>,
    #[serde(default)]
    taggings: Vec<Tagging>,
}

#[derive(Deserialize)]
struct Tagging {
    oracle_id: String,
}

#[derive(Default)]
pub struct RoleTags {
    pub roles: HashMap<String, u8>,
    /// Every oracle id that carries any tag at all (untagged cards fall back to heuristics).
    pub tagged: HashSet<String>,
}

pub fn load_role_tags(path: &Path) -> Result<RoleTags> {
    let mut tags: HashMap<String, RawTag> = HashMap::new();
    for line in open_maybe_gz(path)?.lines() {
        let line = line?;
        let line = line.trim().trim_end_matches(',');
        if line.starts_with('{') {
            let t: RawTag = serde_json::from_str(line).context("parsing a Scryfall oracle tag")?;
            tags.insert(t.id.clone(), t);
        }
    }
    let by_label: HashMap<&str, &RawTag> = tags.values().map(|t| (t.label.as_str(), t)).collect();
    let mut out = RoleTags::default();
    for t in tags.values() {
        out.tagged.extend(t.taggings.iter().map(|x| x.oracle_id.clone()));
    }
    for (bit, roots, excluded) in ROLES {
        let mut stack: Vec<&RawTag> = roots.iter().filter_map(|r| by_label.get(r).copied()).collect();
        let mut seen = HashSet::new();
        while let Some(t) = stack.pop() {
            if excluded.contains(&t.label.as_str()) || !seen.insert(t.id.as_str()) {
                continue;
            }
            for x in &t.taggings {
                *out.roles.entry(x.oracle_id.clone()).or_default() |= bit;
            }
            stack.extend(t.child_ids.iter().filter_map(|c| tags.get(c)));
        }
    }
    Ok(out)
}
