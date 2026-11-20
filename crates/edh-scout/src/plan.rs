//! Searching across commanders: whose deck to fetch next, and when to scan more commanders' deck
//! tables, from how their decks have done against the `-T` card filters so far.
//!
//! The fetch budget goes out in proportion to √(eligible decks) × (how likely a deck passes),
//! interleaved as [`weighted_order_from`](crate::select::weighted_order_from) does. The pass rate
//! starts at the rate across every commander so far, worth [`PRIOR_DECKS`] decks, and moves to the
//! commander's own as its decks are judged; without card filters every deck passes and the order
//! is the table's alone. A commander is a miss once its table has no eligible decks or
//! `miss_after` of its decks (all of them, if it has fewer; [`MISS_AFTER`] unless set with
//! `--miss-after` or the config file) failed without one passing. While fewer than a third of the
//! commanders scanned (at least [`MIN_LIVE`]) are live, the search scans [`EXPAND_BY`] more from
//! its reserve, then fetches `widen_every` decks ([`WIDEN_EVERY`] unless set with `--widen-every`
//! or the config file; 0 never widens) before it looks again, unless nothing is left to fetch.

/// How many decks the overall pass rate counts for in a commander's estimate.
pub const PRIOR_DECKS: f64 = 6.0;
/// Decks judged without one passing before a commander counts as a miss, unless set otherwise.
pub const MISS_AFTER: usize = 3;
/// Commanders scanned at a time when the search widens.
pub const EXPAND_BY: usize = 5;
/// Decks fetched between one widening and the next, unless set otherwise.
pub const WIDEN_EVERY: usize = 5;
/// The fewest live commanders a search settles for while it has more in reserve.
pub const MIN_LIVE: usize = 3;

#[derive(Debug, Default)]
struct Planned {
    eligible: usize,
    /// Decks it already has a share of: stored ones, and those fetched this run.
    taken: usize,
    /// Decks still queued to fetch.
    queued: usize,
    /// Decks scored against the card filters so far, and those that passed.
    judged: usize,
    passed: usize,
}

#[derive(Debug, Default)]
pub struct SearchPlan {
    commanders: Vec<Planned>,
    /// Decks fetched since the search last widened.
    since_expansion: usize,
    /// Decks judged without one passing before a commander counts as a miss.
    miss_after: usize,
    /// Decks fetched between one widening and the next; 0 never widens.
    widen_every: usize,
}

impl SearchPlan {
    /// A plan that counts a commander as a miss after `miss_after` failed decks, none passing, and
    /// widens the search at most every `widen_every` fetched decks (0: never).
    pub fn new(miss_after: usize, widen_every: usize) -> Self {
        Self { commanders: Vec::new(), since_expansion: widen_every, miss_after: miss_after.max(1), widen_every }
    }

    /// A commander whose table was scanned: `eligible` decks, `stored` of them already in the
    /// deck database, `queued` to fetch. Returns its index.
    pub fn add(&mut self, eligible: usize, stored: usize, queued: usize) -> usize {
        self.commanders.push(Planned { eligible, taken: stored, queued, ..Default::default() });
        self.commanders.len() - 1
    }

    /// One of commander `i`'s decks was scored: did it pass the card filters?
    pub fn judge(&mut self, i: usize, passed: bool) {
        let c = &mut self.commanders[i];
        c.judged += 1;
        c.passed += usize::from(passed);
    }

    /// A deck of commander `i` was taken from its queue to fetch.
    pub fn took(&mut self, i: usize) {
        let c = &mut self.commanders[i];
        c.queued -= 1;
        c.taken += 1;
        self.since_expansion += 1;
    }

    /// The search scanned more commanders.
    pub fn expanded(&mut self) {
        self.since_expansion = 0;
    }

    pub fn queued(&self) -> usize {
        self.commanders.iter().map(|c| c.queued).sum()
    }

