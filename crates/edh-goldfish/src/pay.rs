//! Paying a mana cost from the mana a deck has: units that each make one of some kinds.

use edh_data::mana::{Mana, ManaCost, ManaSet, Pip};

use crate::profile::Unit;

/// What a pip will take: one unit of these kinds, or (if none is left) `fallback` generic mana
/// or life.
struct Need {
    kinds: ManaSet,
    /// Generic mana it can be paid with instead ({2/W}), or `None` if it must be these kinds.
    /// Phyrexian pips can always be paid with life, which costs no mana: `Some(0)`.
    fallback: Option<u32>,
}

/// Pay `cost` (X as 0) plus `extra` generic from `units`: the units left, or `None` if it can't
/// be paid. Coloured pips go first, the hardest to pay first, each from the least flexible unit
/// that can pay it; generic mana then takes the least flexible units left.
pub fn pay(units: &[Unit], cost: &ManaCost, extra: u32) -> Option<Vec<Unit>> {
    let mut generic = cost.generic + extra;
    let mut needs = Vec::with_capacity(cost.pips.len());
    for pip in &cost.pips {
        let one = |m: Mana| ManaSet::of(m);
        let need = match *pip {
            Pip::One(m) => Need { kinds: one(m), fallback: None },
            Pip::Hybrid(a, b) => Need { kinds: one(a) | one(b), fallback: None },
            Pip::TwoOr(c) => Need { kinds: one(c.into()), fallback: Some(2) },
            Pip::Phyrexian(c) => Need { kinds: one(c.into()), fallback: Some(0) },
            Pip::PhyrexianHybrid(a, b) => Need { kinds: one(a.into()) | one(b.into()), fallback: Some(0) },
            Pip::Snow => {
                generic += 1;
                continue;
            }
            Pip::X => continue,
        };
        needs.push(need);
    }
    if (needs.iter().filter(|n| n.fallback.is_none()).count() as u32 + generic) as usize > units.len() {
        return None;
    }
    let mut left: Vec<Unit> = units.to_vec();
    left.sort_by_key(|u| u.len());
    let can_pay = |left: &[Unit], n: &Need| left.iter().filter(|u| u.intersects(n.kinds)).count();
    needs.sort_by_key(|n| (n.fallback.is_some(), can_pay(&left, n)));
    for need in &needs {
        match left.iter().position(|u| u.intersects(need.kinds)) {
            Some(i) => {
                left.remove(i);
            }
            None => generic += need.fallback?,
        }
    }
    if generic as usize > left.len() {
        return None;
    }
    Some(left.split_off(generic as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(codes: &[&str]) -> Vec<Unit> {
        codes.iter().map(|c| ManaSet::from_codes(c.split("").filter(|s| !s.is_empty()))).collect()
    }
    fn cost(s: &str) -> ManaCost {
        s.parse().unwrap()
    }

    #[test]
    fn coloured_pips_need_a_source_of_their_colour() {
        assert!(pay(&units(&["G", "G", "C"]), &cost("{1}{G}{U}"), 0).is_none(), "no blue source");
        let left = pay(&units(&["G", "U", "C", "C"]), &cost("{1}{G}{U}"), 0).unwrap();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn the_least_flexible_source_pays_first() {
        // Paying {G} with the five-colour unit would leave nothing for {U}.
        assert!(pay(&units(&["WUBRG", "G"]), &cost("{G}{U}"), 0).is_some());
        // Generic mana takes the colourless unit and keeps the flexible one.
        assert_eq!(pay(&units(&["C", "WUBRG"]), &cost("{1}"), 0).unwrap(), units(&["WUBRG"]));
    }

    #[test]
    fn colourless_pips_need_colourless_mana() {
        assert!(pay(&units(&["G", "G"]), &cost("{C}"), 0).is_none());
        assert!(pay(&units(&["C"]), &cost("{C}"), 0).is_some());
    }

    #[test]
    fn hybrid_twobrid_and_phyrexian_pips_have_their_ways_out() {
        assert!(pay(&units(&["U"]), &cost("{G/U}"), 0).is_some());
        assert!(pay(&units(&["C", "C"]), &cost("{2/G}"), 0).is_some(), "two generic for {{2/G}}");
        assert!(pay(&units(&["C"]), &cost("{2/G}"), 0).is_none());
        assert!(pay(&units(&[]), &cost("{G/P}"), 0).is_some(), "2 life for {{G/P}}");
    }

    #[test]
    fn x_costs_nothing_here_and_extra_mana_is_generic() {
        assert_eq!(pay(&units(&["R"]), &cost("{X}{R}"), 0).unwrap().len(), 0);
        assert!(pay(&units(&["C"]), &cost("{1}"), 1).is_none(), "the activation's {{1}} on top");
    }
}
