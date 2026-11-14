//! Reading EDHREC.
//!
//! * the deck table for a commander (`json.edhrec.com/pages/decks/<slug>.json`): every public deck
//!   with price, salt, card-type counts, tags, save date and declared bracket — one request;
//! * individual decks (`edhrec.com/deckpreview/<hash>`): since the 2026 redesign these are
//!   server-rendered Next.js App Router pages whose RSC payload embeds the full deck as JSON;
//! * the commander page (per-card synergy and lift), the commander's combos page (with community
//!   bracket votes) and the top-salt list.

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::config::{EDHREC_JSON, EDHREC_SITE, TTL_COMMANDER_PAGE, TTL_DECK_TABLE, TTL_SALT};
use crate::deck::{Deck, DeckMeta};
use crate::http::{Http, Ttl};

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

/// Bump when the parser reads deck pages differently (more fields, a fix): stored decks read by
/// an older version are re-read from the deck object kept with them (see [`deck_from_source`]).
pub const DECK_PARSER_VERSION: u32 = 1;

/// Is this a plain EDHREC urlhash (safe to use as a file name)?
pub fn is_deck_id(urlhash: &str) -> bool {
    !urlhash.is_empty() && urlhash.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A deck's page (one request).
pub fn fetch_deck_page(http: &Http, urlhash: &str) -> Result<String> {
    let bytes = http.get_bytes(&deck_url(urlhash))?;
    String::from_utf8(bytes).with_context(|| format!("non-UTF-8 deck page {urlhash}"))
}

/// What a deck page holds: the deck, and EDHREC's deck object it was read from (compact JSON,
/// ~9 KB of the page's ~270), which is all a later parser needs to read the deck again.
pub struct DeckPage {
    pub deck: Deck,
    pub source: String,
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
    read_deck_page(html, urlhash).map(|p| p.deck)
}

/// The deck on a deck page, with the deck object it came from.
pub fn read_deck_page(html: &str, urlhash: &str) -> Result<DeckPage> {
    let blob = rsc_payload(html);
    if blob.is_empty() {
        bail!("deck page {urlhash} has no RSC payload — EDHREC's page format may have changed");
    }
    let data = enclosing_object(&blob, "\"recs\":[", |v| {
        v.get("commanderRec").is_some() || v.get("price").is_some() || v.get("salt").is_some()
    })
    .ok_or_else(|| anyhow!("no decklist in deck page {urlhash} — EDHREC's page format may have changed"))?;
    Ok(DeckPage { deck: deck_from_object(&data, urlhash)?, source: data.to_string() })
}

/// Read a deck again from the deck object [`read_deck_page`] kept.
pub fn deck_from_source(source: &str, urlhash: &str) -> Result<Deck> {
    let data: Value = serde_json::from_str(source).with_context(|| format!("stored deck object for {urlhash}"))?;
    deck_from_object(&data, urlhash)
}

fn deck_from_object(data: &Value, urlhash: &str) -> Result<Deck> {
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

// --- themes and top commanders ---------------------------------------------------------------------

/// A commander as EDHREC lists it on theme and "top commanders" pages.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedCommander {
    pub name: String,
    /// EDHREC page slug (also the deck-table slug).
    pub slug: String,
    /// Decks with this commander (on a theme page: with this commander *and* theme).
    pub num_decks: u64,
}

fn listed_commanders(v: &Value, tags: &[&str]) -> Vec<ListedCommander> {
    let mut out: Vec<ListedCommander> = Vec::new();
    for list in v["container"]["json_dict"]["cardlists"].as_array().into_iter().flatten() {
        if !tags.is_empty() && !tags.contains(&list["tag"].as_str().unwrap_or("")) {
            continue;
        }
        for cv in list["cardviews"].as_array().into_iter().flatten() {
            let (Some(name), Some(slug)) = (cv["name"].as_str(), cv["sanitized"].as_str()) else { continue };
            if out.iter().any(|c| c.slug == slug) {
                continue;
            }
            out.push(ListedCommander {
                name: name.to_string(),
                slug: slug.to_string(),
                num_decks: cv["num_decks"].as_u64().unwrap_or(0),
            });
        }
    }
    out
}

/// The commanders EDHREC lists for a theme (`edhrec.com/tags/<theme>`): its top and newest ones.
pub fn theme_commanders(http: &Http, theme: &str) -> Result<Vec<ListedCommander>> {
    let url = format!("{EDHREC_JSON}/tags/{theme}.json");
    let v: Value = http.cached_json(&url, None, Ttl::For(TTL_COMMANDER_PAGE))?;
    Ok(parse_theme_commanders(&v))
}

pub fn parse_theme_commanders(v: &Value) -> Vec<ListedCommander> {
    listed_commanders(v, &["topcommanders", "newcommanders"])
}

/// One of EDHREC's deck themes (`edhrec.com/tags/<slug>`).
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// As EDHREC shows it ("Equipment", "+1/+1 Counters"); also what decks are tagged with.
    pub name: String,
    /// The page slug ("equipment", "plus-1-plus-1-counters").
    pub slug: String,
    pub num_decks: u64,
}

