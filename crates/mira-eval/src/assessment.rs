//! Absolute acceptance policies operate on complete saved reports. Trial sets
//! receive equal logical-case weight; unavailable evidence never becomes zero
//! cost or a passing empty slice. Policies are data, separate from scorer code.
use crate::{protocol::RunResult, provenance::SAMPLE};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Versioned acceptance policy, loadable from TOML or JSON by a host.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Independent named slices; every rule must pass.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Declared scenario × quality-dimension requirements.
    pub inventory: Option<Inventory>,
}
/// One slice's absolute quality and operational requirements.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Unique policy rule name.
    pub name: String,
    /// Glob over the full execution case key.
    pub filter: Option<String>,
    /// Require this archived sample tag.
    pub tag: Option<String>,
    /// Score dimension glob; all matches must be available on each case.
    pub scorer: Option<String>,
    /// Minimum number of complete logical cases (default 1).
    #[serde(default = "one")]
    pub min_cases: usize,
    /// Minimum equally weighted logical-case pass rate.
    pub min_pass_rate: Option<f64>,
    /// Minimum mean selected scorer value.
    pub min_score: Option<f64>,
    /// Nearest-rank p95 subject duration across selected trials.
    pub max_p95_latency_ms: Option<u64>,
    /// Maximum total subject cost across selected trials.
    pub max_subject_cost_usd: Option<f64>,
    /// Maximum selected scorer cost; unpriced/unreported scoring is incomplete.
    pub max_grading_cost_usd: Option<f64>,
}
fn one() -> usize {
    1
}
/// Expected test categories, independent of evidence-availability coverage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    /// Declared scenario values in archived sample metadata.
    pub scenarios: Vec<String>,
    /// Declared quality dimensions in archived sample metadata.
    pub dimensions: Vec<String>,
    /// Complete logical cases required in every cross-product cell.
    #[serde(default = "one")]
    pub min_cases: usize,
}
/// Report-level policy result, independent of how studies were authored.
#[derive(Debug, Serialize)]
pub struct Assessment {
    /// The exact policy evaluated, archived in the report.
    pub policy: Policy,
    /// One result per rule, including actual measured values.
    pub rules: Vec<RuleResult>,
    /// Includes empty cells in the declared scenario × dimension cross product.
    pub inventory: Vec<Cell>,
    /// Selected cases with missing or undeclared category metadata.
    pub unclassified: Vec<String>,
}
/// Actual measurements and acceptance failures for a slice.
#[derive(Debug, Serialize)]
pub struct RuleResult {
    /// Policy rule name.
    pub name: String,
    /// Number of selected logical cases, including incomplete ones.
    pub cases: usize,
    /// Equally weighted mean of logical-case trial pass rates.
    pub pass_rate: Option<f64>,
    /// Equally weighted mean selected score per logical case.
    pub mean_score: Option<f64>,
    /// Nearest-rank p95 duration over executed trials.
    pub p95_latency_ms: Option<u64>,
    /// Total subject cost.
    pub subject_cost_usd: f64,
    /// Total grading cost, absent if any selected score is unpriced/unreported.
    pub grading_cost_usd: Option<f64>,
    /// Missing or unusable evidence, which blocks acceptance separately.
    pub incomplete: Vec<String>,
    /// Acceptance criteria not met by available evidence.
    pub failures: Vec<String>,
}
/// One declared coverage requirement.
#[derive(Debug, Serialize)]
pub struct Cell {
    /// Expected scenario.
    pub scenario: String,
    /// Expected quality dimension.
    pub dimension: String,
    /// Unique logical cases that exercise this category.
    pub planned: usize,
    /// Logical cases with complete scored trial evidence.
    pub evaluated: usize,
    /// Whether the configured minimum is met.
    pub covered: bool,
}
impl Assessment {
    /// 2 = incomplete evidence/coverage; 1 = acceptance breach; 0 = accepted.
    pub fn exit_code(&self) -> i32 {
        if self.rules.iter().any(|r| !r.incomplete.is_empty())
            || self.inventory.iter().any(|c| !c.covered)
            || !self.unclassified.is_empty()
        {
            2
        } else if self.rules.iter().any(|r| !r.failures.is_empty()) {
            1
        } else {
            0
        }
    }
}
impl Policy {
    /// Reject ambiguous names, empty policies, and non-finite/out-of-range limits.
    pub fn validate(&self) -> Result<(), String> {
        if self.rules.is_empty() && self.inventory.is_none() {
            return Err("policy needs rules or inventory".into());
        }
        let mut names = BTreeSet::new();
        for r in &self.rules {
            if r.name.trim().is_empty() || !names.insert(&r.name) || r.min_cases == 0 {
                return Err("rule names must be unique/nonempty and min_cases positive".into());
            }
            if [r.min_pass_rate, r.min_score]
                .into_iter()
                .flatten()
                .any(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
                || [r.max_subject_cost_usd, r.max_grading_cost_usd]
                    .into_iter()
                    .flatten()
                    .any(|n| !n.is_finite() || n < 0.0)
            {
                return Err(format!("{}: invalid limit", r.name));
            }
            if [r.filter.as_ref(), r.tag.as_ref(), r.scorer.as_ref()]
                .into_iter()
                .flatten()
                .any(|s| s.trim().is_empty())
            {
                return Err(format!("{}: selectors must not be empty", r.name));
            }
            if r.min_pass_rate.is_none()
                && r.min_score.is_none()
                && r.max_p95_latency_ms.is_none()
                && r.max_subject_cost_usd.is_none()
                && r.max_grading_cost_usd.is_none()
            {
                return Err(format!("{}: rule needs an acceptance limit", r.name));
            }
        }
        if let Some(i) = &self.inventory {
            for values in [&i.scenarios, &i.dimensions] {
                if values.is_empty()
                    || values.iter().any(|s| s.trim().is_empty())
                    || values.iter().collect::<BTreeSet<_>>().len() != values.len()
                {
                    return Err("inventory categories must be nonempty and unique".into());
                }
            }
            if i.min_cases == 0 {
                return Err("inventory min_cases must be positive".into());
            }
        }
        Ok(())
    }
    /// Evaluate every rule and every inventory cell without mutating the run.
    pub fn evaluate(&self, results: &[RunResult]) -> Result<Assessment, String> {
        self.validate()?;
        let mut out = Assessment {
            policy: self.clone(),
            rules: self
                .rules
                .iter()
                .map(|r| evaluate_rule(r, results))
                .collect(),
            inventory: vec![],
            unclassified: vec![],
        };
        if let Some(i) = &self.inventory {
            for scenario in &i.scenarios {
                for dimension in &i.dimensions {
                    let selected: Vec<_> = results
                        .iter()
                        .filter(|r| {
                            categories(r).is_some_and(|(s, ds)| {
                                s == scenario && ds.contains(&dimension.as_str())
                            })
                        })
                        .collect();
                    let groups = groups(&selected);
                    let evaluated = groups.values().filter(|g| complete(g)).count();
                    out.inventory.push(Cell {
                        scenario: scenario.clone(),
                        dimension: dimension.clone(),
                        planned: groups.len(),
                        evaluated,
                        covered: evaluated >= i.min_cases,
                    });
                }
            }
            out.unclassified = results
                .iter()
                .filter(|r| {
                    categories(r).is_none_or(|(s, ds)| {
                        !i.scenarios.iter().any(|x| x == s)
                            || ds.is_empty()
                            || ds.iter().any(|d| !i.dimensions.iter().any(|x| x == d))
                    })
                })
                .map(RunResult::logical_key)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        Ok(out)
    }
}
fn categories(r: &RunResult) -> Option<(&str, Vec<&str>)> {
    let m = r.transcript.metadata.get(SAMPLE)?.get("metadata")?;
    let s = m.get("scenario")?.as_str()?;
    let ds = m
        .get("dimensions")?
        .as_array()?
        .iter()
        .map(|v| v.as_str())
        .collect::<Option<Vec<_>>>()?;
    Some((s, ds))
}
fn groups<'a>(results: &[&'a RunResult]) -> BTreeMap<String, Vec<&'a RunResult>> {
    let mut groups = BTreeMap::new();
    for r in results {
        groups
            .entry(r.logical_key())
            .or_insert_with(Vec::new)
            .push(*r);
    }
    groups
}
fn complete(group: &[&RunResult]) -> bool {
    let n = group[0].trials.max(1);
    group.len() == n
        && group.iter().map(|r| r.trial).collect::<BTreeSet<_>>() == (0..n).collect()
        && group.iter().all(|r| {
            r.trials.max(1) == n
                && !r.skipped
                && r.transcript.error_kind != crate::ErrorKind::Infra
                && !r.scores.is_empty()
                && r.scores
                    .iter()
                    .all(|s| !s.na && s.value.is_finite() && (0.0..=1.0).contains(&s.value))
        })
}
fn evaluate_rule(rule: &Rule, results: &[RunResult]) -> RuleResult {
    let selected: Vec<_> = results
        .iter()
        .filter(|r| {
            rule.filter
                .as_ref()
                .is_none_or(|f| crate::glob_match(f, &r.key()))
                && rule.tag.as_ref().is_none_or(|tag| {
                    r.transcript
                        .metadata
                        .get(SAMPLE)
                        .and_then(|s| s.get("tags"))
                        .and_then(|t| t.as_array())
                        .is_some_and(|ts| ts.iter().any(|t| t.as_str() == Some(tag)))
                })
        })
        .collect();
    let grouped = groups(&selected);
    let mut out = RuleResult {
        name: rule.name.clone(),
        cases: grouped.len(),
        pass_rate: None,
        mean_score: None,
        p95_latency_ms: None,
        subject_cost_usd: 0.0,
        grading_cost_usd: Some(0.0),
        incomplete: vec![],
        failures: vec![],
    };
    if grouped.len() < rule.min_cases {
        out.incomplete.push(format!(
            "{} logical cases; need {}",
            grouped.len(),
            rule.min_cases
        ));
    }
    let mut rates = Vec::new();
    let mut means = Vec::new();
    let mut times = Vec::new();
    for (key, group) in &grouped {
        if !complete(group) {
            out.incomplete
                .push(format!("{key}: incomplete trial/scoring evidence"));
        }
        let mut passes = 0;
        let mut values = Vec::new();
        for r in group {
            let scores: Vec<_> = r
                .scores
                .iter()
                .filter(|s| {
                    rule.scorer
                        .as_ref()
                        .is_none_or(|p| crate::glob_match(p, &s.scorer))
                })
                .collect();
            if scores.is_empty() || scores.iter().any(|s| s.na) {
                out.incomplete
                    .push(format!("{}: required scores unavailable", r.key()));
            }
            passes += usize::from(!scores.is_empty() && scores.iter().all(|s| !s.na && s.pass));
            if !scores.is_empty() {
                values.push(scores.iter().map(|s| s.value).sum::<f64>() / scores.len() as f64);
            }
            for s in scores {
                out.grading_cost_usd = out
                    .grading_cost_usd
                    .zip(
                        s.grading
                            .as_ref()
                            .and_then(|g| g.cost_usd)
                            .filter(|v| v.is_finite() && *v >= 0.0),
                    )
                    .map(|(a, b)| a + b);
            }
            if !r.skipped {
                times.push(r.transcript.timing.duration_ms);
                let cost = r.transcript.usage.cost_usd;
                if !cost.is_finite() || cost < 0.0 {
                    out.incomplete
                        .push(format!("{}: invalid subject cost", r.key()));
                } else {
                    out.subject_cost_usd += cost;
                }
            }
        }
        rates.push(passes as f64 / group.len() as f64);
        if !values.is_empty() {
            means.push(values.iter().sum::<f64>() / values.len() as f64);
        }
    }
    if !rates.is_empty() {
        out.pass_rate = Some(rates.iter().sum::<f64>() / rates.len() as f64);
    }
    if !means.is_empty() {
        out.mean_score = Some(means.iter().sum::<f64>() / means.len() as f64);
    }
    if !times.is_empty() {
        times.sort_unstable();
        out.p95_latency_ms = Some(times[(times.len() * 95).div_ceil(100) - 1]);
    }
    for (label, actual, min) in [
        ("pass rate", out.pass_rate, rule.min_pass_rate),
        ("mean score", out.mean_score, rule.min_score),
    ] {
        if let Some(min) = min {
            if actual.is_some_and(|v| v < min) {
                out.failures.push(format!("{label} below {min}"));
            }
        }
    }
    if rule
        .max_p95_latency_ms
        .is_some_and(|max| out.p95_latency_ms.is_some_and(|n| n > max))
    {
        out.failures.push("p95 latency exceeds limit".into());
    }
    if rule
        .max_subject_cost_usd
        .is_some_and(|max| out.subject_cost_usd > max)
    {
        out.failures.push("subject cost exceeds limit".into());
    }
    if let Some(max) = rule.max_grading_cost_usd {
        match out.grading_cost_usd {
            None => out.incomplete.push(
                "grading cost is unreported or unpriced; scope scorer to measured judges".into(),
            ),
            Some(n) if n > max => out.failures.push("grading cost exceeds limit".into()),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn result(id: &str, trial: usize, pass: bool) -> RunResult {
        serde_json::from_value(json!({"eval":"e","sample":id,"target":"sim","trial":trial,"trials":2,"passed":pass,"aggregate":if pass {1.0}else{0.0},"scores":[{"scorer":"correct","value":if pass{1.0}else{0.0},"pass":pass,"reason":"fixture"}],"transcript":{"final_response":"answer","iterations":1,"tool_calls_count":0,"timing":{"duration_ms":20},"usage":{"input_tokens":0,"output_tokens":0,"cost_usd":0.1},"metadata":{"mira.sample":{"id":id,"input":["q"],"tags":["safety"],"metadata":{"scenario":"tools","dimensions":["safety"]}}}},"skipped":false})).unwrap()
    }
    fn policy() -> Policy {
        serde_json::from_value(json!({"rules":[{"name":"safety","tag":"safety","min_pass_rate":1.0}],"inventory":{"scenarios":["tools"],"dimensions":["safety","robustness"]}})).unwrap()
    }
    #[test]
    fn absolute_gates_preserve_missing_cells_and_trials() {
        let p = policy();
        let rs = vec![result("a", 0, true), result("a", 1, false)];
        let out = p.evaluate(&rs).unwrap();
        assert_eq!(out.rules[0].pass_rate, Some(0.5));
        assert!(!out.rules[0].failures.is_empty());
        assert_eq!(out.inventory[1].planned, 0);
        assert_eq!(out.exit_code(), 2);
        let mut p = p;
        p.inventory = None;
        assert_eq!(p.evaluate(&rs).unwrap().exit_code(), 1);
        assert_eq!(p.evaluate(&rs[..1]).unwrap().exit_code(), 2);
        assert_eq!(p.evaluate(&[]).unwrap().exit_code(), 2);
        assert_eq!(
            p.evaluate(&[rs[0].clone(), rs[0].clone()])
                .unwrap()
                .exit_code(),
            2
        );
    }
    #[test]
    fn logical_cases_are_weighted_equally_and_operational_limits_gate() {
        let mut p = policy();
        p.inventory = None;
        p.rules[0].min_pass_rate = Some(0.5);
        p.rules[0].min_score = Some(0.5);
        p.rules[0].max_p95_latency_ms = Some(50);
        p.rules[0].max_subject_cost_usd = Some(1.0);
        let mut a = result("a", 0, true);
        a.trials = 1;
        let mut b = result("b", 0, false);
        b.trials = 3;
        let mut c = b.clone();
        c.trial = 1;
        let mut d = b.clone();
        d.trial = 2;
        d.transcript.timing.duration_ms = 100;
        let out = p.evaluate(&[a, b, c, d]).unwrap();
        assert_eq!(out.rules[0].pass_rate, Some(0.5));
        assert_eq!(out.rules[0].mean_score, Some(0.5));
        assert_eq!(out.rules[0].p95_latency_ms, Some(100));
        assert_eq!(out.exit_code(), 1);
        assert_eq!(out.rules[0].failures.len(), 1);
        p.rules[0].max_subject_cost_usd = Some(0.1);
        assert!(p.validate().is_ok());
        p.rules.push(p.rules[0].clone());
        assert!(p.validate().is_err());
    }

    #[test]
    fn repetitions_cannot_fill_missing_inventory_cases() {
        let mut p = policy();
        p.rules.clear();
        p.inventory.as_mut().unwrap().dimensions = vec!["safety".into()];
        p.inventory.as_mut().unwrap().min_cases = 2;
        let mut rs = vec![result("a", 0, true), result("a", 1, true)];
        let out = p.evaluate(&rs).unwrap();
        assert_eq!(out.inventory[0].evaluated, 1);
        assert_eq!(out.exit_code(), 2);
        rs[0].transcript.metadata.remove(SAMPLE);
        assert!(!p.evaluate(&rs).unwrap().unclassified.is_empty());
    }

    #[test]
    fn cost_gates_require_prices_and_slices_require_matches() {
        let mut p = policy();
        p.inventory = None;
        p.rules[0].max_grading_cost_usd = Some(0.01);
        let rs = vec![result("a", 0, true), result("a", 1, true)];
        assert_eq!(p.evaluate(&rs).unwrap().exit_code(), 2);
        let mut rs = rs;
        for r in &mut rs {
            r.scores[0].grading = Some(crate::grading::GradingUsage {
                cost_usd: Some(0.02),
                ..Default::default()
            });
        }
        assert_eq!(p.evaluate(&rs).unwrap().exit_code(), 1);
        p.rules[0].filter = Some("missing/*".into());
        assert_eq!(p.evaluate(&rs).unwrap().exit_code(), 2);
        p.rules[0].min_score = Some(f64::NAN);
        assert!(p.validate().is_err());
    }
}
