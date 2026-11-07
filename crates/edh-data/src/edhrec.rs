//! Reading EDHREC.
//!
//! * the deck table for a commander (`json.edhrec.com/pages/decks/<slug>.json`): every public deck
//!   with price, salt, card-type counts, tags, save date and declared bracket — one request;
//! * individual decks (`edhrec.com/deckpreview/<hash>`): since the 2026 redesign these are
//!   server-rendered Next.js App Router pages whose RSC payload embeds the full deck as JSON;
//! * the commander page (per-card synergy and lift), the commander's combos page (with community
//!   bracket votes) and the top-salt list.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::config::{EDHREC_JSON, EDHREC_SITE, TTL_COMMANDER_PAGE, TTL_DECK_TABLE, TTL_SALT};
use crate::deck::{Deck, DeckMeta};
use crate::http::{Http, Ttl};
use crate::storage::atomic_write;

// --- deck table -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TableRow {
    pub urlhash: String,
    pub savedate: String,
    pub price: f64,
    pub salt: f64,
    pub creature: u32,
    pub instant: u32,
    pub sorcery: u32,
    pub artifact: u32,
    pub enchantment: u32,
    pub battle: u32,
    pub planeswalker: u32,
    pub land: u32,
    pub bracket: Option<u8>,
    pub tags: Vec<String>,
}

impl TableRow {
    pub fn card_count(&self) -> u32 {
        self.creature
            + self.instant
            + self.sorcery
            + self.artifact
            + self.enchantment
            + self.battle
            + self.planeswalker
            + self.land
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DeckTable {
    pub header: String,
    pub table: Vec<TableRow>,
    pub similar: Vec<String>,
}

pub fn deck_table_url(slug: &str) -> String {
    format!("{EDHREC_JSON}/decks/{slug}.json")
}

pub fn deck_table(http: &Http, slug: &str) -> Result<DeckTable> {
    let url = deck_table_url(slug);
    http.cached_json(&url, None, Ttl::For(TTL_DECK_TABLE))
        .with_context(|| format!("EDHREC deck table for '{slug}' (check the commander name or pass --slug)"))
}

// --- a single deck --------------------------------------------------------------------------------

pub fn deck_url(urlhash: &str) -> String {
    format!("{EDHREC_SITE}/deckpreview/{urlhash}")
}

/// The decklists we have fetched, kept as the raw deck pages (`<urlhash>.html`) so a parser fix
/// never needs a refetch. Decks are snapshots: once stored, a deck never costs a request again.
/// Lives in the XDG *state* dir (see [`crate::config::Paths`]) so cache cleanups don't eat it.
pub struct DeckStore {
    dir: PathBuf,
}

impl DeckStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `None` for anything that isn't a plain EDHREC urlhash (so it can't escape the directory).
    fn path(&self, urlhash: &str) -> Option<PathBuf> {
        let ok = !urlhash.is_empty() && urlhash.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        ok.then(|| self.dir.join(format!("{urlhash}.html")))
    }

    pub fn contains(&self, urlhash: &str) -> bool {
        self.path(urlhash).is_some_and(|p| p.exists())
    }

    /// The deck from disk, or fetched (one request) and stored.
    pub fn fetch(&self, http: &Http, urlhash: &str) -> Result<Deck> {
        let path = self.path(urlhash).ok_or_else(|| anyhow!("'{urlhash}' is not an EDHREC deck id"))?;
        let html = match std::fs::read_to_string(&path) {
            Ok(html) => html,
            Err(_) => {
                let bytes = http.get_bytes(&deck_url(urlhash))?;
                let html = String::from_utf8(bytes).with_context(|| format!("non-UTF-8 deck page {urlhash}"))?;
                // Parse before storing, so a page we can't read is never kept as "fetched".
                let deck = parse_deck_page(&html, urlhash)?;
                atomic_write(&path, html.as_bytes())?;
                return Ok(deck);
            }
        };
        parse_deck_page(&html, urlhash)
    }
}