/// Every theme EDHREC has a page for (1 request, cached for a week).
pub fn themes(http: &Http) -> Result<Vec<Theme>> {
    let url = format!("{EDHREC_JSON}/tags.json");
    let v: Value = http.cached_json(&url, None, Ttl::For(TTL_COMMANDER_PAGE))?;
    Ok(parse_themes(&v))
}

pub fn parse_themes(v: &Value) -> Vec<Theme> {
    let mut out: Vec<Theme> = Vec::new();
    for list in v["container"]["json_dict"]["cardlists"].as_array().into_iter().flatten() {
        for cv in list["cardviews"].as_array().into_iter().flatten() {
            let (Some(name), Some(slug)) =
                (cv["name"].as_str(), cv["url"].as_str().and_then(|u| u.strip_prefix("/tags/")))
            else {
                continue;
            };
            if !out.iter().any(|t| t.slug == slug) {
                out.push(Theme {
                    name: name.to_string(),
                    slug: slug.to_string(),
                    num_decks: cv["num_decks"].as_u64().unwrap_or(0),
                });
            }
        }
    }
    out
}

/// EDHREC's most-built commanders of the past two years.
pub fn top_commanders(http: &Http) -> Result<Vec<ListedCommander>> {
    let url = format!("{EDHREC_JSON}/commanders/year.json");
    let v: Value = http.cached_json(&url, None, Ttl::For(TTL_COMMANDER_PAGE))?;
    Ok(listed_commanders(&v, &[]))
}

pub fn parse_top_commanders(v: &Value) -> Vec<ListedCommander> {
    listed_commanders(v, &[])
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
    fn a_deck_reads_the_same_from_its_kept_deck_object() {
        let page = read_deck_page(&read("deckpreview_jodah.html"), "8K2SZWMbz80NrhCJmt9DsA").unwrap();
        assert!(page.source.len() < 20_000, "{}", page.source.len());
        let again = deck_from_source(&page.source, "8K2SZWMbz80NrhCJmt9DsA").unwrap();
        assert_eq!((again.commanders, again.cards), (page.deck.commanders, page.deck.cards));
        assert_eq!(again.meta.tags, page.deck.meta.tags);
        assert_eq!(again.meta.salt, page.deck.meta.salt);
        assert!(!is_deck_id("../etc/passwd") && is_deck_id("8K2SZWMbz80NrhCJmt9DsA"));
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
    fn theme_and_top_commander_lists() {
        let v: Value = serde_json::from_str(&read("tag_tokens.json")).unwrap();
        let t = parse_theme_commanders(&v);
        assert_eq!(t.len(), 29); // 5 new + 24 top, no duplicates
        assert!(t.iter().any(|c| c.slug == "baylen-the-haymaker" && c.num_decks == 3605));
        let v: Value = serde_json::from_str(&read("commanders_year.json")).unwrap();
        let top = parse_top_commanders(&v);
        assert_eq!(top.len(), 40);
        assert_eq!(top[0].slug, "yshtola-nights-blessed");
    }

    #[test]
    fn theme_list() {
        let v: Value = serde_json::from_str(&read("tags.json")).unwrap();
        let t = parse_themes(&v);
        assert_eq!(t.len(), 22);
        assert_eq!(t[0], Theme { name: "Tokens".into(), slug: "tokens".into(), num_decks: 208811 });
        assert!(t.iter().any(|t| t.slug == "sagas" && t.name == "Sagas"));
    }

    #[test]
    fn salt_list() {
        let v: Value = serde_json::from_str(&read("top_salt.json")).unwrap();
        let s = parse_top_salt(&v);
        assert_eq!(s.len(), 100);
        assert!(s["Stasis"] > 3.0);
    }
}
