//! Choosing which decks to fetch, using only the deck table (zero extra requests).
//!
//! EDHREC lists tens of thousands of decks per popular commander and gives no quality signal, so
//! we spend the fetch budget carefully:
//! 1. drop incomplete lists and anything the owner declared outside the target brackets;
//! 2. calibrate a salt ceiling (and, with `--price`, a price ceiling) from the decks whose owners
//!    *did* declare a target bracket (their 90th percentile), and drop decks above it — they are
//!    very likely stronger or meaner than wanted, whatever their label says;
//! 3. drop exact duplicates (re-uploads of the same list);
//! 4. include every already-cached deck for free, then spread the remaining budget across themes
//!    and salt/price tiers so the pool isn't fifty copies of the same idea.
//!
//! `-T` card filters can't be checked before a deck is fetched, but their [`Prior`]s drop decks
//! that can't reach the minimum and, within each theme and tier, fetch the likeliest decks first.
//!
//! The decks past the budget come out in the same order (any prefix of `to_fetch` + `more` is
//! spread the same way), so a browser left open can keep fetching where the budget stopped.

use std::collections::{BTreeMap, HashSet};

use edh_data::edhrec::TableRow;

use crate::filters::{Prior, TypeFilter};

#[derive(Clone, Debug)]
pub struct SelectOptions {
    pub brackets: (u8, u8),
    pub declared_only: bool,
    pub max_price: Option<f64>,
    /// Also cap price at the declared decks' 90th percentile (off unless asked for: a deck's price
    /// is often one or two cards, which says little about how it plays).
    pub price_ceiling: bool,
    /// Skip decks whose EDHREC salt is above this.
    pub max_salt: Option<f64>,
    /// Keep only decks with one of these EDHREC tags (case-insensitive).
    pub tags: Vec<String>,
    /// Keep only decks saved on/after this date (YYYY-MM-DD).
    pub since: Option<String>,
    /// Card-type count filters (`-T planeswalkers=10+`); all must match.
    pub types: Vec<TypeFilter>,
    /// What the deck table says about the `-T` card filters' minimums.
    pub priors: Vec<Prior>,
    /// How many not-yet-cached decks we may fetch.
    pub budget: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ceiling {
    pub salt: f64,
    pub price: f64,
    pub from_decks: usize,
}

#[derive(Debug, Default)]
pub struct Selection<'a> {
    pub cached: Vec<&'a TableRow>,
    pub to_fetch: Vec<&'a TableRow>,
    /// The other eligible decks that aren't stored yet, continuing `to_fetch`'s order.
    pub more: Vec<&'a TableRow>,
    /// Decks that passed the filters (cached or not).
    pub eligible: usize,
    pub total: usize,
    pub ceiling: Option<Ceiling>,
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::INFINITY;
    }
    let pos = q * (sorted.len() - 1) as f64;
    let (i, frac) = (pos.floor() as usize, pos.fract());
    if i + 1 < sorted.len() { sorted[i] + frac * (sorted[i + 1] - sorted[i]) } else { sorted[i] }
}

/// The 90th percentile of salt and price among decks declared within the target brackets.
pub fn calibrate(rows: &[TableRow], brackets: (u8, u8)) -> Option<Ceiling> {
    let declared: Vec<&TableRow> =
        rows.iter().filter(|r| r.bracket.is_some_and(|b| (brackets.0..=brackets.1).contains(&b))).collect();
    if declared.len() < 20 {
        return None;
    }
    let mut salts: Vec<f64> = declared.iter().map(|r| r.salt).collect();
    let mut prices: Vec<f64> = declared.iter().map(|r| r.price).collect();
    salts.sort_by(f64::total_cmp);
    prices.sort_by(f64::total_cmp);
    Some(Ceiling { salt: quantile(&salts, 0.9), price: quantile(&prices, 0.9), from_decks: declared.len() })
}

