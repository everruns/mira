//! Evaluation completeness is independent of target quality. Missing judges
//! remain N/A; a release gate can require evidence without blaming the target.

use crate::{protocol::RunResult, report};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
/// Optional host coverage gates, usable directly by library consumers.
///
/// ```
/// use mira::evaluation::CoveragePolicy;
/// let policy = CoveragePolicy { min_coverage: Some(1.0), ..Default::default() };
/// let outcome = policy.evaluate(&[]);
/// assert!(!outcome.complete);
/// assert_eq!(outcome.exit_code(), 2);
/// ```
pub struct CoveragePolicy {
    /// Minimum fraction of selected cases evaluated (0..1).
    pub min_coverage: Option<f64>,
    /// Maximum fraction of selected cases with infrastructure/N/A scores (0..1).
    pub max_infra_rate: Option<f64>,
    /// Scorer glob required to return a real score on every selected case; repeatable.
    #[serde(default)]
    pub require_scorer: Vec<String>,
}

fn fraction(value: &str) -> Result<f64, String> {
    let n: f64 = value.parse().map_err(|_| "expected a fraction in 0..1")?;
    if n.is_finite() && (0.0..=1.0).contains(&n) {
        Ok(n)
    } else {
        Err("expected a fraction in 0..1".into())
    }
}

impl CoveragePolicy {
    /// Apply explicit settings over fallback settings.
    pub fn overlay(&self, fallback: &Self) -> Self {
        Self {
            min_coverage: self.min_coverage.or(fallback.min_coverage),
            max_infra_rate: self.max_infra_rate.or(fallback.max_infra_rate),
            require_scorer: if self.require_scorer.is_empty() {
                fallback.require_scorer.clone()
            } else {
                self.require_scorer.clone()
            },
        }
    }

    /// Reject non-finite/out-of-range fractions and empty required-scorer globs.
    pub fn validate(&self) -> Result<(), String> {
        for n in [self.min_coverage, self.max_infra_rate]
            .into_iter()
            .flatten()
        {
            fraction(&n.to_string())?;
        }
        if self.require_scorer.iter().any(|s| s.trim().is_empty()) {
            return Err("required scorer cannot be empty".into());
        }
        Ok(())
    }

    /// Evaluate evidence separately from target quality. Validate authored policies
    /// with [`Self::validate`] before using them at an input boundary.
    pub fn evaluate(&self, results: &[RunResult]) -> Evaluation {
        let planned = results.len();
        let evaluated = results
            .iter()
            .filter(|r| {
                !r.skipped
                    && r.scores.iter().any(|s| !s.na)
                    && self.require_scorer.iter().all(|pattern| {
                        let matching: Vec<_> = r
                            .scores
                            .iter()
                            .filter(|s| crate::glob_match(pattern, &s.scorer))
                            .collect();
                        !matching.is_empty() && matching.iter().all(|s| !s.na)
                    })
            })
            .count();
        let infra = results
            .iter()
            .filter(|r| {
                !r.skipped
                    && (r.transcript.error_kind == crate::ErrorKind::Infra
                        || r.scores.iter().any(|s| s.na))
            })
            .count();
        let coverage = if planned == 0 {
            0.0
        } else {
            evaluated as f64 / planned as f64
        };
        let infra_rate = if planned == 0 {
            0.0
        } else {
            infra as f64 / planned as f64
        };
        let quality_passed = !results
            .iter()
            .any(|r| !r.skipped && !report::is_na(r) && !r.passed);
        let mut issues = Vec::new();
        if self.min_coverage.is_some_and(|min| coverage < min) {
            issues.push(format!(
                "coverage {coverage:.3} below minimum {}",
                self.min_coverage.unwrap()
            ));
        }
        if self.max_infra_rate.is_some_and(|max| infra_rate > max) {
            issues.push(format!(
                "infrastructure rate {infra_rate:.3} exceeds maximum {}",
                self.max_infra_rate.unwrap()
            ));
        }
        if !self.require_scorer.is_empty() && (evaluated != planned || planned == 0) {
            issues.push(format!(
                "required scorers evaluated on {evaluated}/{planned} cases"
            ));
        }
        Evaluation {
            policy: self.clone(),
            planned,
            evaluated,
            coverage,
            infra_rate,
            quality_passed,
            complete: planned > 0 && evaluated == planned && infra == 0,
            gates_passed: issues.is_empty(),
            gates_configured: self.min_coverage.is_some()
                || self.max_infra_rate.is_some()
                || !self.require_scorer.is_empty(),
            issues,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
/// Quality and evidence completeness for one selected set of case results.
/// Persist this alongside results so renderers and publishers use the same verdict.
pub struct Evaluation {
    /// The policy that produced this outcome; reused when rendering a slice.
    pub policy: CoveragePolicy,
    /// Selected cases, including skipped and unavailable cases.
    pub planned: usize,
    /// Cases with real scores and every required scorer available.
    pub evaluated: usize,
    /// Evaluated / planned; zero for an empty selection.
    pub coverage: f64,
    /// Fraction of selected cases with N/A scorers or infrastructure errors.
    pub infra_rate: f64,
    /// No applicable target score failed; check completeness separately.
    pub quality_passed: bool,
    /// Every selected case was evaluated without skips or N/A evidence.
    pub complete: bool,
    /// The configured coverage requirements were satisfied.
    pub gates_passed: bool,
    /// At least one coverage requirement was explicitly configured.
    pub gates_configured: bool,
    /// Human-readable reasons why coverage gates failed.
    pub issues: Vec<String>,
}

impl Evaluation {
    /// CI status: 2 for unmet coverage gates, 1 for target failures, 0 otherwise.
    pub fn exit_code(&self) -> i32 {
        if !self.gates_passed {
            2
        } else if !self.quality_passed {
            1
        } else {
            0
        }
    }
}
