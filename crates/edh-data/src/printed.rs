//! Where each card has been printed, for the set keys of a search (`s:`, `in:`, `r:`, `year`, …):
//! one [`Printed`] per set, rarity and kind of printing, and the set list ([`SetInfo`]).
//!
//! A decklist names cards, not printings, so this keeps only what says something about the card:
//! which sets, at what rarity, when, whether as a reprint, whether as Universes Beyond, and in
//! which games. Art, frames, finishes and collector numbers are left out.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A printing's rarity, in Scryfall's order: special sits between rare and mythic (`r>=r` finds
/// specials, `r>m` only bonus cards).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
    Special,
    Mythic,
    Bonus,
}

impl Rarity {
    /// Scryfall's names, and their first letters.
    pub fn named(s: &str) -> Option<Rarity> {
        Some(match s.to_lowercase().as_str() {
            "c" | "common" => Rarity::Common,
            "u" | "uncommon" => Rarity::Uncommon,
            "r" | "rare" => Rarity::Rare,
            "s" | "special" => Rarity::Special,
            "m" | "mythic" => Rarity::Mythic,
            "b" | "bonus" => Rarity::Bonus,
            _ => return None,
        })
    }
}

/// A game a printing is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Paper = 1,
    Mtgo = 2,
    Arena = 4,
}

impl Game {
    pub fn named(s: &str) -> Option<Game> {
        Some(match s.to_lowercase().as_str() {
            "paper" => Game::Paper,
            "mtgo" => Game::Mtgo,
            "arena" => Game::Arena,
            _ => return None,
        })
    }
}

/// Printings of one card that look the same to a search: one set, one rarity, one date.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Printed {
    /// The set code, lowercase ("neo").
    #[serde(rename = "s")]
    pub set: String,
    #[serde(rename = "r", default)]
    pub rarity: Rarity,
    /// Release date as a number, 2015-08-18 being 20150818.
    #[serde(rename = "d", default)]
    pub released: u32,
    /// [`Printed::REPRINT`], [`Printed::UB`], [`Printed::SIDE_PRODUCT`] and the [`Game`]s, as bits.
    #[serde(rename = "f", default)]
    pub flags: u8,
}

impl Printed {
    /// A reprint, not the card's first printing.
    pub const REPRINT: u8 = 8;
    /// A Universes Beyond printing.
    pub const UB: u8 = 16;
    /// In a promo, Secret Lair, masterpiece, From the Vault or memorabilia set: `in:rare` leaves
    /// those out, as on Scryfall (`r:rare` doesn't).
    pub const SIDE_PRODUCT: u8 = 32;

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub fn in_game(&self, game: Game) -> bool {
        self.has(game as u8)
    }
}

/// One set, from Scryfall's set list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetInfo {
    pub code: String,
    #[serde(default)]
    pub name: String,
    /// "expansion", "commander", "masters", …
    #[serde(default)]
    pub set_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Release date as a number ([`Printed::released`]); 0 when unknown.
    #[serde(default)]
    pub released: u32,
}

/// The set list a search looks sets up in.
pub struct Sets<'a>(pub &'a [SetInfo]);