fn tercile(sorted: &[f64], x: f64) -> u8 {
    if x <= quantile(sorted, 1.0 / 3.0) {
        0
    } else if x <= quantile(sorted, 2.0 / 3.0) {
        1
    } else {
        2
    }
}

pub fn select<'a>(rows: &'a [TableRow], opts: &SelectOptions, is_cached: impl Fn(&str) -> bool) -> Selection<'a> {
    let in_target = |b: u8| (opts.brackets.0..=opts.brackets.1).contains(&b);
    let ceiling = calibrate(rows, opts.brackets);
    let tags: Vec<String> = opts.tags.iter().map(|t| t.to_lowercase()).collect();
    let mut seen_signatures = HashSet::new();
    let eligible: Vec<&TableRow> = rows
        .iter()
        .filter(|r| !r.urlhash.is_empty())
        .filter(|r| (98..=101).contains(&r.card_count()) && (28..=44).contains(&r.land))
        .filter(|r| match r.bracket {
            Some(b) => in_target(b),
            None => !opts.declared_only,
        })
        .filter(|r| opts.max_price.is_none_or(|m| r.price <= m))
        .filter(|r| opts.max_salt.is_none_or(|m| r.salt <= m))
        .filter(|r| opts.since.as_ref().is_none_or(|s| r.savedate.as_str() >= s.as_str()))
        .filter(|r| tags.is_empty() || r.tags.iter().any(|t| tags.contains(&t.to_lowercase())))
        .filter(|r| opts.types.iter().all(|f| f.matches(r)))
        .filter(|r| opts.priors.iter().all(|p| p.possible(r)))
        // Owners' labels are a weak signal (a $11k, salt-93 list declared "B3" exists), so the
        // ceiling applies to declared decks too.
        .filter(|r| {
            ceiling.is_none_or(|c| {
                r.salt <= c.salt && (!opts.price_ceiling || opts.max_price.is_some() || r.price <= c.price)
            })
        })
        .filter(|r| {
            let sig = (
                (r.price * 100.0) as i64,
                (r.salt * 1000.0) as i64,
                r.creature,
                r.instant,
                r.sorcery,
                r.artifact,
                r.enchantment,
                r.land,
            );
            seen_signatures.insert(sig)
        })
        .collect();

    let (cached, fresh): (Vec<&TableRow>, Vec<&TableRow>) = eligible.iter().partition(|r| is_cached(&r.urlhash));

    let mut salts: Vec<f64> = eligible.iter().map(|r| r.salt).collect();
    let mut prices: Vec<f64> = eligible.iter().map(|r| r.price).collect();
    salts.sort_by(f64::total_cmp);
    prices.sort_by(f64::total_cmp);

    // Group by primary theme (first EDHREC tag; untagged is its own group), then by salt/price tier.
    let mut themes: BTreeMap<String, BTreeMap<(u8, u8), Vec<&TableRow>>> = BTreeMap::new();
    for r in fresh {
        let theme = r.tags.first().cloned().unwrap_or_default();
        let tier = (tercile(&salts, r.salt), tercile(&prices, r.price));
        themes.entry(theme).or_default().entry(tier).or_default().push(r);
    }
    let sizes: Vec<usize> = themes.values().map(|t| t.values().map(Vec::len).sum()).collect();
    let weights: Vec<f64> = sizes.iter().map(|&s| (s as f64).sqrt()).collect();

    // How likely a deck is to pass the card filters, in tenths of the tightest minimum: a coarse
    // estimate, so close calls still go to the declared, recent deck.
    let likelihood = |r: &TableRow| -> i64 {
        let s = opts.priors.iter().map(|p| p.score(r)).fold(f64::INFINITY, f64::min);
        if s.is_finite() { (s * 10.0).floor() as i64 } else { 0 }
    };
    // Per theme: one queue per tier, taken round-robin.
    let mut queues: Vec<(usize, Vec<Vec<&TableRow>>)> = themes
        .into_values()
        .map(|tiers| {
            let tiers = tiers
                .into_values()
                .map(|mut v| {
                    // Likeliest to pass the card filters first, then declared-in-target, then most
                    // recent; we pop from the back.
                    v.sort_by(|a, b| {
                        likelihood(a)
                            .cmp(&likelihood(b))
                            .then(a.bracket.is_some().cmp(&b.bracket.is_some()))
                            .then(a.savedate.cmp(&b.savedate))
                    });
                    v
                })
                .collect();
            (0, tiers)
        })
        .collect();
    let mut to_fetch = Vec::with_capacity(sizes.iter().sum());
    for theme in weighted_order(&weights, &sizes) {
        let (next, tiers) = &mut queues[theme];
        let n = tiers.len();
        let tier = (0..n).map(|k| (*next + k) % n).find(|&t| !tiers[t].is_empty()).expect("theme has decks left");
        to_fetch.extend(tiers[tier].pop());
        *next = (tier + 1) % n;
    }
    let more = to_fetch.split_off(opts.budget.min(to_fetch.len()));
    Selection { cached, to_fetch, more, eligible: eligible.len(), total: rows.len(), ceiling }
}

