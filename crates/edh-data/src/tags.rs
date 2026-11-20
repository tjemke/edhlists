//! Scryfall Tagger oracle tags ("otags"): community-curated labels for what a card *does*.
//!
//! Two uses: the four structural roles — is it ramp, card draw, removal/interaction, a board wipe —
//! are folded into a set of [`Role`]s at build time ([`load_role_tags`]); and `otag:` searches read the
//! whole tag tree on demand ([`TagIndex`]).

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::flags::{EnumSet, flags};
use crate::storage::open_maybe_gz;

flags! {
    /// A structural role in a deck, as the deckbuilding template counts them.
    pub enum Role { Ramp = "Ramp", Draw = "Draw", Removal = "Removal", Wipe = "Wipe" }
}

pub type Roles = EnumSet<Role>;

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Ramp => "Ramp",
            Role::Draw => "Card draw",
            Role::Removal => "Removal",
            Role::Wipe => "Board wipes",
        }
    }
    /// Fits the detail pane's label column.
    pub fn short_label(self) -> &'static str {
        match self {
            Role::Ramp => "Ramp",
            Role::Draw => "Draw",
            Role::Removal => "Removal",
            Role::Wipe => "Wipes",
        }
    }
}

/// (role, root tag labels, labels whose subtrees are excluded)
const ROLES: &[(Role, &[&str], &[&str])] = &[
    (Role::Ramp, &["ramp"], &["ritual"]),
    (
        Role::Draw,
        &["card advantage"],
        &["loot", "rummage", "loot to library", "rummage to library", "loot to exile", "regrowth"],
    ),
    (Role::Removal, &["removal", "counterspell"], &["sweeper", "removal-land"]),
    (Role::Wipe, &["sweeper"], &[]),
];

#[derive(Deserialize)]
struct RawTag {
    id: String,
    label: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    aliases: Vec<String>,
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
    pub roles: HashMap<String, Roles>,
    /// Every oracle id that carries any tag at all (untagged cards fall back to heuristics).
    pub tagged: HashSet<String>,
}

/// The raw tag file (JSON Lines, possibly gzipped), by tag id.
fn read_raw(reader: impl BufRead) -> Result<HashMap<String, RawTag>> {
    let mut tags = HashMap::new();
    for line in reader.lines() {
        let line = line?;
        let line = line.trim().trim_end_matches(',');
        if line.starts_with('{') {
            let t: RawTag = serde_json::from_str(line).context("parsing a Scryfall oracle tag")?;
            tags.insert(t.id.clone(), t);
        }
    }
    Ok(tags)
}

pub fn load_role_tags(path: &Path) -> Result<RoleTags> {
    let tags = read_raw(open_maybe_gz(path)?)?;
    let by_label: HashMap<&str, &RawTag> = tags.values().map(|t| (t.label.as_str(), t)).collect();
    let mut out = RoleTags::default();
    for t in tags.values() {
        out.tagged.extend(t.taggings.iter().map(|x| x.oracle_id.clone()));
    }
    for &(role, roots, excluded) in ROLES {
        let mut stack: Vec<&RawTag> = roots.iter().filter_map(|r| by_label.get(r).copied()).collect();
        let mut seen = HashSet::new();
        while let Some(t) = stack.pop() {
            if excluded.contains(&t.label.as_str()) || !seen.insert(t.id.as_str()) {
                continue;
            }
            for x in &t.taggings {
                out.roles.entry(x.oracle_id.clone()).or_default().insert(role);
            }
            stack.extend(t.child_ids.iter().filter_map(|c| tags.get(c)));
        }
    }
    Ok(out)
}

/// How `otag:` names a tag: lowercase, words joined by hyphens ("mana rock" -> "mana-rock").
pub fn tag_slug(name: &str) -> String {
    name.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
}

struct IndexedTag {
    slug: String,
    children: Vec<usize>,
    oracle_ids: Vec<String>,
}

/// Every Tagger tag with its sub-tags, for `otag:` searches.
pub struct TagIndex {
    tags: Vec<IndexedTag>,
    /// Slugs, slugified labels and aliases -> tag.
    by_name: HashMap<String, usize>,
}

impl TagIndex {
    /// Read Scryfall's raw tag file (the one `sync` downloads).
    pub fn load(path: &Path) -> Result<Self> {
        Self::read(open_maybe_gz(path)?)
    }

