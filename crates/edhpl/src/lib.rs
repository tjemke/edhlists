//! EDHPowerLevel's Commander power level and bracket, computed locally.
//!
//! [edhpowerlevel.com](https://edhpowerlevel.com) has no API, but its scoring is plain client-side
//! JavaScript. This crate ports that code (bundle `main-C1lbqCDd.js`, checked 2026-11-06).
//! `reference/edhpl_reference.js` holds the same logic transcribed from the bundle, and the golden
//! files in `tests/data/` tie the two together — see this crate's README.
//!
//! * [`power_level`]: each card's impact comes from its price and EDHREC popularity rank; the total
//!   is scaled by an efficiency factor from the mana curve and read off a power curve (0-10).
//! * [`bracket`]: the larger of a rules floor (extra turns, land denial, Game Changers, early and
//!   late 2-card combos) and the bracket the power level maps to via the site's `bracketCurve`.
//! * [`edhpowerlevel_url`]: a link that opens a list on the site, prefilled.
//!
//! Inputs use the site's own data shapes ([`DeckCard`], [`Combo`]), so the JavaScript reference and
//! the Rust tests can read the very same fixture. Quirks of the original are reproduced on purpose
//! and marked "quirk".

mod bracket;
mod link;
mod power;

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

pub use bracket::{BracketReport, ComboHit, bracket, classify_combos};
pub use link::edhpowerlevel_url;
pub use power::{PowerLevel, js_to_fixed2, power_level};

pub const LAND: f64 = 0.6;
pub const RESERVED: f64 = 0.2;
pub const FAVOR_PRICE: f64 = 0.25;
pub const CMC_FLOOR: f64 = 1.75;
pub const CMC_CEILING: f64 = 6.0;
pub const EFFICIENCY_LIMITS: (f64, f64) = (0.65, 1.1);
pub const PRICE_CURVE: [f64; 11] = [0.0, 0.5, 1.5, 3.5, 6.0, 10.0, 15.0, 25.0, 40.0, 65.0, 100.0];
pub const POP_CURVE: [f64; 11] =
    [0.0, 8500.0, 13600.0, 17100.0, 19800.0, 21900.0, 23700.0, 25300.0, 26200.0, 26700.0, 27000.0];
pub const POWER_CURVE: [f64; 11] = [0.0, 250.0, 320.0, 350.0, 380.0, 420.0, 470.0, 560.0, 760.0, 890.0, 1000.0];
/// Power level -> bracket: `ceil(curve(level, BRACKET_CURVE))`. B2 is (4.7, 6.7], B3 is (6.7, 7.7].
pub const BRACKET_CURVE: [f64; 6] = [0.0, 4.7, 6.7, 7.7, 9.25, 10.0];

/// One deck entry, shaped like the card objects edhpowerlevel.com builds internally.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DeckCard {
    pub name: String,
    pub quantity: u32,
    #[serde(rename = "isCommander", default)]
    pub is_commander: bool,
    pub cmc: f64,
    /// Market price in USD.
    pub price: f64,
    /// `None` means "no rank" (the site's `false`), which scores no popularity.
    pub edhrec_rank: Option<u32>,
    #[serde(default)]
    pub reserved: bool,
    /// `None` means the card has no colors at all; the site treats that differently from `[]`.
    pub colors: Option<Vec<String>>,
    pub layout: String,
    pub type_line: String,
    /// Oracle text per face.
    #[serde(default)]
    pub oracle_text: Vec<String>,
    #[serde(rename = "gamechanger", default)]
    pub game_changer: bool,
}

