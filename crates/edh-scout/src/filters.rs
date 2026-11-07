//! Card-type filters (`-T planeswalkers=10+`) on EDHREC's per-deck type counts, and the EDHREC
//! themes they map to when searching across commanders.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use edh_data::edhrec::TableRow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeField {
    Creatures,
    Instants,
    Sorceries,
    /// Instants + sorceries.
    Spells,
    Artifacts,
    Enchantments,
    Planeswalkers,
    Battles,
    Lands,
}

impl TypeField {
    pub fn count(self, r: &TableRow) -> u32 {
        match self {
            TypeField::Creatures => r.creature,
            TypeField::Instants => r.instant,
            TypeField::Sorceries => r.sorcery,
            TypeField::Spells => r.instant + r.sorcery,
            TypeField::Artifacts => r.artifact,
            TypeField::Enchantments => r.enchantment,
            TypeField::Planeswalkers => r.planeswalker,
            TypeField::Battles => r.battle,
            TypeField::Lands => r.land,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TypeField::Creatures => "creatures",
            TypeField::Instants => "instants",
            TypeField::Sorceries => "sorceries",
            TypeField::Spells => "spells",
            TypeField::Artifacts => "artifacts",
            TypeField::Enchantments => "enchantments",
            TypeField::Planeswalkers => "planeswalkers",
            TypeField::Battles => "battles",
            TypeField::Lands => "lands",
        }
    }
}

impl FromStr for TypeField {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_lowercase().as_str() {
            "creature" | "creatures" => TypeField::Creatures,
            "instant" | "instants" => TypeField::Instants,
            "sorcery" | "sorceries" => TypeField::Sorceries,
            "spell" | "spells" => TypeField::Spells,
            "artifact" | "artifacts" => TypeField::Artifacts,
            "enchantment" | "enchantments" => TypeField::Enchantments,
            "planeswalker" | "planeswalkers" | "walkers" => TypeField::Planeswalkers,
            "battle" | "battles" => TypeField::Battles,
            "land" | "lands" => TypeField::Lands,
            other => bail!(
                "unknown card type '{other}' (use creatures, instants, sorceries, spells, artifacts, \
                 enchantments, planeswalkers, battles or lands)"
            ),
        })
    }
}

/// `planeswalkers=10+`, `creatures=..5`, `spells=25-40`, `lands=36`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeFilter {
    pub field: TypeField,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl TypeFilter {
    pub fn matches(&self, r: &TableRow) -> bool {
        let n = self.field.count(r);
        self.min.is_none_or(|m| n >= m) && self.max.is_none_or(|m| n <= m)
    }
}

impl FromStr for TypeFilter {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let (field, range) =
            s.split_once('=').ok_or_else(|| anyhow!("expected TYPE=RANGE like planeswalkers=10+, got '{s}'"))?;
        let field: TypeField = field.parse()?;
        let num = |x: &str| x.trim().parse::<u32>().map_err(|_| anyhow!("'{x}' in '{s}' is not a whole number"));
        let range = range.trim();
        let (min, max) = if let Some(lo) = range.strip_suffix('+') {
            (Some(num(lo)?), None)
        } else if let Some(hi) = range.strip_prefix("..") {
            (None, Some(num(hi)?))
        } else if let Some((lo, hi)) = range.split_once('-') {
            let (lo, hi) = (num(lo)?, num(hi)?);
            (Some(lo.min(hi)), Some(lo.max(hi)))
        } else {
            let n = num(range)?;
            (Some(n), Some(n))
        };
        Ok(TypeFilter { field, min, max })
    }
}

impl fmt::Display for TypeFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.field.name();
        match (self.min, self.max) {
            (Some(lo), None) => write!(f, "{lo}+ {name}"),
            (None, Some(hi)) => write!(f, "≤{hi} {name}"),
            (Some(lo), Some(hi)) if lo == hi => write!(f, "{lo} {name}"),
            (Some(lo), Some(hi)) => write!(f, "{lo}–{hi} {name}"),
            (None, None) => write!(f, "any {name}"),
        }
    }
}

pub fn describe(filters: &[TypeFilter]) -> String {
    filters.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

/// The EDHREC themes whose commanders are worth searching for these filters (empty: no theme
/// fits, use the most-built commanders instead).
pub fn themes(filters: &[TypeFilter]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for f in filters {
        let wants_many = f.min.is_some_and(|m| m > 0);
        let wants_few = f.max.is_some() && f.min.is_none_or(|m| m == 0);
        let theme = match f.field {
            TypeField::Planeswalkers if wants_many => Some("planeswalkers"),
            TypeField::Spells | TypeField::Instants | TypeField::Sorceries if wants_many => Some("spellslinger"),
            TypeField::Artifacts if wants_many => Some("artifacts"),
            TypeField::Enchantments if wants_many => Some("enchantress"),
            TypeField::Lands if wants_many => Some("lands-matter"),
            TypeField::Creatures if wants_few => Some("creatureless"),
            _ => None,
        };
        if let Some(t) = theme
            && !out.contains(&t)
        {
            out.push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(creature: u32, instant: u32, sorcery: u32, planeswalker: u32) -> TableRow {
        TableRow { creature, instant, sorcery, planeswalker, land: 36, ..Default::default() }
    }

    #[test]
    fn parsing_and_matching() {
        let f: TypeFilter = "planeswalkers=10+".parse().unwrap();
        assert_eq!((f.field, f.min, f.max), (TypeField::Planeswalkers, Some(10), None));
        assert!(f.matches(&row(20, 5, 5, 12)) && !f.matches(&row(20, 5, 5, 9)));
        let few: TypeFilter = "Creature=..5".parse().unwrap();
        assert!(few.matches(&row(3, 20, 20, 0)) && !few.matches(&row(6, 20, 20, 0)));
        let spells: TypeFilter = "spells=40-25".parse().unwrap();
        assert_eq!((spells.min, spells.max), (Some(25), Some(40)));
        assert!(spells.matches(&row(10, 15, 15, 0)));
        assert_eq!("lands=36".parse::<TypeFilter>().unwrap().to_string(), "36 lands");
        assert_eq!(few.to_string(), "≤5 creatures");
        assert!("goblins=5+".parse::<TypeFilter>().is_err());
        assert!("planeswalkers".parse::<TypeFilter>().is_err());
        assert!("planeswalkers=lots".parse::<TypeFilter>().is_err());
    }

    #[test]
    fn filters_pick_themes() {
        let p = |s: &[&str]| themes(&s.iter().map(|x| x.parse().unwrap()).collect::<Vec<_>>());
        assert_eq!(p(&["planeswalkers=10+"]), vec!["planeswalkers"]);
        assert_eq!(p(&["instants=15+", "sorceries=10+"]), vec!["spellslinger"]);
        assert_eq!(p(&["creatures=..5"]), vec!["creatureless"]);
        assert_eq!(p(&["creatures=40+"]), Vec::<&str>::new()); // no theme: most-built commanders
        assert_eq!(p(&["planeswalkers=8+", "spells=25+"]), vec!["planeswalkers", "spellslinger"]);
    }
}
