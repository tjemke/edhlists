//! Colours, kinds of mana, and mana costs.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::flags::{EnumSet, Flag, flags};

flags! {
    /// One of Magic's five colours. Colourless isn't one.
    pub enum Color { W = "W", U = "U", B = "B", R = "R", G = "G" }
}

/// A card's colours or colour identity; reads in WUBRG order ("WUG").
pub type ColorSet = EnumSet<Color>;

impl ColorSet {
    /// The colours named by their letters, in any case and order ("rg", "WUBRG"); `None` if
    /// anything else is in there.
    pub fn from_letters(letters: &str) -> Option<Self> {
        letters.chars().map(|c| Color::from_code(&c.to_ascii_uppercase().to_string())).collect()
    }
}

flags! {
    /// A kind of mana: one of the colours, or colourless ({C}).
    pub enum Mana { W = "W", U = "U", B = "B", R = "R", G = "G", C = "C" }
}

/// The kinds of mana a card can make (Command Tower: WUBRG in a five-colour deck, Sol Ring: C).
pub type ManaSet = EnumSet<Mana>;

impl Mana {
    /// The colour, or `None` for colourless.
    pub fn color(self) -> Option<Color> {
        match self {
            Mana::W => Some(Color::W),
            Mana::U => Some(Color::U),
            Mana::B => Some(Color::B),
            Mana::R => Some(Color::R),
            Mana::G => Some(Color::G),
            Mana::C => None,
        }
    }
}

impl From<Color> for Mana {
    fn from(c: Color) -> Self {
        match c {
            Color::W => Mana::W,
            Color::U => Mana::U,
            Color::B => Mana::B,
            Color::R => Mana::R,
            Color::G => Mana::G,
        }
    }
}

/// One mana symbol of a cost, other than generic mana (which [`ManaCost`] counts as a number).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pip {
    /// {W} … {G}, or {C}: exactly that kind of mana.
    One(Mana),
    /// {W/U}: either kind ({C/W} exists too).
    Hybrid(Mana, Mana),
    /// {2/W}: the colour, or two of any kind.
    TwoOr(Color),
    /// {W/P}: the colour, or 2 life.
    Phyrexian(Color),
    /// {W/U/P}: either colour, or 2 life.
    PhyrexianHybrid(Color, Color),
    /// {S}: one mana from a snow source.
    Snow,
    /// {X}, {Y} or {Z}: an amount chosen as the spell is cast; 0 everywhere else.
    X,
}

impl Pip {
    /// What the symbol adds to the mana value.
    pub fn mana_value(self) -> u32 {
        match self {
            Pip::TwoOr(_) => 2,
            Pip::X => 0,
            _ => 1,
        }
    }

    fn parse(symbol: &str) -> Option<Pip> {
        let mana = |s: &str| Mana::from_code(s);
        let color = |s: &str| Color::from_code(s);
        let parts: Vec<&str> = symbol.split('/').collect();
        Some(match parts.as_slice() {
            ["X" | "Y" | "Z"] => Pip::X,
            ["S"] => Pip::Snow,
            [m] => Pip::One(mana(m)?),
            ["2", c] => Pip::TwoOr(color(c)?),
            [c, "P"] => Pip::Phyrexian(color(c)?),
            [a, b] => Pip::Hybrid(mana(a)?, mana(b)?),
            [a, b, "P"] => Pip::PhyrexianHybrid(color(a)?, color(b)?),
            _ => return None,
        })
    }
}

impl fmt::Display for Pip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pip::One(m) => write!(f, "{{{m}}}"),
            Pip::Hybrid(a, b) => write!(f, "{{{a}/{b}}}"),
            Pip::TwoOr(c) => write!(f, "{{2/{c}}}"),
            Pip::Phyrexian(c) => write!(f, "{{{c}/P}}"),
            Pip::PhyrexianHybrid(a, b) => write!(f, "{{{a}/{b}/P}}"),
            Pip::Snow => f.write_str("{S}"),
            Pip::X => f.write_str("{X}"),
        }
    }
}

/// A mana cost: generic mana as a number, every other symbol as a [`Pip`], in printed order.
/// Reads and prints as Scryfall writes it ("{X}{2}{G}{G}"); stored that way too.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ManaCost {
    pub generic: u32,
    pub pips: Vec<Pip>,
}

impl ManaCost {
    pub fn mana_value(&self) -> u32 {
        self.generic + self.pips.iter().map(|p| p.mana_value()).sum::<u32>()
    }
}

