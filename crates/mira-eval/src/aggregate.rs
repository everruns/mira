//! Trial aggregation — the contract for turning N repetitions of a case into
//! pass@k, pass-rate, and score variance.
//!
//! When a case is run multiple times (see [`Eval::trials`](crate::Eval) /
//! `--trials`), each repetition is a separate [`RunResult`] sharing one *logical*
//! key (`eval/sample@target[…]`, without the `#index` trial suffix). This module
//! groups them back by that logical key and rolls each group into a
//! [`TrialAggregate`]: how many trials passed, the pass-rate, the unbiased
//! [`pass@k`](TrialAggregate::pass_at_k) estimator, and the mean/standard
//! deviation of the case's score across trials.
//!
//! N/A and skipped trials are excluded from the denominators (as everywhere
//! else): a trial only counts toward pass@k / pass-rate / variance if it produced
//! a real verdict.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Params;
use crate::protocol::RunResult;
use crate::report::is_na;

/// Rolled-up statistics over all trials of one logical case.
///
/// `scored` is the number of trials with a real verdict (`passed + failed`);
/// `passed` of those passed. `pass_rate`, `pass_at_k`, `mean`, and `std_dev` are
/// all computed over the `scored` trials only (N/A and skipped trials are
/// excluded), matching how single-case verdicts are counted elsewhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrialAggregate {
    pub eval: String,
    pub sample: String,
    pub target: String,
    /// Extra matrix-axis values for this case (empty for a target-only matrix).
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// The logical case key all these trials share (no `#index` suffix).
    pub key: String,
    /// Total trials seen for this case, including N/A and skipped ones.
    pub total: usize,
    /// Trials with a real verdict (the denominator for pass-rate / pass@k).
    pub scored: usize,
    /// Scored trials that passed (the numerator).
    pub passed: usize,
    /// Scored trials that failed (`scored - passed`).
    pub failed: usize,
    /// Trials that ran but were all-N/A (excluded from the verdict).
    pub na: usize,
    /// Trials that never executed (e.g. target unavailable).
    pub skipped: usize,
    /// `passed / scored` (0.0 when nothing was scored). This is pass@1.
    pub pass_rate: f64,
    /// Mean of the per-trial aggregate score over scored trials.
    pub mean: f64,
    /// Population standard deviation of the per-trial aggregate score over scored
    /// trials — the reproducibility signal (0 when a stochastic case is stable).
    pub std_dev: f64,
}

impl TrialAggregate {
    /// True when this case was actually repeated (more than one trial).
    pub fn repeated(&self) -> bool {
        self.total > 1
    }

    /// The unbiased **pass@k** estimator (Chen et al., "Evaluating Large Language
    /// Models Trained on Code"): the probability that at least one of `k` samples
    /// drawn from this case's `scored` trials passes, estimated from the observed
    /// `passed` count.
    ///
    /// `1 - C(n-c, k) / C(n, k)` for `k <= n` (with `n = scored`, `c = passed`),
    /// computed in the numerically stable product form. Returns `1.0` when at
    /// least `n - c < k` (every draw of `k` must include a pass), and `0.0` when
    /// nothing was scored.
    pub fn pass_at_k(&self, k: usize) -> f64 {
        pass_at_k(self.scored, self.passed, k)
    }
}

/// The unbiased pass@k estimator for `c` correct out of `n` samples.
/// See [`TrialAggregate::pass_at_k`]. `c` is clamped to `n` (so a caller passing
/// `c > n` can't underflow), and `k` to `1..=n`. Returns `0.0` for the degenerate
/// `n == 0` or `k == 0`.
pub fn pass_at_k(n: usize, c: usize, k: usize) -> f64 {
    if n == 0 || k == 0 {
        return 0.0;
    }
    let c = c.min(n); // guard against c > n underflowing `n - c` below
    let k = k.min(n);
    // If fewer than k samples failed, every k-subset contains a pass.
    if n - c < k {
        return 1.0;
    }
    // 1 - prod_{i=n-c+1}^{n} (1 - k/i).
    let mut prod = 1.0_f64;
    for i in (n - c + 1)..=n {
        prod *= 1.0 - (k as f64) / (i as f64);
    }
    1.0 - prod
}

/// Group `results` by their logical case key (all trials of one case), in
/// first-seen order, and roll each group into a [`TrialAggregate`].
///
/// Works for any result set: a single-trial case yields a one-trial aggregate
/// (use [`TrialAggregate::repeated`] to keep only the repeated ones). The grouping
/// key is [`RunResult::logical_key`], so trials differing only by `#index` land
/// together.
pub fn aggregate_trials(results: &[RunResult]) -> Vec<TrialAggregate> {
    // Preserve first-seen order of logical keys while grouping.
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&RunResult>> = BTreeMap::new();
    for r in results {
        let key = r.logical_key();
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(r);
    }

    order
        .into_iter()
        .map(|key| {
            let group = &groups[&key];
            let first = group[0];
            let total = group.len();
            let mut scored = 0usize;
            let mut passed = 0usize;
            let mut na = 0usize;
            let mut skipped = 0usize;
            let mut values: Vec<f64> = Vec::new();
            for r in group {
                if r.skipped {
                    skipped += 1;
                } else if is_na(r) {
                    na += 1;
                } else {
                    scored += 1;
                    if r.passed {
                        passed += 1;
                    }
                    values.push(r.aggregate);
                }
            }
            let mean = if values.is_empty() {
                0.0
            } else {
                values.iter().sum::<f64>() / values.len() as f64
            };
            let std_dev = if values.is_empty() {
                0.0
            } else {
                let var =
                    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
                var.sqrt()
            };
            let pass_rate = if scored == 0 {
                0.0
            } else {
                passed as f64 / scored as f64
            };
            TrialAggregate {
                eval: first.eval.clone(),
                sample: first.sample.clone(),
                target: first.target.clone(),
                params: first.params.clone(),
                key,
                total,
                scored,
                passed,
                failed: scored - passed,
                na,
                skipped,
                pass_rate,
                mean,
                std_dev,
            }
        })
        .collect()
}