static RSC_PUSH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"self\.__next_f\.push\(\[1,"((?:[^"\\]|\\.)*)"\]\)"#).unwrap());

/// Concatenate the React Server Components payload chunks embedded in a Next.js page.
pub fn rsc_payload(html: &str) -> String {
    RSC_PUSH
        .captures_iter(html)
        .filter_map(|c| serde_json::from_str::<String>(&format!("\"{}\"", &c[1])).ok())
        .collect()
}

/// Find the first JSON object in `blob` that encloses `anchor` and satisfies `accept`.
fn enclosing_object(blob: &str, anchor: &str, accept: impl Fn(&Value) -> bool) -> Option<Value> {
    for (pos, _) in blob.match_indices(anchor) {
        let floor = pos.saturating_sub(20_000);
        let mut j = pos;
        while j > floor {
            j -= 1;
            if blob.as_bytes()[j] != b'{' || !blob.is_char_boundary(j) {
                continue;
            }
            let mut it = serde_json::Deserializer::from_str(&blob[j..]).into_iter::<Value>();
            if let Some(Ok(v)) = it.next()
                && v.get("recs").is_some()
                && accept(&v)
            {
                return Some(v);
            }
        }
    }
    None
}

pub fn parse_deck_page(html: &str, urlhash: &str) -> Result<Deck> {
    let blob = rsc_payload(html);
    if blob.is_empty() {
        bail!("deck page {urlhash} has no RSC payload — EDHREC's page format may have changed");
    }
    let data = enclosing_object(&blob, "\"recs\":[", |v| {
        v.get("commanderRec").is_some() || v.get("price").is_some() || v.get("salt").is_some()
    })
    .ok_or_else(|| anyhow!("no decklist in deck page {urlhash} — EDHREC's page format may have changed"))?;

    let mut commanders = Vec::new();
    let mut cards = std::collections::BTreeMap::new();
    for rec in data["recs"].as_array().into_iter().flatten() {
        let Some(name) = rec["name"].as_str() else { continue };
        let qty = rec["count"].as_u64().unwrap_or(1) as u32;
        if rec["role"].as_str() == Some("commander") || rec["category"].as_str() == Some("type-commander") {
            if !commanders.iter().any(|c| c == name) {
                commanders.push(name.to_string());
            }
        } else {
            *cards.entry(name.to_string()).or_insert(0) += qty;
        }
    }
    if commanders.is_empty()
        && let Some(n) = data["commanderRec"]["name"].as_str()
    {
        commanders.push(n.to_string());
        if let Some(p) = data["commanderRec"]["partnerRef"]["name"].as_str() {
            commanders.push(p.to_string());
        }
    }
    if cards.is_empty() {
        bail!("deck {urlhash} came back empty — EDHREC's payload may have moved");
    }
    let tags = data["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(String::from)).collect();
    Ok(Deck {
        id: urlhash.to_string(),
        commanders,
        cards,
        meta: DeckMeta {
            edhrec_url: Some(deck_url(urlhash)),
            source_url: data["url"].as_str().map(String::from),
            savedate: data["savedate"].as_str().map(String::from),
            price: data["price"].as_f64(),
            salt: data["salt"].as_f64(),
            tags,
            declared_bracket: None,
        },
    })
}

// --- commander page: synergy -------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct CardStat {
    pub synergy: f64,
    pub lift: f64,
    pub inclusion: f64,
}

#[derive(Clone, Debug, Default)]
pub struct CommanderPage {
    pub num_decks: u64,
    pub commander_salt: Option<f64>,
    /// Per-card synergy for the cards EDHREC lists on the commander page (front-face names).
    pub cards: HashMap<String, CardStat>,
}

pub fn commander_page(http: &Http, slug: &str) -> Result<CommanderPage> {
    let url = format!("{EDHREC_JSON}/commanders/{slug}.json");
    let v: Value = http.cached_json(&url, None, Ttl::For(TTL_COMMANDER_PAGE))?;
    Ok(parse_commander_page(&v))
}

pub fn parse_commander_page(v: &Value) -> CommanderPage {
    let jd = &v["container"]["json_dict"];
    let mut cards = HashMap::new();
    for list in jd["cardlists"].as_array().into_iter().flatten() {
        for cv in list["cardviews"].as_array().into_iter().flatten() {
            let Some(name) = cv["name"].as_str() else { continue };
            let potential = cv["potential_decks"].as_f64().unwrap_or(0.0);
            let inclusion = if potential > 0.0 { cv["num_decks"].as_f64().unwrap_or(0.0) / potential } else { 0.0 };
            cards.entry(name.to_string()).or_insert(CardStat {
                synergy: cv["synergy"].as_f64().unwrap_or(0.0),
                lift: cv["lift"].as_f64().unwrap_or(0.0),
                inclusion,
            });
        }
    }
    CommanderPage {
        num_decks: jd["card"]["num_decks"].as_u64().unwrap_or(0),
        commander_salt: jd["card"]["salt"].as_f64(),
        cards,
    }
}

// --- combos page: community bracket votes ----------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct EdhrecCombo {
    pub cards: Vec<String>,
    pub results: Vec<String>,
    pub deck_count: u64,
    /// Share of this commander's decks that run the combo, in percent.
    pub percentage: f64,
    /// Community vote on the lowest bracket this combo belongs in ("3", "4-5", "any", ...).
    pub vote_bracket: Option<String>,
    pub vote_count: u64,
    pub controversial: bool,
}

