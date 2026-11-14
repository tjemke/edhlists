//! Ranking decks against a target ("brackets 2-3") with EDHPowerLevel as the default yardstick.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};

use crate::analysis::Analysis;
use crate::select::weighted_order;
use edhpl::level_band;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum RankMode {
    /// How close EDHPowerLevel's score is to the middle of the target band (default).
    Fit,
    /// Highest EDHPowerLevel score first.
    Power,
    /// Weighted blend of power fit, salt, structure and synergy (see --weights).
    Combined,
    /// Least salty first.
    Salt,
    /// Closest to the deckbuilding template first.
    Structure,
    /// Most on-plan for the commander first.
    Synergy,
}

impl RankMode {
    pub const ALL: [RankMode; 6] =
        [RankMode::Fit, RankMode::Power, RankMode::Combined, RankMode::Salt, RankMode::Structure, RankMode::Synergy];
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    pub fn label(self) -> &'static str {
        match self {
            RankMode::Fit => "fit",
            RankMode::Power => "power",
            RankMode::Combined => "combined",
            RankMode::Salt => "salt",
            RankMode::Structure => "structure",
            RankMode::Synergy => "synergy",
        }
    }
    /// What the ranking sorts by, for legends and reports.
    pub fn describe(self) -> &'static str {
        match self {
            RankMode::Fit => "power fit (closest to the middle of the target band)",
            RankMode::Power => "power (highest first)",
            RankMode::Combined => "combined score",
            RankMode::Salt => "salt (least salty first)",
            RankMode::Structure => "structure",
            RankMode::Synergy => "synergy",
        }
    }
}

/// Weights for `--rank combined`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    pub fit: f64,
    pub salt: f64,
    pub structure: f64,
    pub synergy: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self { fit: 0.5, salt: 0.2, structure: 0.15, synergy: 0.15 }
    }
}

impl FromStr for Weights {
    type Err = anyhow::Error;
    /// "fit=0.6,salt=0.4" — unnamed components become 0. "power" is accepted for "fit".
    fn from_str(s: &str) -> Result<Self> {
        let mut w = Weights { fit: 0.0, salt: 0.0, structure: 0.0, synergy: 0.0 };
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let Some((k, v)) = part.split_once('=') else { bail!("expected name=value, got '{part}'") };
            let v: f64 = v.trim().parse().map_err(|_| anyhow::anyhow!("'{v}' is not a number"))?;
            if v < 0.0 {
                bail!("weights must be non-negative");
            }
            match k.trim() {
                "fit" | "power" => w.fit = v,
                "salt" => w.salt = v,
                "structure" => w.structure = v,
                "synergy" => w.synergy = v,
                other => bail!("unknown weight '{other}' (use fit, salt, structure, synergy)"),
            }
        }
        if w.fit + w.salt + w.structure + w.synergy <= 0.0 {
            bail!("at least one weight must be positive");
        }
        Ok(w)
    }
}

impl fmt::Display for Weights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fit={} salt={} structure={} synergy={}", self.fit, self.salt, self.structure, self.synergy)
    }
}