impl<'a> Sets<'a> {
    pub fn get(&self, code: &str) -> Option<&'a SetInfo> {
        let code = code.to_lowercase();
        self.0.iter().find(|s| s.code == code)
    }

    /// Does the list say what block and parent set each set has (it's read from Scryfall's set
    /// list, which an older download may not have)?
    pub fn knows_relations(&self) -> bool {
        self.0.iter().any(|s| s.block.is_some() || s.parent.is_some())
    }

    pub fn is_type(&self, set_type: &str) -> bool {
        self.0.iter().any(|s| s.set_type == set_type)
    }

    /// Codes of every set of this type.
    pub fn of_type(&self, set_type: &str) -> HashSet<String> {
        self.0.iter().filter(|s| s.set_type == set_type).map(|s| s.code.clone()).collect()
    }

    /// The sets in the same block as `code` (`b:wwk` is Zendikar block), or of the block with that
    /// code.
    pub fn block(&self, code: &str) -> HashSet<String> {
        let code = code.to_lowercase();
        let block = self.get(&code).and_then(|s| s.block.clone()).unwrap_or(code);
        self.0.iter().filter(|s| s.block.as_deref() == Some(block.as_str())).map(|s| s.code.clone()).collect()
    }

    /// The sets tied to `code`'s release: everything under the same top-level set (`g:fic` is
    /// FIN, FIC and their token, art and promo sets).
    pub fn group(&self, code: &str) -> HashSet<String> {
        let root = |code: &str| {
            let mut at = code.to_string();
            // Parents never loop, but don't trust that blindly.
            for _ in 0..8 {
                match self.get(&at).and_then(|s| s.parent.clone()) {
                    Some(p) => at = p,
                    None => break,
                }
            }
            at
        };
        let want = root(code);
        self.0.iter().filter(|s| root(&s.code) == want).map(|s| s.code.clone()).collect()
    }

    /// Set codes and names near `code`, for "did you mean".
    pub fn suggestions(&self, code: &str) -> Vec<String> {
        let needle = code.to_lowercase();
        let mut hits: Vec<&SetInfo> = self
            .0
            .iter()
            .filter(|s| (needle.len() >= 2 && s.code.starts_with(&needle)) || s.name.to_lowercase().contains(&needle))
            .collect();
        hits.sort_by_key(|s| (s.code.len(), s.code.clone()));
        hits.iter().take(6).map(|s| format!("{} ({})", s.code, s.name)).collect()
    }
}

/// "2015-08-18" as 20150818; "2015-08" and "2015" are that month's or year's first day.
pub fn parse_date(s: &str) -> Option<u32> {
    let mut parts = s.trim().splitn(3, '-');
    let year: u32 = parts.next()?.parse().ok().filter(|y| (1000..10000).contains(y))?;
    let month: u32 = parts.next().map_or(Some(1), |m| m.parse().ok().filter(|m| (1..=12).contains(m)))?;
    let day: u32 = parts.next().map_or(Some(1), |d| d.parse().ok().filter(|d| (1..=31).contains(d)))?;
    Some(year * 10000 + month * 100 + day)
}

/// Today, as [`parse_date`] numbers dates (UTC).
pub fn today() -> u32 {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year * 10000 + month * 100 + day) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(code: &str, block: Option<&str>, parent: Option<&str>) -> SetInfo {
        SetInfo {
            code: code.into(),
            name: code.to_uppercase(),
            set_type: "expansion".into(),
            block: block.map(Into::into),
            parent: parent.map(Into::into),
            released: 0,
        }
    }

    #[test]
    fn blocks_and_groups_gather_related_sets() {
        let list = vec![
            set("zen", Some("zen"), None),
            set("wwk", Some("zen"), None),
            set("roe", Some("zen"), None),
            set("fin", None, None),
            set("fic", None, Some("fin")),
            set("tfic", None, Some("fic")),
            set("neo", None, None),
        ];
        let sets = Sets(&list);
        let sorted = |s: HashSet<String>| {
            let mut v: Vec<String> = s.into_iter().collect();
            v.sort();
            v
        };
        assert_eq!(sorted(sets.block("wwk")), vec!["roe", "wwk", "zen"], "any set of the block names it");
        assert_eq!(sorted(sets.group("tfic")), vec!["fic", "fin", "tfic"], "up to the top set and back down");
        assert_eq!(sorted(sets.group("neo")), vec!["neo"]);
    }

    #[test]
    fn dates_read_as_numbers_that_sort() {
        assert_eq!(parse_date("2015-08-18"), Some(20150818));
        assert_eq!(parse_date("2015"), Some(20150101));
        assert_eq!(parse_date("2015-13-01"), None);
        assert_eq!(parse_date("ori"), None);
        assert!(today() > 20260000 && today() < 21000000, "{}", today());
        assert!(Rarity::Rare < Rarity::Special && Rarity::Special < Rarity::Mythic, "Scryfall's order");
    }
}