    /// No eligible decks, or `miss_after` judged without one passing (all of them, for a
    /// commander with fewer eligible decks than that).
    fn missed(&self, c: &Planned) -> bool {
        c.eligible == 0 || (c.passed == 0 && c.judged >= self.miss_after.min(c.eligible))
    }

    /// Commanders still worth fetching from: eligible decks, and not only failures so far.
    pub fn live(&self) -> usize {
        self.commanders.iter().filter(|c| !self.missed(c)).count()
    }

    /// Should the search scan more commanders, with `reserve` left to scan? When nothing is left
    /// to fetch, or when too few commanders are live and enough decks were fetched since it last
    /// widened; never with `widen_every` 0.
    pub fn should_expand(&self, reserve: usize) -> bool {
        let want = (self.commanders.len() / 3).max(MIN_LIVE);
        self.widen_every > 0
            && reserve > 0
            && (self.queued() == 0 || (self.live() < want && self.since_expansion >= self.widen_every))
    }

    /// How likely commander `i`'s next deck is to pass, from its own decks and everyone's.
    pub fn pass_rate(&self, i: usize) -> f64 {
        let (judged, passed) = self.commanders.iter().fold((0, 0), |(j, p), c| (j + c.judged, p + c.passed));
        // Exactly the rate seen, so that every deck passing leaves the table's order alone; while
        // none has passed, a small rate that shrinks as decks fail, so untried commanders go first.
        let overall = if passed > 0 { passed as f64 / judged as f64 } else { 0.5 / (judged as f64 + 1.0) };
        let c = &self.commanders[i];
        (c.passed as f64 + PRIOR_DECKS * overall) / (c.judged as f64 + PRIOR_DECKS)
    }

