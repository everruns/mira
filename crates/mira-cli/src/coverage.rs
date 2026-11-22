//! Clap adapter for the shared coverage policy. Parsing stays host-side; the
//! provider-independent core owns evaluation and gate semantics.

use clap::Args;
use mira::protocol::RunResult;
use serde::{Deserialize, Serialize};

#[derive(Args, Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Minimum fraction of selected cases evaluated (0..1).
    #[arg(long, value_parser = fraction)]
    pub min_coverage: Option<f64>,
    /// Maximum fraction of selected cases with infrastructure/N/A scores (0..1).
    #[arg(long, value_parser = fraction)]
    pub max_infra_rate: Option<f64>,
    /// Scorer glob required to return a real score on every selected case; repeatable.
    #[arg(long = "require-scorer")]
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

impl Policy {
    fn core(&self) -> mira::evaluation::CoveragePolicy {
        mira::evaluation::CoveragePolicy {
            min_coverage: self.min_coverage,
            max_infra_rate: self.max_infra_rate,
            require_scorer: self.require_scorer.clone(),
        }
    }
    pub fn overlay(&self, fallback: &Self) -> Self {
        let policy = self.core().overlay(&fallback.core());
        Self {
            min_coverage: policy.min_coverage,
            max_infra_rate: policy.max_infra_rate,
            require_scorer: policy.require_scorer,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.core().validate()
    }
    pub fn evaluate(&self, results: &[RunResult]) -> mira::evaluation::Evaluation {
        self.core().evaluate(results)
    }
}

pub fn print(outcome: &mira::evaluation::Evaluation) {
    eprintln!(
        "quality: {}; evaluation: {} ({}/{} cases); coverage gates: {}",
        if outcome.quality_passed {
            "passed"
        } else {
            "failed"
        },
        if outcome.complete {
            "complete"
        } else {
            "incomplete"
        },
        outcome.evaluated,
        outcome.planned,
        if !outcome.gates_configured {
            "not configured"
        } else if outcome.gates_passed {
            "passed"
        } else {
            "failed"
        }
    );
    for issue in &outcome.issues {
        eprintln!("  {issue}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_matching_required_scorer_must_be_available() {
        let mut r = crate::compare::fixture("a", true, 0);
        r.scores = vec![
            mira::Score::pass("judge:facts", "ok"),
            mira::Score::na("judge:tools", "offline"),
        ];
        let policy = Policy {
            require_scorer: vec!["judge:*".into()],
            ..Default::default()
        };
        assert_eq!(policy.evaluate(&[r]).exit_code(), 2);
    }

    #[test]
    fn required_judge_cannot_disappear_behind_passing_checks() {
        let r: RunResult = serde_json::from_value(serde_json::json!({"eval":"e","sample":"s","target":"sim","passed":true,"aggregate":1.0,"scores":[
            {"scorer":"succeeded","value":1.0,"pass":true,"reason":"ok"},
            {"scorer":"judge:v1","value":0.0,"pass":false,"na":true,"reason":"offline"}],
            "transcript":{"final_response":"ok","iterations":1,"tool_calls_count":0,"usage":{"input_tokens":0,"output_tokens":0,"cost_usd":0}},"skipped":false})).unwrap();
        assert_eq!(
            Policy::default()
                .evaluate(std::slice::from_ref(&r))
                .exit_code(),
            0
        );
        assert!(
            !Policy::default()
                .evaluate(std::slice::from_ref(&r))
                .complete
        );
        let policy = Policy {
            min_coverage: Some(1.0),
            max_infra_rate: Some(0.0),
            require_scorer: vec!["judge:*".into()],
        };
        let outcome = policy.evaluate(&[r]);
        assert!(outcome.quality_passed);
        assert_eq!(outcome.exit_code(), 2);
        assert_eq!(outcome.evaluated, 0);
        assert_eq!(policy.evaluate(&[]).exit_code(), 2);
        assert!(fraction("NaN").is_err());
        assert!(fraction("1.1").is_err());
    }
}