pub fn combos_page(http: &Http, slug: &str) -> Result<Vec<EdhrecCombo>> {
    let url = format!("{EDHREC_JSON}/combos/{slug}.json");
    let v: Value = http.cached_json(&url, None, Ttl::For(TTL_COMMANDER_PAGE))?;
    Ok(parse_combos_page(&v))
}

pub fn parse_combos_page(v: &Value) -> Vec<EdhrecCombo> {
    let mut out = Vec::new();
    for list in v["container"]["json_dict"]["cardlists"].as_array().into_iter().flatten() {
        let combo = &list["combo"];
        let vote = &combo["comboVote"];
        out.push(EdhrecCombo {
            cards: list["cardviews"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["name"].as_str().map(String::from))
                .collect(),
            results: combo["results"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str().map(String::from))
                .collect(),
            deck_count: combo["count"].as_u64().unwrap_or(0),
            percentage: combo["percentage"].as_f64().unwrap_or(0.0),
            vote_bracket: vote["bracket"].as_str().map(String::from),
            vote_count: vote["voteCount"].as_u64().unwrap_or(0),
            controversial: vote["controversial"].as_bool().unwrap_or(false),
        });
    }
    out
}

// --- salt -----------------------------------------------------------------------------------------

/// EDHREC's 100 saltiest cards with their salt score (0-4 scale), keyed by front-face name.
pub fn top_salt(http: &Http) -> Result<HashMap<String, f64>> {
    let url = format!("{EDHREC_JSON}/top/salt.json");
    let v: Value = http.cached_json(&url, None, Ttl::For(TTL_SALT))?;
    Ok(parse_top_salt(&v))
}

