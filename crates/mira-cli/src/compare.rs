//! Compare paired logical cases, keeping repeated trials inside their case.
//! Bootstrap resamples logical cases, not individual trials, to avoid treating
//! correlated repetitions as independent evidence.

use clap::Args;
use mira::{protocol::RunResult, report};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Args, Debug)]
pub struct CompareArgs {
    pub baseline: String,
    pub candidate: String,
    /// Restrict comparison to case keys matching this glob (a slice).
    #[arg(long)]
    pub filter: Option<String>,
    /// Restrict comparison to one scorer glob, required on both sides.
    #[arg(long)]
    pub scorer: Option<String>,
    #[command(flatten)]
    pub limits: Limits,
    /// Write the full comparison JSON here (otherwise stdout).
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Args, Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Maximum logical cases whose pass rate regresses.
    #[arg(long)]
    pub max_regressions: Option<usize>,
    /// Maximum decrease in any scorer's mean (absolute 0..1).
    #[arg(long)]
    pub max_score_drop: Option<f64>,
    /// Maximum relative total subject cost increase (0.1 = 10%).
    #[arg(long)]
    pub max_cost_increase: Option<f64>,
    /// Maximum relative total subject duration increase (0.1 = 10%).
    #[arg(long)]
    pub max_latency_increase: Option<f64>,
}