/// A `lo-hi` pair, e.g. brackets "2-3" or power "5.5-7.5"; a single value means lo == hi.
pub fn parse_range<T: FromStr + PartialOrd + Copy>(s: &str) -> Result<(T, T)> {
    let parse = |x: &str| x.trim().parse::<T>().map_err(|_| anyhow::anyhow!("'{x}' is not a valid number"));
    let (lo, hi) = match s.split_once('-') {
        Some((a, b)) => (parse(a)?, parse(b)?),
        None => {
            let v = parse(s)?;
            (v, v)
        }
    };
    if lo > hi { Ok((hi, lo)) } else { Ok((lo, hi)) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub brackets: (u8, u8),
    /// EDHPowerLevel score band `(lo, hi]`.
    pub band: (f64, f64),
    /// Was the band given explicitly (instead of derived from the brackets)?
    pub custom_band: bool,
}

impl Target {
    pub fn new(brackets: (u8, u8), band: Option<(f64, f64)>) -> Self {
        let derived = level_band(brackets.0, brackets.1);
        Self { brackets, band: band.unwrap_or(derived), custom_band: band.is_some() }
    }

    pub fn describe(&self) -> String {
        let b = if self.brackets.0 == self.brackets.1 {
            format!("B{}", self.brackets.0)
        } else {
            format!("B{}–{}", self.brackets.0, self.brackets.1)
        };
        let hi = if self.band.1.is_finite() { format!("{:.1}", self.band.1) } else { "10".into() };
        format!("{b} · power {:.1}–{hi}", self.band.0)
    }

    /// The deck fits if EDHPowerLevel's bracket is in range (or, with a custom band, if the power
    /// level is in the band and the rules floor doesn't exceed the top bracket).
    pub fn fits(&self, a: &Analysis) -> bool {
        if self.custom_band {
            let lvl = a.power.level;
            lvl > self.band.0 && lvl <= self.band.1 && a.bracket.floor <= self.brackets.1
        } else {
            (self.brackets.0..=self.brackets.1).contains(&a.bracket.bracket)
        }
    }

    /// 1 at the centre of the band, 0.5 at its edges, falling to 0 one point outside.
    pub fn power_fit(&self, level: f64) -> f64 {
        let (lo, hi) = (self.band.0, self.band.1.min(10.0));
        let centre = (lo + hi) / 2.0;
        let half = ((hi - lo) / 2.0).max(0.05);
        let d = (level - centre).abs();
        if d <= half { 1.0 - 0.5 * d / half } else { (0.5 * (1.0 - (d - half))).max(0.0) }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scores {
    /// How close the power level is to the middle of the target band (see [`Target::power_fit`]).
    pub fit: f64,
    pub salt: f64,
    pub structure: f64,
    pub synergy: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Ranked {
    /// Index into the caller's slice.
    pub index: usize,
    pub fits: bool,
    pub scores: Scores,
    /// The value the current mode sorts by.
    pub key: f64,
}

/// Fraction of the other values strictly below `x`, counting ties as half. `sorted` must be in
/// ascending order and contain `x`; two binary searches replace a scan, so ranking stays
/// O(n log n) however many decks there are.
fn percentile(sorted: &[f64], x: f64) -> f64 {
    if sorted.len() <= 1 {
        return 0.5;
    }
    let below = sorted.partition_point(|v| *v < x) as f64;
    let equal = sorted.partition_point(|v| *v <= x) as f64 - below;
    (below + 0.5 * (equal - 1.0)) / (sorted.len() as f64 - 1.0)
}

/// Rank analyses (best first). Salt and synergy are scored relative to the pool being ranked.
pub fn rank(analyses: &[&Analysis], target: &Target, mode: RankMode, weights: &Weights) -> Vec<Ranked> {
    let mut salts: Vec<f64> = analyses.iter().filter_map(|a| a.salt).collect();
    let mut syns: Vec<f64> = analyses.iter().filter_map(|a| a.synergy.as_ref().map(|s| s.average)).collect();
    salts.sort_by(f64::total_cmp);
    syns.sort_by(f64::total_cmp);
    let mut out: Vec<Ranked> = analyses
        .iter()
        .enumerate()
        .map(|(index, a)| {
            let scores = Scores {
                fit: target.power_fit(a.power.level),
                salt: a.salt.map(|s| 1.0 - percentile(&salts, s)).unwrap_or(0.5),
                structure: a.structure.score,
                synergy: a.synergy.as_ref().map(|s| percentile(&syns, s.average)).unwrap_or(0.5),
            };
            let key = match mode {
                RankMode::Fit => scores.fit,
                RankMode::Power => a.power.level,
                RankMode::Salt => scores.salt,
                RankMode::Structure => scores.structure,
                RankMode::Synergy => scores.synergy,
                RankMode::Combined => {
                    let w = weights;
                    (w.fit * scores.fit
                        + w.salt * scores.salt
                        + w.structure * scores.structure
                        + w.synergy * scores.synergy)
                        / (w.fit + w.salt + w.structure + w.synergy)
                }
            };
            Ranked { index, fits: target.fits(a), scores, key }
        })
        .collect();
    out.sort_by(|a, b| {
        b.fits
            .cmp(&a.fits)
            .then(b.key.total_cmp(&a.key))
            .then(b.scores.salt.total_cmp(&a.scores.salt))
            .then(a.index.cmp(&b.index))
    });
    out
}

/// Interleave groups (commanders) in a ranking so one with many decks doesn't fill the top of the
/// list. `group[i]` is the group of the caller's deck `i`; groups take turns in proportion to
/// `weights` (Sainte-Laguë, as when fetching), each giving its best remaining deck. Decks in the
/// target still come first, and each group's decks keep their order.
pub fn mix(ranking: Vec<Ranked>, group: &[usize], weights: &[f64]) -> Vec<Ranked> {
    let (fits, rest): (Vec<Ranked>, Vec<Ranked>) = ranking.into_iter().partition(|r| r.fits);
    let interleave = |block: Vec<Ranked>| -> Vec<Ranked> {
        let mut queues: Vec<std::collections::VecDeque<Ranked>> = vec![Default::default(); weights.len()];
        for r in block {
            queues[group[r.index]].push_back(r);
        }
        let caps: Vec<usize> = queues.iter().map(|q| q.len()).collect();
        weighted_order(weights, &caps).into_iter().map(|g| queues[g].pop_front().expect("within caps")).collect()
    };
    let mut out = interleave(fits);
    out.extend(interleave(rest));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis(level: f64, bracket: u8, salt: f64) -> Analysis {
        let mut a = Analysis::default();
        a.power.level = level;
        a.bracket.bracket = bracket;
        a.bracket.floor = 1;
        a.salt = Some(salt);
        a
    }

    #[test]
    fn percentiles_match_counting_every_value() {
        // The scan this replaced: below + half the other ties, out of everything else.
        fn counted(values: &[f64], x: f64) -> f64 {
            if values.len() <= 1 {
                return 0.5;
            }
            let below = values.iter().filter(|v| **v < x).count() as f64;
            let equal = values.iter().filter(|v| **v == x).count() as f64;
            (below + 0.5 * (equal - 1.0)) / (values.len() as f64 - 1.0)
        }
        let values: Vec<f64> = (0..200).map(|i| f64::from((i * 37) % 23) * 0.5).collect();
        let mut sorted = values.clone();
        sorted.sort_by(f64::total_cmp);
        for &x in &values {
            assert!((percentile(&sorted, x) - counted(&values, x)).abs() < 1e-12, "{x}");
        }
        assert_eq!(percentile(&[3.0], 3.0), 0.5);
    }

    #[test]
    fn weights_parse() {
        let w: Weights = "power=0.6, salt=0.4".parse().unwrap();
        assert_eq!((w.fit, w.salt, w.structure), (0.6, 0.4, 0.0));
        assert_eq!("fit=1".parse::<Weights>().unwrap().fit, 1.0);
        assert!("power=x".parse::<Weights>().is_err());
        assert!("bogus=1".parse::<Weights>().is_err());
        assert!("power=0".parse::<Weights>().is_err());
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range::<u8>("2-3").unwrap(), (2, 3));
        assert_eq!(parse_range::<u8>("3").unwrap(), (3, 3));
        assert_eq!(parse_range::<f64>("7.5-5.5").unwrap(), (5.5, 7.5));
    }

    #[test]
    fn default_target_is_edhpl_bracket_band() {
        let t = Target::new((2, 3), None);
        assert_eq!(t.band, (4.7, 7.7));
        assert!((t.power_fit(6.2) - 1.0).abs() < 1e-9);
        assert!((t.power_fit(7.7) - 0.5).abs() < 1e-9);
        assert!(t.power_fit(9.0) == 0.0);
    }

    #[test]
    fn fitting_decks_come_first_then_power_fit() {
        let a = analysis(8.5, 4, 60.0);
        let b = analysis(7.5, 3, 50.0);
        let c = analysis(6.2, 3, 70.0);
        let t = Target::new((2, 3), None);
        let r = rank(&[&a, &b, &c], &t, RankMode::Fit, &Weights::default());
        assert_eq!(r.iter().map(|x| x.index).collect::<Vec<_>>(), vec![2, 1, 0]);
        let r = rank(&[&a, &b, &c], &t, RankMode::Salt, &Weights::default());
        assert_eq!(r.iter().map(|x| x.index).collect::<Vec<_>>(), vec![1, 2, 0]);
    }

    #[test]
    fn mixing_interleaves_commanders_by_weight_and_keeps_fits_first() {
        // Decks 0-5 belong to commander 0, 6-7 to commander 1; deck 7 misses the target.
        let t = Target::new((2, 3), None);
        let decks: Vec<Analysis> = [6.2, 6.1, 6.0, 5.9, 5.8, 5.7, 5.5, 9.5]
            .iter()
            .map(|&l| analysis(l, if l > 8.0 { 5 } else { 3 }, 50.0))
            .collect();
        let refs: Vec<&Analysis> = decks.iter().collect();
        let ranked = rank(&refs, &t, RankMode::Power, &Weights::default());
        let group = [0, 0, 0, 0, 0, 0, 1, 1];
        let order = |w: &[f64]| mix(ranked.clone(), &group, w).iter().map(|r| r.index).collect::<Vec<_>>();
        // Equal weights: commander 1's only fitting deck comes second, its misfit stays last.
        assert_eq!(order(&[1.0, 1.0]), vec![0, 6, 1, 2, 3, 4, 5, 7]);
        // Commander 0 four times as popular: it gets more of the top, commander 1 still shows up.
        assert_eq!(order(&[4.0, 1.0]), vec![0, 1, 6, 2, 3, 4, 5, 7]);
    }

    #[test]
    fn power_ranks_strongest_first_within_each_group() {
        let a = analysis(5.0, 2, 50.0);
        let b = analysis(8.5, 4, 50.0);
        let c = analysis(7.5, 3, 50.0);
        let d = analysis(9.5, 5, 50.0);
        let t = Target::new((2, 3), None);
        let r = rank(&[&a, &b, &c, &d], &t, RankMode::Power, &Weights::default());
        assert_eq!(r.iter().map(|x| x.index).collect::<Vec<_>>(), vec![2, 0, 3, 1]);
    }
}