pub fn parse_top_salt(v: &Value) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for list in v["container"]["json_dict"]["cardlists"].as_array().into_iter().flatten() {
        for cv in list["cardviews"].as_array().into_iter().flatten() {
            if let (Some(n), Some(s)) = (cv["name"].as_str(), cv["salt"].as_f64()) {
                out.insert(n.to_string(), s);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/edhrec/");

    fn read(name: &str) -> String {
        std::fs::read_to_string(format!("{DATA}{name}")).unwrap()
    }

    #[test]
    fn single_commander_deck_page() {
        let d = parse_deck_page(&read("deckpreview_jodah.html"), "8K2SZWMbz80NrhCJmt9DsA").unwrap();
        assert_eq!(d.commanders, vec!["Jodah, the Unifier"]);
        assert_eq!(d.card_count(), 100);
        assert!(d.cards.contains_key("Sol Ring"));
        assert_eq!(d.meta.source_url.as_deref(), Some("https://archidekt.com/decks/24860094"));
        assert_eq!(d.meta.tags, vec!["Legends", "Go Wide", "Good Stuff", "Cascade"]);
        assert!((d.meta.salt.unwrap() - 75.526).abs() < 0.01);
        assert!((d.meta.price.unwrap() - 1350.86).abs() < 0.01);
    }

    #[test]
    fn partner_deck_page() {
        let d = parse_deck_page(&read("deckpreview_partner.html"), "qqdG7wTXY32jPgVRdP-x5g").unwrap();
        assert_eq!(d.commanders, vec!["Kraum, Ludevic's Opus", "Tymna the Weaver"]);
        assert_eq!(d.card_count(), 100);
    }

    #[test]
    fn deck_store_reads_from_disk_and_never_fetches_twice() {
        let state = tempfile::tempdir().unwrap();
        let store = DeckStore::new(state.path().join("decks"));
        let offline = Http::new(state.path().join("http"), true);
        let id = "8K2SZWMbz80NrhCJmt9DsA";
        assert!(!store.contains(id));
        let err = store.fetch(&offline, id).unwrap_err();
        assert!(err.downcast_ref::<crate::http::OfflineMiss>().is_some());
        atomic_write(&state.path().join("decks").join(format!("{id}.html")), read("deckpreview_jodah.html").as_bytes())
            .unwrap();
        assert!(store.contains(id));
        assert_eq!(store.fetch(&offline, id).unwrap().commanders, vec!["Jodah, the Unifier"]);
        assert!(offline.request_counts().is_empty());
        assert!(store.fetch(&offline, "../etc/passwd").is_err());
        assert!(!store.contains("../x"));
    }

    #[test]
    fn garbage_page_is_a_clear_error() {
        let err = parse_deck_page("<html>nothing</html>", "x").unwrap_err().to_string();
        assert!(err.contains("format may have changed"), "{err}");
    }

    #[test]
    fn deck_table_rows() {
        let t: DeckTable = serde_json::from_str(&read("decks_jodah_head.json")).unwrap();
        assert_eq!(t.table.len(), 400);
        let r = &t.table[1];
        assert_eq!(r.urlhash, "8K2SZWMbz80NrhCJmt9DsA");
        assert_eq!(r.bracket, Some(4));
        assert!((98..=101).contains(&r.card_count()));
        assert!(t.table.iter().any(|r| r.bracket.is_none()));
    }

    #[test]
    fn commander_page_synergy() {
        let v: Value = serde_json::from_str(&read("commander_jodah.json")).unwrap();
        let p = parse_commander_page(&v);
        assert_eq!(p.num_decks, 34431);
        let fl = &p.cards["Flowering of the White Tree"];
        assert!(fl.synergy > 0.2 && fl.lift > 5.0);
        assert!(p.cards.len() > 150);
    }

    #[test]
    fn combos_page_votes() {
        let v: Value = serde_json::from_str(&read("combos_jodah.json")).unwrap();
        let c = parse_combos_page(&v);
        assert_eq!(c.len(), 25);
        assert_eq!(c[0].cards, vec!["Najeela, the Blade-Blossom", "Esika, God of the Tree"]);
        assert_eq!(c[1].vote_bracket.as_deref(), Some("4-5"));
    }

    #[test]
    fn salt_list() {
        let v: Value = serde_json::from_str(&read("top_salt.json")).unwrap();
        let s = parse_top_salt(&v);
        assert_eq!(s.len(), 100);
        assert!(s["Stasis"] > 3.0);
    }
}