    /// The commander to fetch from next, if any has decks queued.
    pub fn next(&self) -> Option<usize> {
        let priority = |i: usize| {
            (self.commanders[i].eligible as f64).sqrt() * self.pass_rate(i) / (self.commanders[i].taken as f64 + 0.5)
        };
        (0..self.commanders.len())
            .filter(|&i| self.commanders[i].queued > 0)
            .max_by(|&a, &b| priority(a).total_cmp(&priority(b)).then(b.cmp(&a)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::weighted_order_from;

    /// Take `n` picks, judging each picked deck with `passes(commander)`.
    fn run(plan: &mut SearchPlan, n: usize, passes: impl Fn(usize) -> bool) -> Vec<usize> {
        let mut picks = Vec::new();
        for _ in 0..n {
            let Some(i) = plan.next() else { break };
            plan.took(i);
            plan.judge(i, passes(i));
            picks.push(i);
        }
        picks
    }

    #[test]
    fn without_card_filters_the_order_is_the_tables() {
        let tables = [(100, 3, 50), (25, 0, 50), (9, 1, 50)];
        let mut plan = SearchPlan::new(MISS_AFTER, WIDEN_EVERY);
        for (eligible, stored, queued) in tables {
            plan.add(eligible, stored, queued);
        }
        let picks = run(&mut plan, 30, |_| true);
        let weights: Vec<f64> = tables.iter().map(|t| (t.0 as f64).sqrt()).collect();
        let order = weighted_order_from(&weights, &[50, 50, 50], &[3, 0, 1]);
        assert_eq!(picks, order[..30], "every deck passing, the shares should be √eligible's");
    }

    #[test]
    fn a_commander_whose_decks_fail_gets_fewer_fetches() {
        let mut plan = SearchPlan::new(MISS_AFTER, WIDEN_EVERY);
        let (failing, passing) = (plan.add(100, 0, 100), plan.add(100, 0, 100));
        let picks = run(&mut plan, 40, |i| i == passing);
        let share = |c| picks.iter().filter(|&&i| i == c).count();
        assert!(
            share(passing) >= 3 * share(failing),
            "the same table share, but only one commander's decks pass: {} vs {} fetches",
            share(passing),
            share(failing)
        );
        assert_eq!(plan.live(), 1, "the failing one is a miss after {MISS_AFTER} failures");
    }

    #[test]
    fn the_search_widens_slowly_while_few_commanders_are_live() {
        let mut plan = SearchPlan::new(MISS_AFTER, WIDEN_EVERY);
        for _ in 0..7 {
            plan.add(0, 0, 0); // no eligible decks
        }
        plan.add(50, 0, 50);
        plan.add(50, 0, 50);
        assert!(plan.should_expand(10), "2 of 9 live, fewer than {MIN_LIVE}: scan more");
        assert!(!plan.should_expand(0), "nothing left in reserve");
        plan.expanded();
        assert!(!plan.should_expand(10), "just widened: fetch {WIDEN_EVERY} decks first");
        run(&mut plan, WIDEN_EVERY - 1, |_| true);
        assert!(!plan.should_expand(10));
        run(&mut plan, 1, |_| true);
        assert!(plan.should_expand(10), "{WIDEN_EVERY} decks later, still only 2 live");
        plan.add(50, 0, 50);
        plan.add(50, 0, 50);
        plan.expanded();
        run(&mut plan, WIDEN_EVERY, |_| true);
        assert!(!plan.should_expand(10), "4 of 11 live is a third: enough");
    }

    #[test]
    fn a_commander_with_few_eligible_decks_is_a_miss_once_they_all_fail() {
        let mut plan = SearchPlan::new(MISS_AFTER, WIDEN_EVERY);
        plan.add(2, 0, 2);
        run(&mut plan, 2, |_| false);
        assert_eq!(plan.live(), 0, "both its decks failed; waiting for a third would keep it live forever");
        let one = plan.add(1, 1, 0);
        plan.judge(one, true);
        assert_eq!(plan.live(), 1, "its one deck passed");
    }

    #[test]
    fn a_higher_miss_after_gives_commanders_longer_and_widens_later() {
        let fails = |miss_after: usize| {
            let mut plan = SearchPlan::new(miss_after, WIDEN_EVERY);
            for _ in 0..3 {
                plan.add(50, 0, 50);
            }
            plan.expanded();
            // Fetch until the search wants to widen; nothing passes.
            let mut fetched = 0;
            while !plan.should_expand(10) && fetched < 100 {
                run(&mut plan, 1, |_| false);
                fetched += 1;
            }
            fetched
        };
        assert_eq!(fails(3), 7, "3 commanders × 3 failures, the first miss judged on the 7th fetch");
        assert!(fails(8) > 2 * fails(3), "8 failures each before a miss: {} fetches", fails(8));
        assert_eq!(SearchPlan::new(0, WIDEN_EVERY).miss_after, 1, "0 would make every commander a miss at once");
    }

    #[test]
    fn widen_every_sets_the_pace_and_0_never_widens() {
        let plan = |widen_every: usize| {
            let mut plan = SearchPlan::new(MISS_AFTER, widen_every);
            for _ in 0..9 {
                plan.add(0, 0, 0); // no eligible decks: every one a miss
            }
            plan.add(50, 0, 50);
            plan
        };
        let mut slow = plan(20);
        assert!(slow.should_expand(10), "right after the first scan it may widen at once");
        slow.expanded();
        run(&mut slow, 19, |_| true);
        assert!(!slow.should_expand(10), "19 of 20 decks fetched since it widened");
        run(&mut slow, 1, |_| true);
        assert!(slow.should_expand(10));
        let mut never = plan(0);
        assert!(!never.should_expand(10), "1 of 10 live, but widening is off");
        run(&mut never, 50, |_| true);
        assert!(!never.should_expand(10), "not even with nothing left to fetch");
    }

    #[test]
    fn a_search_with_nothing_left_to_fetch_widens_at_once() {
        let mut plan = SearchPlan::new(MISS_AFTER, WIDEN_EVERY);
        for _ in 0..5 {
            plan.add(20, 20, 0); // all stored already, and passing
        }
        for i in 0..5 {
            plan.judge(i, true);
        }
        plan.expanded();
        assert_eq!(plan.live(), 5);
        assert!(plan.should_expand(3), "enough live commanders, but nothing to fetch from them");
    }
}