/// Scryfall's order: X first, then generic, then the rest as printed. A cost of nothing but
/// generic 0 is "{0}".
impl fmt::Display for ManaCost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (xs, rest): (Vec<&Pip>, Vec<&Pip>) = self.pips.iter().partition(|p| **p == Pip::X);
        xs.iter().try_for_each(|p| write!(f, "{p}"))?;
        if self.generic > 0 || self.pips.is_empty() {
            write!(f, "{{{}}}", self.generic)?;
        }
        rest.iter().try_for_each(|p| write!(f, "{p}"))
    }
}

/// Parses Scryfall's notation ("{2}{G}{U}"). An empty string is an error: a card without a mana
/// cost has no `ManaCost` at all, which isn't the same as {0}. So are symbols outside ordinary
/// play (half mana, {∞}).
impl FromStr for ManaCost {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut cost = ManaCost::default();
        let mut rest = s.trim();
        if rest.is_empty() {
            bail!("an empty mana cost");
        }
        while !rest.is_empty() {
            let inner = rest.strip_prefix('{').and_then(|r| r.split_once('}'));
            let Some((symbol, after)) = inner else { bail!("'{s}' isn't a mana cost like {{2}}{{G}}") };
            if let Ok(n) = symbol.parse::<u32>() {
                cost.generic += n;
            } else {
                cost.pips.push(Pip::parse(symbol).ok_or_else(|| anyhow!("unknown mana symbol {{{symbol}}} in '{s}'"))?);
            }
            rest = after;
        }
        Ok(cost)
    }
}

impl Serialize for ManaCost {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ManaCost {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = std::borrow::Cow::<'de, str>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_in_wubrg_order() {
        assert_eq!(ColorSet::from_codes(["G", "W", "U"]).to_string(), "WUG");
        assert_eq!(ColorSet::all().to_string(), "WUBRG");
        assert_eq!(ColorSet::from_codes(["C"]), ColorSet::empty(), "colourless isn't a colour");
        assert_eq!(ColorSet::from_letters("gw"), Some(ColorSet::from_codes(["W", "G"])));
        assert_eq!(ColorSet::from_letters("gc"), None, "c isn't a colour letter");
    }

    #[test]
    fn colourless_mana_has_no_colour() {
        assert_eq!(Mana::C.color(), None);
        assert_eq!(Mana::from(Color::G).color(), Some(Color::G));
        assert_eq!(ManaSet::from_codes(["C", "G"]).to_string(), "GC");
    }

    #[test]
    fn costs_read_and_print_as_scryfall_writes_them() {
        for s in [
            "{2}{G}{U}",
            "{X}{X}{G}",
            "{X}{2}{R}",
            "{0}",
            "{W/U}{W/U}",
            "{2/G}{2/G}{2/G}",
            "{G/P}",
            "{G/U/P}",
            "{C}{C}",
            "{C/W}",
            "{S}{S}",
            "{10}",
        ] {
            assert_eq!(s.parse::<ManaCost>().unwrap().to_string(), s, "{s} should print the way it reads");
        }
        let fireball: ManaCost = "{X}{R}".parse().unwrap();
        assert_eq!(fireball.pips, vec![Pip::X, Pip::One(Mana::R)]);
        assert_eq!(fireball.generic, 0);
    }

    #[test]
    fn mana_value_counts_twobrid_as_two_and_x_as_nothing() {
        let mv = |s: &str| s.parse::<ManaCost>().unwrap().mana_value();
        assert_eq!(mv("{2}{G}{U}"), 4);
        assert_eq!(mv("{X}{X}{G}"), 1);
        assert_eq!(mv("{2/G}{2/G}{2/G}"), 6, "Spectral Procession");
        assert_eq!(mv("{G/P}"), 1);
        assert_eq!(mv("{0}"), 0);
    }

    #[test]
    fn no_cost_and_odd_symbols_are_errors() {
        assert!("".parse::<ManaCost>().is_err(), "no cost isn't {{0}}");
        assert!("{HW}".parse::<ManaCost>().is_err());
        assert!("2G".parse::<ManaCost>().is_err());
        assert!(serde_json::from_str::<ManaCost>(r#""{1}{Q}""#).is_err());
        assert_eq!(serde_json::to_string(&"{1}{G}".parse::<ManaCost>().unwrap()).unwrap(), r#""{1}{G}""#);
    }
}