impl Limits {
    pub fn validate(&self) -> Result<(), String> {
        for value in [
            self.max_score_drop,
            self.max_cost_increase,
            self.max_latency_increase,
        ]
        .into_iter()
        .flatten()
        {
            if !value.is_finite() || value < 0.0 {
                return Err("comparison limits must be finite and nonnegative".into());
            }
        }
        if self.max_score_drop.is_some_and(|v| v > 1.0) {
            return Err("max_score_drop must be in 0..1".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct CaseDelta {
    pub key: String,
    pub trials: usize,
    pub baseline_pass_rate: f64,
    pub candidate_pass_rate: f64,
    pub score_deltas: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub cases: Vec<CaseDelta>,
    pub regressions: usize,
    pub improvements: usize,
    pub pass_rate_delta: f64,
    pub pass_rate_delta_ci95: Option<[f64; 2]>,
    pub score_deltas: BTreeMap<String, f64>,
    pub baseline_cost_usd: f64,
    pub candidate_cost_usd: f64,
    pub baseline_duration_ms: u64,
    pub candidate_duration_ms: u64,
    pub incomplete: Vec<String>,
    pub gate_failures: Vec<String>,
}

impl Comparison {
    pub fn exit_code(&self) -> i32 {
        if !self.incomplete.is_empty() {
            2
        } else if !self.gate_failures.is_empty() {
            1
        } else {
            0
        }
    }
}

pub fn load(run: &str) -> Result<Vec<RunResult>, Box<dyn std::error::Error>> {
    let path = PathBuf::from(run);
    let path = if path.is_dir() {
        path
    } else {
        crate::config::run_dir(&crate::config::Config::load().results_dir(), run)
    };
    let meta = crate::config::load_meta(&path).ok_or("run has no valid meta.json")?;
    if meta.finished_unix == 0 {
        return Err("run is unfinished".into());
    }
    let results = crate::config::load_case_results(&path);
    if results.len() != meta.summary.scored + meta.summary.na + meta.summary.skipped {
        return Err("run result count differs from its summary".into());
    }
    let keys: BTreeSet<_> = results.iter().map(|r| r.key()).collect();
    if results.is_empty() || keys.len() != results.len() {
        return Err("run is empty or contains duplicate case keys".into());
    }
    Ok(results)
}

pub fn run(args: &CompareArgs) -> Result<i32, Box<dyn std::error::Error>> {
    args.limits.validate()?;
    let result = compare(
        &load(&args.baseline)?,
        &load(&args.candidate)?,
        args.filter.as_deref(),
        args.scorer.as_deref(),
        &args.limits,
    );
    eprintln!(
        "comparison: {} paired cases; {} regressions; {} improvements; {} incomplete",
        result.cases.len(),
        result.regressions,
        result.improvements,
        result.incomplete.len()
    );
    let bytes = serde_json::to_vec_pretty(&result)?;
    if let Some(out) = &args.out {
        std::fs::write(out, bytes)?;
    } else {
        println!("{}", String::from_utf8(bytes)?);
    }
    Ok(result.exit_code())
}

pub fn compare(
    baseline: &[RunResult],
    candidate: &[RunResult],
    filter: Option<&str>,
    scorer: Option<&str>,
    limits: &Limits,
) -> Comparison {
    let selected = |r: &&RunResult| filter.is_none_or(|f| mira::glob_match(f, &r.key()));
    let a: BTreeMap<_, _> = baseline
        .iter()
        .filter(selected)
        .map(|r| (r.key(), r))
        .collect();
    let b: BTreeMap<_, _> = candidate
        .iter()
        .filter(selected)
        .map(|r| (r.key(), r))
        .collect();
    let keys: BTreeSet<_> = a.keys().chain(b.keys()).cloned().collect();
    let mut out = Comparison {
        cases: vec![],
        regressions: 0,
        improvements: 0,
        pass_rate_delta: 0.0,
        pass_rate_delta_ci95: None,
        score_deltas: BTreeMap::new(),
        baseline_cost_usd: 0.0,
        candidate_cost_usd: 0.0,
        baseline_duration_ms: 0,
        candidate_duration_ms: 0,
        incomplete: vec![],
        gate_failures: vec![],
    };
    let mut grouped: BTreeMap<String, Vec<(&RunResult, &RunResult)>> = BTreeMap::new();
    if keys.is_empty() {
        out.incomplete
            .push("no cases matched the comparison slice".into());
    }
    for key in keys {
        let (Some(left), Some(right)) = (a.get(&key), b.get(&key)) else {
            out.incomplete.push(format!("{key}: missing from one run"));
            continue;
        };
        if left.skipped || right.skipped || report::is_na(left) || report::is_na(right) {
            out.incomplete.push(format!("{key}: skipped or N/A"));
            continue;
        }
        let snapshot = |r: &RunResult| r.transcript.metadata.get(mira::provenance::SAMPLE).cloned();
        if snapshot(left).is_none()
            || snapshot(left) != snapshot(right)
            || left.seed != right.seed
            || left.trials != right.trials
        {
            out.incomplete.push(format!(
                "{key}: different or unverified samples, seeds, or trial counts"
            ));
            continue;
        }
        let scores = |r: &RunResult| {
            r.scores
                .iter()
                .filter(|s| scorer.is_none_or(|pattern| mira::glob_match(pattern, &s.scorer)))
                .map(|s| (s.scorer.clone(), s.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let ls = scores(left);
        let rs = scores(right);
        let score_count = |r: &RunResult| {
            r.scores
                .iter()
                .filter(|s| scorer.is_none_or(|pattern| mira::glob_match(pattern, &s.scorer)))
                .count()
        };
        if ls.is_empty()
            || ls.len() != score_count(left)
            || rs.len() != score_count(right)
            || ls.keys().ne(rs.keys())
            || ls.values().chain(rs.values()).any(|s| s.na)
        {
            out.incomplete.push(format!(
                "{key}: missing, duplicate, changed, or unavailable scorers"
            ));
            continue;
        }
        let definitions = |r: &RunResult| {
            r.transcript
                .metadata
                .get("mira.scorer_definitions")
                .and_then(|v| v.as_array())
                .map(|defs| {
                    defs.iter()
                        .filter(|definition| {
                            definition
                                .get("name")
                                .and_then(|v| v.as_str())
                                .is_some_and(|name| {
                                    scorer.is_none_or(|pattern| mira::glob_match(pattern, name))
                                })
                        })
                        .map(|definition| {
                            (
                                definition["name"].as_str().unwrap().to_owned(),
                                definition.clone(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                })
        };
        if definitions(left) != definitions(right) {
            out.incomplete
                .push(format!("{key}: scorer configurations changed"));
            continue;
        }
        grouped
            .entry(left.logical_key())
            .or_default()
            .push((left, right));
    }
    let pass = |r: &RunResult| {
        let s = r
            .scores
            .iter()
            .filter(|s| scorer.is_none_or(|p| mira::glob_match(p, &s.scorer)))
            .collect::<Vec<_>>();
        s.iter().all(|s| s.pass) as u8 as f64
    };
    for (key, pairs) in grouped {
        let expected = pairs[0].0.trials.max(1);
        let indices: BTreeSet<_> = pairs.iter().map(|(r, _)| r.trial).collect();
        if pairs.len() != expected || indices != (0..expected).collect() {
            out.incomplete.push(format!("{key}: incomplete trial set"));
            continue;
        }
        let n = pairs.len() as f64;
        let baseline_pass_rate = pairs.iter().map(|(a, _)| pass(a)).sum::<f64>() / n;
        let candidate_pass_rate = pairs.iter().map(|(_, b)| pass(b)).sum::<f64>() / n;
        let mut score_deltas = BTreeMap::new();
        for (a, b) in &pairs {
            out.baseline_cost_usd += a.transcript.usage.cost_usd;
            out.candidate_cost_usd += b.transcript.usage.cost_usd;
            out.baseline_duration_ms += a.transcript.timing.duration_ms;
            out.candidate_duration_ms += b.transcript.timing.duration_ms;
            for left in &a.scores {
                if scorer.is_some_and(|p| !mira::glob_match(p, &left.scorer)) {
                    continue;
                }
                let right = b.scores.iter().find(|s| s.scorer == left.scorer).unwrap();
                *score_deltas.entry(left.scorer.clone()).or_insert(0.0) +=
                    (right.value - left.value) / n;
            }
        }
        out.regressions += usize::from(candidate_pass_rate < baseline_pass_rate);
        out.improvements += usize::from(candidate_pass_rate > baseline_pass_rate);
        out.cases.push(CaseDelta {
            key,
            trials: pairs.len(),
            baseline_pass_rate,
            candidate_pass_rate,
            score_deltas,
        });
    }
    let deltas: Vec<_> = out
        .cases
        .iter()
        .map(|c| c.candidate_pass_rate - c.baseline_pass_rate)
        .collect();
    if !deltas.is_empty() {
        out.pass_rate_delta = deltas.iter().sum::<f64>() / deltas.len() as f64;
    }
    out.pass_rate_delta_ci95 = interval(&deltas);
    let mut scores: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for case in &out.cases {
        for (name, delta) in &case.score_deltas {
            scores.entry(name.clone()).or_default().push(*delta);
        }
    }
    out.score_deltas = scores
        .into_iter()
        .map(|(name, values)| (name, values.iter().sum::<f64>() / values.len() as f64))
        .collect();
    if limits
        .max_regressions
        .is_some_and(|max| out.regressions > max)
    {
        out.gate_failures
            .push(format!("{} regressions exceed limit", out.regressions));
    }
    if let Some(max) = limits.max_score_drop {
        for (name, delta) in &out.score_deltas {
            if *delta < -max {
                out.gate_failures
                    .push(format!("{name}: score decrease {} exceeds {max}", -delta));
            }
        }
    }
    for (name, a, b, max) in [
        (
            "cost",
            out.baseline_cost_usd,
            out.candidate_cost_usd,
            limits.max_cost_increase,
        ),
        (
            "latency",
            out.baseline_duration_ms as f64,
            out.candidate_duration_ms as f64,
            limits.max_latency_increase,
        ),
    ] {
        if let Some(max) = max
            && b > a * (1.0 + max)
        {
            out.gate_failures
                .push(format!("{name} increase exceeds {max}"));
        }
    }
    out
}

fn interval(values: &[f64]) -> Option<[f64; 2]> {
    if values.len() < 2 {
        return None;
    }
    let mut state = 0x6d697261_u64;
    let mut means = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let mut sum = 0.0;
        for _ in values {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            sum += values[((state >> 32) as usize) % values.len()];
        }
        means.push(sum / values.len() as f64);
    }
    means.sort_by(f64::total_cmp);
    Some([means[49], means[1949]])
}

#[cfg(test)]
pub fn fixture(sample: &str, pass: bool, trial: usize) -> RunResult {
    serde_json::from_value(serde_json::json!({"eval":"e","sample":sample,"target":"sim","trial":trial,"trials":2,"passed":pass,"aggregate":if pass {1.0} else {0.0},"scores":[{"scorer":"correct","value":if pass {1.0} else {0.0},"pass":pass,"reason":"test"}],"transcript":{"final_response":"answer","iterations":1,"tool_calls_count":0,"usage":{"input_tokens":0,"output_tokens":0,"cost_usd":0.01},"metadata":{"mira.sample":{"id":sample,"input":["question"],"expected":"answer"}}},"skipped":false})).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paired_comparison_groups_trials_and_gates_regressions() {
        let a = vec![
            fixture("a", true, 0),
            fixture("a", true, 1),
            fixture("b", false, 0),
            fixture("b", false, 1),
        ];
        let b = vec![
            fixture("a", false, 0),
            fixture("a", true, 1),
            fixture("b", true, 0),
            fixture("b", true, 1),
        ];
        let out = compare(
            &a,
            &b,
            None,
            None,
            &Limits {
                max_regressions: Some(0),
                ..Default::default()
            },
        );
        assert_eq!(out.cases.len(), 2);
        assert_eq!(out.regressions, 1);
        assert_eq!(out.improvements, 1);
        assert_eq!(out.pass_rate_delta, 0.25);
        assert!(out.pass_rate_delta_ci95.is_some());
        assert_eq!(out.exit_code(), 1);
        let missing = compare(&a, &b[..1], None, None, &Limits::default());
        assert_eq!(missing.exit_code(), 2);
    }
    #[test]
    fn duplicate_scorer_names_do_not_hide_conflicting_scores() {
        let mut a = fixture("a", true, 0);
        a.trials = 1;
        let mut b = a.clone();
        b.scores
            .push(mira::Score::fail("correct", "conflicting duplicate"));
        assert_eq!(
            compare(&[a], &[b], None, None, &Limits::default()).exit_code(),
            2
        );
    }

    #[test]
    fn incomplete_trial_sets_do_not_contribute_cost_totals() {
        let a = fixture("a", true, 0);
        let b = a.clone();
        let out = compare(&[a], &[b], None, None, &Limits::default());
        assert_eq!(out.exit_code(), 2);
        assert!(out.cases.is_empty());
        assert_eq!(out.baseline_cost_usd, 0.0);
        assert_eq!(out.candidate_cost_usd, 0.0);
    }

    #[test]
    fn changed_configuration_with_same_scorer_name_is_incomplete() {
        let mut a = fixture("a", true, 0);
        a.trials = 1;
        a.transcript.metadata.insert(
            "mira.scorer_definitions".into(),
            serde_json::json!([{"name":"correct","rubric":"old"}]),
        );
        let mut b = a.clone();
        b.transcript.metadata.insert(
            "mira.scorer_definitions".into(),
            serde_json::json!([{"name":"correct","rubric":"new"}]),
        );
        let out = compare(&[a], &[b], None, None, &Limits::default());
        assert_eq!(out.exit_code(), 2);
        assert!(out.incomplete[0].contains("scorer configurations changed"));
    }

    #[test]
    fn changed_references_and_judges_are_not_comparable() {
        let a = fixture("a", true, 0);
        let mut b = a.clone();
        b.transcript
            .metadata
            .get_mut(mira::provenance::SAMPLE)
            .unwrap()["expected"] = "different".into();
        assert_eq!(
            compare(
                std::slice::from_ref(&a),
                &[b],
                None,
                None,
                &Limits::default()
            )
            .exit_code(),
            2
        );
        let mut b = a.clone();
        b.scores[0].scorer = "new-rubric".into();
        assert_eq!(
            compare(&[a], &[b], None, None, &Limits::default()).exit_code(),
            2
        );
    }
}
