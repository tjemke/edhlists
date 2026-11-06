//! The 0-10 power level.

use serde::{Deserialize, Serialize};

use crate::{
    CMC_CEILING, CMC_FLOOR, DeckCard, EFFICIENCY_LIMITS, FAVOR_PRICE, LAND, POP_CURVE, POWER_CURVE, PRICE_CURVE,
    RESERVED, curve, overrides,
};

const BASICS: &[&str] = &[
    "Mountain",
    "Forest",
    "Island",
    "Swamp",
    "Plains",
    "Wastes",
    "Snow-Covered Mountain",
    "Snow-Covered Forest",
    "Snow-Covered Island",
    "Snow-Covered Swamp",
    "Snow-Covered Plains",
    "Snow-Covered Wastes",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PowerLevel {
    /// The 0-10 number the site shows.
    pub level: f64,
    /// Impact scaled by efficiency; what the power curve is read against.
    pub score: f64,
    pub impact: f64,
    pub efficiency: f64,
    /// Average cmc of nonland cards, rounded like JavaScript's `toFixed(2)`.
    pub avg_cmc: f64,
    /// Mana value by which 65% of the deck's coloured impact has landed.
    pub curve_cmc: usize,
    pub lands: u32,
    pub nonlands: u32,
}

impl PowerLevel {
    pub fn label(&self) -> String {
        if self.level >= 10.0 { "10+".into() } else { format!("{:.1}", self.level) }
    }
}

/// JavaScript's `Number(x.toFixed(2))` for non-negative x: round half *up* on the exact binary value.
pub fn js_to_fixed2(x: f64) -> f64 {
    if !x.is_finite() || x < 0.0 {
        return x;
    }
    // 80 digits is enough to print any double >= 0.01 exactly.
    let exact = format!("{x:.80}");
    let (int, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut cents: u64 = int.parse::<u64>().unwrap_or(0) * 100 + frac[..2].parse::<u64>().unwrap_or(0);
    if frac.as_bytes().get(2).is_some_and(|&d| d >= b'5') {
        cents += 1;
    }
    format!("{}.{:02}", cents / 100, cents % 100).parse().unwrap_or(x)
}

fn bump(buckets: &mut Vec<f64>, index: usize, amount: f64) {
    if buckets.len() < index + 1 {
        buckets.resize(index + 1, 0.0);
    }
    buckets[index] += amount;
}

/// Score a deck the way edhpowerlevel.com does.
pub fn power_level(cards: &[DeckCard]) -> PowerLevel {
    let mut total_cmc = 0.0;
    let mut total_impact = 0.0;
    let (mut lands, mut nonlands) = (0u32, 0u32);
    let mut by_cmc: Vec<f64> = Vec::new();
    // R W G U B C, in the site's order
    let mut by_color: [Vec<f64>; 6] = Default::default();
    for card in cards {
        let qty = f64::from(card.quantity);
        let o = overrides(&card.name);
        let mut price = card.price;
        let mut cmc = card.cmc;
        if let Some(o) = o {
            if let Some(p) = o.price {
                price *= p;
            }
            if let Some(c) = o.cmc {
                cmc = c;
            }
        }
        if card.reserved {
            price *= RESERVED;
        }
        // A missing rank is the site's `false`: 27000 - false = 27000, which scores 0.
        let pop = POP_CURVE[10] - card.edhrec_rank.map(f64::from).unwrap_or(0.0);
        let mut impact =
            (curve(price, &PRICE_CURVE, 1.0 + FAVOR_PRICE) + curve(pop, &POP_CURVE, 1.0 - FAVOR_PRICE)) * qty;
        if let Some(o) = o {
            if let Some(m) = o.impact {
                impact *= m;
            }
            if let (Some(m), true) = (o.commander_impact, card.is_commander) {
                impact *= m;
            }
        }
        let is_land = card.is_land() || card.is_mdfc();
        if is_land {
            impact *= LAND;
            cmc = 0.0;
        }
        if BASICS.contains(&card.name.as_str()) {
            impact = 2.0 * qty;
        }
        if !card.is_mdfc() {
            total_cmc += cmc * qty;
        }
        total_impact += impact;
        if is_land {
            lands += card.quantity;
            bump(&mut by_cmc, 0, impact);
            continue;
        }
        nonlands += card.quantity;
        let idx = cmc.max(0.0) as usize;
        bump(&mut by_cmc, idx, impact);
        if let Some(colors) = &card.colors {
            for (slot, key) in ["R", "W", "G", "U", "B"].iter().enumerate() {
                if colors.iter().any(|c| c == key) {
                    bump(&mut by_color[slot], idx, impact / colors.len() as f64);
                }
            }
            if colors.is_empty() {
                bump(&mut by_color[5], idx, impact);
            }
        }
    }
    let avg_cmc = if nonlands > 0 { js_to_fixed2(total_cmc / f64::from(nonlands)) } else { 0.0 };
    let colored_total: f64 = by_color.iter().flatten().sum();
    let mut curve_cmc = 0;
    let mut running = 0.0;
    for z in 0..by_cmc.len() {
        running += by_color.iter().map(|c| c.get(z).copied().unwrap_or(0.0)).sum::<f64>();
        if running > colored_total * 0.65 {
            curve_cmc = z;
            break;
        }
    }
    // Efficiency rewards doing the work early: blend the average cost with the point where the
    // deck's impact arrives, then place that between the cmc floor and ceiling.
    let midpoint = (avg_cmc + curve_cmc as f64) / 2.0;
    let ratio = (CMC_CEILING - midpoint) / (CMC_CEILING - CMC_FLOOR);
    let (lo, hi) = EFFICIENCY_LIMITS;
    let efficiency = lo + (hi - lo) * ratio;
    let score = total_impact * efficiency;
    PowerLevel {
        level: curve(score, &POWER_CURVE, 1.0),
        score,
        impact: total_impact,
        efficiency,
        avg_cmc,
        curve_cmc,
        lands,
        nonlands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_rounds_half_up_like_javascript() {
        assert_eq!(js_to_fixed2(2.125), 2.13); // exact tie in binary: JS rounds up
        assert_eq!(js_to_fixed2(2.675), 2.67); // really 2.67499999... in binary
        assert_eq!(js_to_fixed2(3.0), 3.0);
        assert_eq!(js_to_fixed2(193.0 / 61.0), 3.16);
    }

    fn card(name: &str, cmc: f64, price: f64, rank: Option<u32>, colors: &[&str], type_line: &str) -> DeckCard {
        DeckCard {
            name: name.into(),
            quantity: 1,
            cmc,
            price,
            edhrec_rank: rank,
            colors: Some(colors.iter().map(|s| s.to_string()).collect()),
            layout: "normal".into(),
            type_line: type_line.into(),
            ..Default::default()
        }
    }

    #[test]
    fn basics_lands_and_overrides() {
        let mut forest = card("Forest", 0.0, 0.1, None, &["G"], "Basic Land — Forest");
        forest.quantity = 10;
        let sol = card("Sol Ring", 1.0, 1.5, Some(1), &[], "Artifact");
        let pl = power_level(&[forest, sol]);
        assert_eq!((pl.lands, pl.nonlands), (10, 1));
        // basics are a flat 2 impact each; Sol Ring's price is multiplied by 8 (12 -> curve 6.4*1.25)
        let sol_impact = curve(12.0, &PRICE_CURVE, 1.25) + curve(26999.0, &POP_CURVE, 0.75);
        assert!((pl.impact - (20.0 + sol_impact)).abs() < 1e-9);
        assert_eq!(pl.avg_cmc, 1.0);
    }
}
