//! Choosing which decks to fetch, using only the deck table (zero extra requests).
//!
//! EDHREC lists tens of thousands of decks per popular commander and gives no quality signal, so
//! we spend the fetch budget carefully:
//! 1. drop incomplete lists and anything the owner declared outside the target brackets;
//! 2. calibrate a salt and price ceiling from the decks whose owners *did* declare a target
//!    bracket (their 90th percentile), and drop decks above it — they are very likely stronger or
//!    meaner than wanted, whatever their label says;
//! 3. drop exact duplicates (re-uploads of the same list);
//! 4. include every already-cached deck for free, then spread the remaining budget across themes
//!    and salt/price tiers so the pool isn't fifty copies of the same idea.

use std::collections::{BTreeMap, HashSet};

use edh_data::edhrec::TableRow;

#[derive(Clone, Debug)]
pub struct SelectOptions {
    pub brackets: (u8, u8),
    pub declared_only: bool,
    pub max_price: Option<f64>,
    /// Skip decks whose EDHREC salt is above this.
    pub max_salt: Option<f64>,
    /// Keep only decks with one of these EDHREC tags (case-insensitive).
    pub tags: Vec<String>,
    /// Keep only decks saved on/after this date (YYYY-MM-DD).
    pub since: Option<String>,
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
        // Owners' labels are a weak signal (a $11k, salt-93 list declared "B3" exists), so the
        // ceiling applies to declared decks too.
        .filter(|r| ceiling.is_none_or(|c| r.salt <= c.salt && (opts.max_price.is_some() || r.price <= c.price)))
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
    let quotas = allocate(&sizes, opts.budget);

    let mut to_fetch = Vec::with_capacity(opts.budget);
    for (tiers, quota) in themes.into_values().zip(quotas) {
        let mut queues: Vec<Vec<&TableRow>> = tiers
            .into_values()
            .map(|mut v| {
                // Declared-in-target first, then most recent; we pop from the back.
                v.sort_by(|a, b| a.bracket.is_some().cmp(&b.bracket.is_some()).then(a.savedate.cmp(&b.savedate)));
                v
            })
            .collect();
        let mut taken = 0;
        while taken < quota && queues.iter().any(|q| !q.is_empty()) {
            for q in queues.iter_mut() {
                if taken < quota
                    && let Some(r) = q.pop()
                {
                    to_fetch.push(r);
                    taken += 1;
                }
            }
        }
    }
    Selection { cached, to_fetch, eligible: eligible.len(), total: rows.len(), ceiling }
}

/// Split `budget` across groups proportionally to the square root of their size (Sainte-Laguë
/// divisors), never giving a group more than it has. Small themes get a voice; big ones still get
/// the most.
pub fn allocate(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut quotas = vec![0usize; sizes.len()];
    let weights: Vec<f64> = sizes.iter().map(|&s| (s as f64).sqrt()).collect();
    let priority = |i: usize, q: usize| weights[i] / (q as f64 + 0.5);
    for _ in 0..budget {
        let best = (0..sizes.len())
            .filter(|&i| quotas[i] < sizes[i])
            .max_by(|&a, &b| priority(a, quotas[a]).total_cmp(&priority(b, quotas[b])).then(b.cmp(&a)));
        match best {
            Some(i) => quotas[i] += 1,
            None => break,
        }
    }
    quotas
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
            max_salt: None,
            tags: vec![],
            since: None,
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
    fn calibrated_ceiling_drops_salty_undeclared_decks() {
        let mut rows: Vec<TableRow> = (0..30)
            .map(|i| row(&format!("d{i}"), Some(3), 50.0 + i as f64 * 0.5, 300.0 + i as f64, &[], "2026-01-01"))
            .collect();
        rows.push(row("salty", None, 95.0, 400.0, &[], "2026-01-01"));
        rows.push(row("pricey", None, 55.0, 9000.0, &[], "2026-01-01"));
        rows.push(row("fine", None, 55.2, 311.5, &[], "2026-01-01"));
        let s = select(&rows, &opts(100), |_| false);
        let got: HashSet<&str> = s.to_fetch.iter().map(|r| r.urlhash.as_str()).collect();
        assert!(got.contains("fine"));
        assert!(!got.contains("salty") && !got.contains("pricey"));
        assert!(s.ceiling.is_some());
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
    fn duplicates_are_skipped() {
        let rows = vec![row("a", None, 50.0, 100.0, &[], "2026-01-01"), row("b", None, 50.0, 100.0, &[], "2026-01-02")];
        assert_eq!(select(&rows, &opts(10), |_| false).eligible, 1);
    }
}
