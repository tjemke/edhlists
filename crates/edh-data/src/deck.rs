//! The deck model and decklist text parsing.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeckMeta {
    pub edhrec_url: Option<String>,
    /// Where the list actually lives (Moxfield, Archidekt, ...).
    pub source_url: Option<String>,
    pub savedate: Option<String>,
    pub price: Option<f64>,
    /// EDHREC's deck salt: the sum of its cards' salt scores.
    pub salt: Option<f64>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The bracket the deck's owner declared on EDHREC, if any.
    pub declared_bracket: Option<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Deck {
    /// EDHREC urlhash, or the file stem for local lists.
    pub id: String,
    pub commanders: Vec<String>,
    /// The 99 (commanders excluded), by the names the source used.
    pub cards: BTreeMap<String, u32>,
    pub meta: DeckMeta,
}

impl Deck {
    pub fn card_count(&self) -> u32 {
        self.cards.values().sum::<u32>() + self.commanders.len() as u32
    }

    /// Every entry: (name, quantity, is_commander), commanders first.
    pub fn entries(&self) -> impl Iterator<Item = (&str, u32, bool)> {
        self.commanders
            .iter()
            .map(|c| (c.as_str(), 1, true))
            .chain(self.cards.iter().map(|(n, q)| (n.as_str(), *q, false)))
    }

    /// "Commander / Deck" sectioned text, the format EDHPowerLevel's link and most sites accept.
    pub fn sectioned_lines(&self) -> Vec<String> {
        let mut out = vec!["Commander".to_string()];
        out.extend(self.commanders.iter().map(|c| format!("1 {c}")));
        out.push(String::new());
        out.push("Deck".into());
        out.extend(self.cards.iter().map(|(n, q)| format!("{q} {n}")));
        out
    }
}

static DECKLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:(\d+)\s*[xX]?\s+)?(.+?)",
        r"(?:\s+\([A-Za-z0-9]{2,6}\)(?:\s+[\w★-]+)?)?", // (SET) 123
        r"(?:\s+\[[^\]]*\])*",                          // [Archidekt categories]
        r"(?:\s+\*[A-Za-z]+\*)*\s*$",                   // *F* / *CMDR*
    ))
    .unwrap()
});

const SECTIONS: &[&str] =
    &["commander", "commanders", "deck", "main", "mainboard", "sideboard", "maybeboard", "companion"];

/// Parse plain `1 Sol Ring` lists, MTGA/Moxfield exports with `(SET) 123`, Archidekt `[categories]`,
/// `*CMDR*` markers and `Commander` / `Deck` / `Sideboard` section headers.
pub fn parse_decklist(text: &str, commanders: &[String]) -> Deck {
    let mut cmdrs: Vec<String> = commanders.to_vec();
    let mut cards: BTreeMap<String, u32> = BTreeMap::new();
    let mut section: Option<String> = None;
    for line in text.lines() {
        let s = line.trim();
        if s.is_empty() || s.starts_with("//") || s.starts_with('#') {
            continue;
        }
        let header = s.trim_end_matches(':').to_lowercase();
        if SECTIONS.contains(&header.as_str()) {
            section = Some(header);
            continue;
        }
        if matches!(section.as_deref(), Some("sideboard" | "maybeboard")) {
            continue;
        }
        let Some(m) = DECKLINE.captures(s) else { continue };
        let qty: u32 = m.get(1).and_then(|q| q.as_str().parse().ok()).unwrap_or(1);
        let name = m[2].trim().to_string();
        let is_cmdr = matches!(section.as_deref(), Some("commander" | "commanders"))
            || s.contains("*CMDR*")
            || s.contains("[Commander");
        if is_cmdr {
            if !cmdrs.contains(&name) {
                cmdrs.push(name);
            }
        } else {
            *cards.entry(name).or_default() += qty;
        }
    }
    for c in &cmdrs {
        cards.remove(c);
    }
    Deck { id: "deck".into(), commanders: cmdrs, cards, meta: DeckMeta::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(d: &Deck) -> Vec<(&str, u32)> {
        d.cards.iter().map(|(n, q)| (n.as_str(), *q)).collect()
    }

    #[test]
    fn sections_assign_commanders() {
        let d = parse_decklist("Commander\n1 Atraxa, Praetors' Voice\n\nDeck\n1 Sol Ring\n", &[]);
        assert_eq!(d.commanders, vec!["Atraxa, Praetors' Voice"]);
        assert_eq!(names(&d), vec![("Sol Ring", 1)]);
    }

    #[test]
    fn markers_categories_and_set_codes() {
        let d = parse_decklist("1 Krenko, Mob Boss *CMDR*\n1 Sol Ring (LEA) 123\n1 Counterspell (7ED) 67★\n", &[]);
        assert_eq!(d.commanders, vec!["Krenko, Mob Boss"]);
        assert_eq!(names(&d), vec![("Counterspell", 1), ("Sol Ring", 1)]);
        let d = parse_decklist("1 Krenko, Mob Boss [Commander{top}]\n1 Sol Ring [Ramp] *F*\n", &[]);
        assert_eq!(d.commanders, vec!["Krenko, Mob Boss"]);
        assert_eq!(names(&d), vec![("Sol Ring", 1)]);
    }

    #[test]
    fn quantities_duplicates_and_boards() {
        let d = parse_decklist(
            "// comment\n# another\nDeck:\n4 Forest\n2x Island\nSwamp\n1 Forest\n\nSideboard\n1 Ponder\nMaybeboard\n1 Brainstorm\n",
            &[],
        );
        assert_eq!(names(&d), vec![("Forest", 5), ("Island", 2), ("Swamp", 1)]);
    }

    #[test]
    fn commanders_never_in_the_99_and_explicit_ones_win() {
        let d = parse_decklist("Commander\n1 Krenko, Mob Boss\nDeck\n1 Krenko, Mob Boss\n1 Sol Ring\n", &[]);
        assert_eq!(names(&d), vec![("Sol Ring", 1)]);
        let d = parse_decklist("Deck\n1 Sol Ring\n1 Krenko, Mob Boss\n", &["Krenko, Mob Boss".into()]);
        assert_eq!(d.commanders, vec!["Krenko, Mob Boss"]);
        assert_eq!(d.card_count(), 2);
    }

    #[test]
    fn sectioned_lines_round_trip() {
        let d = parse_decklist(
            "Commander\n1 Thrasios, Triton Hero\n1 Tymna the Weaver\n\nDeck\n2 Forest\n1 Sol Ring\n",
            &[],
        );
        let again = parse_decklist(&d.sectioned_lines().join("\n"), &[]);
        assert_eq!(again.commanders, d.commanders);
        assert_eq!(again.cards, d.cards);
    }
}