/// Split `budget` across groups proportionally to the square root of their size (Sainte-Laguë
/// divisors), never giving a group more than it has. Small themes get a voice; big ones still get
/// the most.
pub fn allocate(sizes: &[usize], budget: usize) -> Vec<usize> {
    let weights: Vec<f64> = sizes.iter().map(|&s| (s as f64).sqrt()).collect();
    allocate_weighted(&weights, sizes, budget)
}

/// [`allocate`] with explicit weights and capacities.
pub fn allocate_weighted(weights: &[f64], caps: &[usize], budget: usize) -> Vec<usize> {
    let mut quotas = vec![0usize; caps.len()];
    for i in weighted_order(weights, caps).into_iter().take(budget) {
        quotas[i] += 1;
    }
    quotas
}

/// The groups to take from, one pick at a time, until every group is used up. Each pick goes to the
/// group with the highest `weight / (taken + 0.5)`, so the first `n` picks are exactly
/// [`allocate_weighted`] with budget `n`.
pub fn weighted_order(weights: &[f64], caps: &[usize]) -> Vec<usize> {
    weighted_order_from(weights, caps, &vec![0; caps.len()])
}

/// [`weighted_order`] for groups that already hold `had[i]` items: those count as taken, so a
/// group that already has more than its share waits until the others catch up. `caps` counts only
/// the items still to hand out.
pub fn weighted_order_from(weights: &[f64], caps: &[usize], had: &[usize]) -> Vec<usize> {
    let mut given = vec![0usize; caps.len()];
    let priority = |i: usize, q: usize| weights[i] / ((had[i] + q) as f64 + 0.5);
    let mut order = Vec::with_capacity(caps.iter().sum());
    while let Some(i) = (0..caps.len())
        .filter(|&i| given[i] < caps[i])
        .max_by(|&a, &b| priority(a, given[a]).total_cmp(&priority(b, given[b])).then(b.cmp(&a)))
    {
        given[i] += 1;
        order.push(i);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hash: &str, bracket: Option<u8>, salt: f64, price: f64, tags: &[&str], date: &str) -> TableRow {
        TableRow {
            urlhash: hash.into(),
            savedate: date.into(),
            price,
            salt,
            creature: 30,
            instant: 10,
            sorcery: 10,
            artifact: 10,
            enchantment: 4,
            land: 36,
            bracket,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn opts(budget: usize) -> SelectOptions {
        SelectOptions {
            brackets: (2, 3),
            declared_only: false,
            max_price: None,
            price_ceiling: false,
            max_salt: None,
            tags: vec![],
            since: None,
            types: vec![],
            priors: vec![],
            budget,
        }
    }

    #[test]
    fn declared_outside_target_and_incomplete_lists_are_dropped() {
        let mut short = row("short", None, 50.0, 100.0, &[], "2026-01-01");
        short.creature = 5;
        let rows = vec![
            row("b4", Some(4), 50.0, 100.0, &[], "2026-01-01"),
            short,
            row("ok", None, 50.0, 101.0, &[], "2026-01-01"),
        ];
        let s = select(&rows, &opts(10), |_| false);
        assert_eq!(s.to_fetch.iter().map(|r| r.urlhash.as_str()).collect::<Vec<_>>(), vec!["ok"]);
    }

    #[test]
    fn calibrated_ceiling_drops_salty_decks_and_pricey_ones_only_with_price() {
        let mut rows: Vec<TableRow> = (0..30)
            .map(|i| row(&format!("d{i}"), Some(3), 50.0 + i as f64 * 0.5, 300.0 + i as f64, &[], "2026-01-01"))
            .collect();
        rows.push(row("salty", None, 95.0, 400.0, &[], "2026-01-01"));
        rows.push(row("pricey", None, 55.0, 9000.0, &[], "2026-01-01"));
        rows.push(row("fine", None, 55.2, 311.5, &[], "2026-01-01"));
        let got = |o: &SelectOptions| -> HashSet<String> {
            select(&rows, o, |_| false).to_fetch.iter().map(|r| r.urlhash.clone()).collect()
        };
        // default: salt ceiling only — price is ignored
        let by_salt = got(&opts(100));
        assert!(by_salt.contains("fine") && by_salt.contains("pricey") && !by_salt.contains("salty"));
        // --price: the price ceiling applies too
        let by_both = got(&SelectOptions { price_ceiling: true, ..opts(100) });
        assert!(by_both.contains("fine") && !by_both.contains("pricey") && !by_both.contains("salty"));
        assert!(select(&rows, &opts(100), |_| false).ceiling.is_some());
    }

    #[test]
    fn budget_is_spread_across_themes_and_cached_decks_are_free() {
        let mut rows = Vec::new();
        for i in 0..20 {
            rows.push(row(&format!("tok{i}"), None, 50.0, 100.0 + i as f64, &["Tokens"], "2026-01-01"));
        }
        rows.push(row("eld", None, 50.0, 99.0, &["Eldrazi"], "2026-01-01"));
        rows.push(row("cached", None, 50.0, 98.0, &["Tokens"], "2026-01-01"));
        let s = select(&rows, &opts(3), |h| h == "cached");
        assert_eq!(s.cached.len(), 1);
        assert_eq!(s.to_fetch.len(), 3);
        assert!(s.to_fetch.iter().any(|r| r.urlhash == "eld"));
        assert_eq!(s.more.len(), 18);
    }

    #[test]
    fn decks_past_the_budget_continue_the_same_order() {
        let mut rows = Vec::new();
        for (theme, n) in [("Tokens", 12), ("Eldrazi", 4), ("Voltron", 1)] {
            for i in 0..n {
                let price = 100.0 + rows.len() as f64; // distinct, so none count as duplicates
                rows.push(row(&format!("{theme}{i}"), None, 40.0 + i as f64, price, &[theme], "2026-01-01"));
            }
        }
        let hashes = |v: &[&TableRow]| v.iter().map(|r| r.urlhash.clone()).collect::<Vec<_>>();
        let all = select(&rows, &opts(100), |_| false);
        assert_eq!(all.to_fetch.len(), 17);
        for budget in [0, 1, 5, 9, 17] {
            let s = select(&rows, &opts(budget), |_| false);
            assert_eq!([hashes(&s.to_fetch), hashes(&s.more)].concat(), hashes(&all.to_fetch), "budget {budget}");
            assert_eq!(s.to_fetch.len(), budget);
        }
    }

    #[test]
    fn allocation_favours_big_groups_without_starving_small_ones() {
        assert_eq!(allocate(&[10000, 100, 1], 12), vec![11, 1, 0]);
        assert_eq!(allocate(&[20, 1], 3), vec![2, 1]);
        assert_eq!(allocate(&[25, 25], 12), vec![6, 6]);
        assert_eq!(allocate(&[2, 1], 10), vec![2, 1]);
        assert_eq!(allocate(&[], 5), Vec::<usize>::new());
    }

    #[test]
    fn type_filters_apply() {
        let mut walkers = row("walkers", None, 50.0, 100.0, &[], "2026-01-01");
        walkers.planeswalker = 12;
        walkers.creature = 18; // keep it a complete 100-card list
        let rows = vec![walkers, row("plain", None, 50.0, 101.0, &[], "2026-01-01")];
        let o = SelectOptions { types: vec!["planeswalkers=10+".parse().unwrap()], ..opts(10) };
        let s = select(&rows, &o, |_| false);
        assert_eq!(s.to_fetch.iter().map(|r| r.urlhash.as_str()).collect::<Vec<_>>(), vec!["walkers"]);
    }

    #[test]
    fn card_filters_skip_hopeless_decks_and_fetch_likely_ones_first() {
        use edh_data::cards::{Card, CardDb};
        let card = |name: &str, type_line: &str, rank: u32| Card {
            name: name.into(),
            type_line: type_line.into(),
            oracle: vec![String::new()],
            layout: "normal".into(),
            rank: Some(rank),
            ..Default::default()
        };
        let db = CardDb::new(vec![
            card("Bonesplitter", "Artifact — Equipment", 10),
            card("Sol Ring", "Artifact", 1),
            card("Llanowar Elves", "Creature — Elf Druid", 2),
        ]);
        let prior = match "equipment=10+".parse::<crate::filters::Filter>().unwrap() {
            crate::filters::Filter::Cards(c) => c.compile(&db, None).unwrap().prior(&db).unwrap(),
            _ => unreachable!(),
        };
        // 99 cards each: artifacts vs instants, creatures fixed.
        let deck = |hash: &str, creature: u32, artifact: u32| TableRow {
            creature,
            artifact,
            instant: 63 - creature - artifact,
            sorcery: 0,
            enchantment: 0,
            ..row(hash, None, 50.0, 100.0, &[], "2026-01-01")
        };
        let rows = vec![
            deck("few", 4, 5), // equipment can only be artifacts or creatures: 9 < 10
            deck("some", 20, 10),
            deck("many", 20, 30),
            deck("most", 20, 40),
            deck("mid", 20, 20),
        ];
        let with = SelectOptions { priors: vec![prior], ..opts(10) };
        let s = select(&rows, &with, |_| false);
        assert_eq!(s.eligible, 4);
        assert_eq!(s.to_fetch.iter().map(|r| r.urlhash.as_str()).collect::<Vec<_>>(), ["most", "many", "mid", "some"]);
        // Without card filters, nothing changes.
        assert_eq!(select(&rows, &opts(10), |_| false).eligible, 5);
    }

    #[test]
    fn weighted_allocation_respects_caps() {
        assert_eq!(allocate_weighted(&[100.0, 1.0], &[3, 50], 10), vec![3, 7]);
        assert_eq!(weighted_order(&[2.0, 1.0], &[2, 2]), vec![0, 1, 0, 1]);
    }

    #[test]
    fn groups_that_already_have_items_wait_their_turn() {
        // Group 0 is four times as popular but already holds 8: group 1 catches up first.
        let order = weighted_order_from(&[4.0, 1.0], &[10, 10], &[8, 0]);
        // After that the popular group gets most of the picks again.
        assert_eq!(&order[..10], &[1, 1, 0, 0, 1, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn duplicates_are_skipped() {
        let rows = vec![row("a", None, 50.0, 100.0, &[], "2026-01-01"), row("b", None, 50.0, 100.0, &[], "2026-01-02")];
        assert_eq!(select(&rows, &opts(10), |_| false).eligible, 1);
    }
}