    pub fn read(reader: impl BufRead) -> Result<Self> {
        let raw = read_raw(reader)?;
        let mut raw: Vec<RawTag> = raw.into_values().collect();
        raw.sort_by(|a, b| a.id.cmp(&b.id));
        let position: HashMap<&str, usize> = raw.iter().enumerate().map(|(i, t)| (t.id.as_str(), i)).collect();
        let tags: Vec<IndexedTag> = raw
            .iter()
            .map(|t| IndexedTag {
                slug: if t.slug.is_empty() { tag_slug(&t.label) } else { t.slug.clone() },
                children: t.child_ids.iter().filter_map(|c| position.get(c.as_str()).copied()).collect(),
                oracle_ids: t.taggings.iter().map(|x| x.oracle_id.clone()).collect(),
            })
            .collect();
        let mut by_name = HashMap::with_capacity(tags.len() * 2);
        // Real slugs win over labels, labels over aliases.
        for (i, t) in tags.iter().enumerate() {
            by_name.insert(t.slug.clone(), i);
        }
        for (i, t) in raw.iter().enumerate() {
            by_name.entry(tag_slug(&t.label)).or_insert(i);
        }
        for (i, t) in raw.iter().enumerate() {
            for a in &t.aliases {
                by_name.entry(tag_slug(a)).or_insert(i);
            }
        }
        Ok(Self { tags, by_name })
    }

    /// The oracle ids tagged `name` or any of its sub-tags, as Scryfall's `otag:` finds them.
    /// `None` if there is no such tag.
    pub fn oracle_ids(&self, name: &str) -> Option<HashSet<String>> {
        let root = *self.by_name.get(&tag_slug(name))?;
        let mut out = HashSet::new();
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            if !seen.insert(i) {
                continue;
            }
            out.extend(self.tags[i].oracle_ids.iter().cloned());
            stack.extend(&self.tags[i].children);
        }
        Some(out)
    }

    /// Tag slugs containing `name` or starting like it, shortest first (for "did you mean").
    pub fn suggestions(&self, name: &str) -> Vec<&str> {
        let needle = tag_slug(name);
        let stem: String = needle.chars().take(4).collect();
        let near = |s: &str| s.contains(needle.as_str()) || (stem.chars().count() == 4 && s.starts_with(&stem));
        let mut hits: Vec<&str> = self.tags.iter().map(|t| t.slug.as_str()).filter(|s| near(s)).collect();
        hits.sort_by_key(|s| (s.len(), *s));
        hits.truncate(8);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = concat!(
        r#"{"id":"1","label":"sweeper","slug":"sweeper","aliases":["wipe","mass removal"],"child_ids":["2"],"taggings":[{"oracle_id":"wrath"}]}"#,
        "\n",
        r#"{"id":"2","label":"sweeper one sided","slug":"sweeper-one-sided","child_ids":[],"taggings":[{"oracle_id":"cyclonic"}]}"#,
        "\n",
        r#"{"id":"3","label":"removal","slug":"removal","child_ids":["1","4"],"taggings":[]}"#,
        "\n",
        r#"{"id":"4","label":"mana rock","slug":"mana-rock","child_ids":[],"taggings":[{"oracle_id":"signet"}]}"#,
        "\n",
    );

    #[test]
    fn tags_include_their_sub_tags_and_answer_to_aliases() {
        let idx = TagIndex::read(RAW.as_bytes()).unwrap();
        let ids = |n: &str| {
            let mut v: Vec<String> = idx.oracle_ids(n).unwrap().into_iter().collect();
            v.sort();
            v
        };
        assert_eq!(ids("sweeper"), vec!["cyclonic", "wrath"]);
        assert_eq!(ids("wipe"), ids("sweeper"));
        assert_eq!(ids("Mass Removal"), ids("sweeper"));
        assert_eq!(ids("removal"), vec!["cyclonic", "signet", "wrath"]);
        assert_eq!(ids("mana rock"), vec!["signet"]);
        assert!(idx.oracle_ids("nope").is_none());
        assert_eq!(idx.suggestions("sweep"), vec!["sweeper", "sweeper-one-sided"]);
        assert_eq!(idx.suggestions("sweepr"), vec!["sweeper", "sweeper-one-sided"]);
    }

    #[test]
    fn roles_are_written_by_name_in_template_order() {
        let roles: Roles = [Role::Wipe, Role::Ramp].into_iter().collect();
        assert_eq!(serde_json::to_string(&roles).unwrap(), r#"["Ramp","Wipe"]"#);
        assert_eq!(serde_json::to_string(&Role::Draw).unwrap(), r#""Draw""#, "as the enum's variant was before");
    }
}