/// True when any case in `results` was repeated (more than one trial) — i.e. the
/// trial dimension is active and a trials report is worth rendering.
pub fn has_trials(results: &[RunResult]) -> bool {
    aggregate_trials(results).iter().any(|a| a.repeated())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Score;
    use crate::protocol::TranscriptSummary;

    fn trial(target: &str, trial: usize, trials: usize, passed: bool) -> RunResult {
        RunResult {
            eval: "e".into(),
            sample: "s".into(),
            target: target.into(),
            params: Default::default(),
            trial,
            trials,
            seed: Some(trial as u64),
            input: Vec::new(),
            expected: None,
            passed,
            aggregate: if passed { 1.0 } else { 0.0 },
            scores: vec![if passed {
                Score::pass("x", "ok")
            } else {
                Score::fail("x", "no")
            }],
            transcript: TranscriptSummary::default(),
            skipped: false,
        }
    }

    #[test]
    fn pass_at_k_matches_reference() {
        // All-correct ⇒ 1.0 for any k.
        assert!((pass_at_k(5, 5, 1) - 1.0).abs() < 1e-9);
        assert!((pass_at_k(5, 5, 3) - 1.0).abs() < 1e-9);
        // None correct ⇒ 0.0.
        assert_eq!(pass_at_k(5, 0, 1), 0.0);
        // pass@1 == c/n.
        assert!((pass_at_k(10, 4, 1) - 0.4).abs() < 1e-9);
        // 1 correct of 2, k=2 ⇒ both-of-2 must include the pass ⇒ 1.0.
        assert!((pass_at_k(2, 1, 2) - 1.0).abs() < 1e-9);
        // Known value: n=4, c=2, k=2 ⇒ 1 - C(2,2)/C(4,2) = 1 - 1/6.
        assert!((pass_at_k(4, 2, 2) - (1.0 - 1.0 / 6.0)).abs() < 1e-9);
        // Degenerate inputs.
        assert_eq!(pass_at_k(0, 0, 1), 0.0);
        assert_eq!(pass_at_k(3, 1, 0), 0.0);
    }

    #[test]
    fn groups_trials_by_logical_key() {
        // Two cases, 4 trials each (3/4 and 1/4 passing).
        let mut results = Vec::new();
        for t in 0..4 {
            results.push(trial("sim", t, 4, t != 0)); // 3 pass
        }
        for t in 0..4 {
            results.push(trial("opus", t, 4, t == 0)); // 1 pass
        }
        let aggs = aggregate_trials(&results);
        assert_eq!(aggs.len(), 2);

        let sim = aggs.iter().find(|a| a.target == "sim").unwrap();
        assert_eq!(sim.total, 4);
        assert_eq!(sim.scored, 4);
        assert_eq!(sim.passed, 3);
        assert_eq!(sim.failed, 1);
        assert!((sim.pass_rate - 0.75).abs() < 1e-9);
        assert!(sim.repeated());
        // pass@1 == pass_rate; pass@4 (k=n, at least one of all) == 1.0 here.
        assert!((sim.pass_at_k(1) - 0.75).abs() < 1e-9);
        assert!((sim.pass_at_k(4) - 1.0).abs() < 1e-9);
        // Score variance: mean 0.75, values {1,1,1,0} ⇒ var .1875 ⇒ sd ~.433.
        assert!((sim.mean - 0.75).abs() < 1e-9);
        assert!((sim.std_dev - 0.1875_f64.sqrt()).abs() < 1e-9);

        let opus = aggs.iter().find(|a| a.target == "opus").unwrap();
        assert_eq!(opus.passed, 1);
        assert!((opus.pass_rate - 0.25).abs() < 1e-9);
    }

    #[test]
    fn na_and_skipped_trials_excluded_from_denominator() {
        let mut na = trial("sim", 1, 3, false);
        na.scores = vec![Score::na("judge", "unreachable")];
        let mut skip = trial("sim", 2, 3, false);
        skip.skipped = true;
        let results = vec![trial("sim", 0, 3, true), na, skip];

        let aggs = aggregate_trials(&results);
        assert_eq!(aggs.len(), 1);
        let a = &aggs[0];
        assert_eq!(a.total, 3);
        assert_eq!(a.scored, 1); // only the real verdict counts
        assert_eq!(a.passed, 1);
        assert_eq!(a.na, 1);
        assert_eq!(a.skipped, 1);
        assert!((a.pass_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn has_trials_only_for_repeated_cases() {
        let single = vec![trial("sim", 0, 1, true)];
        assert!(!has_trials(&single));
        let repeated = vec![trial("sim", 0, 2, true), trial("sim", 1, 2, false)];
        assert!(has_trials(&repeated));
    }
}