impl DeckCard {
    /// The front face's type ("Legendary Creature — Elf // ..." -> "Legendary Creature").
    pub fn front_type(&self) -> &str {
        let front = self.type_line.split(" // ").next().unwrap_or("");
        front.split(" — ").next().unwrap_or(front)
    }
    pub fn is_land(&self) -> bool {
        self.front_type().contains("Land")
    }
    pub fn is_mdfc(&self) -> bool {
        self.layout == "modal_dfc"
    }
    /// The cmc the site works with: after its overrides, and 0 for lands and MDFCs.
    pub fn effective_cmc(&self) -> f64 {
        if self.is_land() || self.is_mdfc() {
            return 0.0;
        }
        overrides(&self.name).and_then(|o| o.cmc).unwrap_or(self.cmc)
    }
}

/// A Commander Spellbook combo, shaped like the site's mapped combo objects (`rl()` in the bundle).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Combo {
    pub id: String,
    /// Spellbook's `manaValueNeeded`.
    pub mv: f64,
    pub cards: Vec<ComboCard>,
    pub produces: Vec<Feature>,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    #[serde(rename = "notablePrerequisites", default)]
    pub notable_prerequisites: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ComboCard {
    pub name: String,
    /// Spellbook zone letters, e.g. `["B"]` for battlefield.
    pub zone: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    pub id: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Requirement {
    pub template: Template,
    pub quantity: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Template {
    pub id: u32,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Override {
    pub price: Option<f64>,
    pub cmc: Option<f64>,
    pub impact: Option<f64>,
    #[serde(rename = "commanderImpact")]
    pub commander_impact: Option<f64>,
}

/// The site's per-card override table, extracted from the bundle.
static OVERRIDES: LazyLock<HashMap<String, Override>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../data/overrides.json")).expect("valid overrides json"));

/// Exact-name lookup, as the site does (`ne(Ba, "name", U.name)`).
pub(crate) fn overrides(name: &str) -> Option<&'static Override> {
    OVERRIDES.get(name)
}

/// Piecewise-linear position of `value` on `curve`. Quirks: only the stop index is scaled by
/// `step`, not the interpolated fraction; a value exactly on the last stop returns 0.
pub fn curve(value: f64, curve: &[f64], step: f64) -> f64 {
    if value <= curve[0] {
        return 0.0;
    }
    let last = curve.len() - 1;
    if value > curve[last] {
        return last as f64 * step;
    }
    for p in 0..last {
        if value < curve[p + 1] && value >= curve[p] {
            return p as f64 * step + (value - curve[p]) / (curve[p + 1] - curve[p]);
        }
    }
    0.0
}

/// The bracket a power level maps to on its own (the site's `X`).
pub fn bracket_from_level(level: f64) -> u8 {
    if level > 0.0 { curve(level, &BRACKET_CURVE, 1.0).ceil() as u8 } else { 5 }
}

/// The power-level interval `(lo, hi]` that maps to brackets `from..=to`.
pub fn level_band(from: u8, to: u8) -> (f64, f64) {
    let from = from.clamp(1, 5) as usize;
    let to = to.clamp(1, 5) as usize;
    let lo = BRACKET_CURVE[from - 1];
    let hi = if to >= 5 { f64::INFINITY } else { BRACKET_CURVE[to] };
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_to_bracket() {
        assert_eq!(bracket_from_level(4.7), 1);
        assert_eq!(bracket_from_level(4.71), 2);
        assert_eq!(bracket_from_level(6.7), 2);
        assert_eq!(bracket_from_level(7.5), 3);
        assert_eq!(bracket_from_level(8.0), 4);
        assert_eq!(bracket_from_level(9.5), 5);
        assert_eq!(bracket_from_level(0.0), 5); // quirk: no score -> the site assumes the worst
        assert_eq!(bracket_from_level(10.0), 0); // quirk: exactly on the last stop
        assert_eq!(level_band(2, 3), (4.7, 7.7));
    }

    #[test]
    fn overrides_load() {
        assert_eq!(overrides("Sol Ring").and_then(|o| o.price), Some(8.0));
        assert_eq!(overrides("Fierce Guardianship").and_then(|o| o.cmc), Some(0.0));
        assert!(overrides("Grizzly Bears").is_none());
    }
}
