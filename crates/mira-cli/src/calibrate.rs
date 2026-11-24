//! Human labels grade the judge, not the subject. Missing judge verdicts stay
//! missing evidence, and never inflate agreement by disappearing from reports.

use clap::Args;
use mira::protocol::RunResult;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Args, Debug)]
pub struct CalibrateArgs {
    pub run: String,
    /// JSONL human labels: {"case_key":"eval/sample@target#0","pass":true}.
    #[arg(long)]
    pub labels: PathBuf,
    /// Exactly one matching scorer per labeled case.
    #[arg(long)]
    pub scorer: String,
    #[arg(long)]
    pub min_agreement: Option<f64>,
    #[arg(long)]
    pub max_false_pass_rate: Option<f64>,
    /// Optional earlier grading of the same captured executions.
    #[arg(long)]
    pub previous: Option<String>,
    /// Earlier scorer identity; defaults to --scorer.
    #[arg(long, requires = "previous")]
    pub previous_scorer: Option<String>,
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub case_key: String,
    pub pass: bool,
    #[serde(default)]
    pub execution_sha256: Option<String>,
}

#[derive(Default, Debug, Serialize)]
pub struct Calibration {
    pub labeled: usize,
    pub evaluated: usize,
    pub agreement: Option<f64>,
    pub false_pass_rate: Option<f64>,
    pub true_pass: usize,
    pub true_fail: usize,
    pub false_pass: usize,
    pub false_fail: usize,
    pub disagreements: Vec<String>,
    pub unavailable: Vec<String>,
    pub verdict_flips: Vec<String>,
    pub previous_agreement: Option<f64>,
    pub judgments: Vec<Judgment>,
    pub gate_failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Judgment {
    pub case_key: String,
    pub human_pass: bool,
    pub execution_sha256: Option<String>,
    pub judge_pass: Option<bool>,
    pub previous_pass: Option<bool>,
}

fn verdict(result: &RunResult, pattern: &str) -> Option<bool> {
    if result.skipped {
        return None;
    }
    let matches: Vec<_> = result
        .scores
        .iter()
        .filter(|s| mira::glob_match(pattern, &s.scorer))
        .collect();
    if matches.len() != 1 || matches[0].na {
        None
    } else {
        Some(matches[0].pass)
    }
}

pub fn evaluate(results: &[RunResult], labels: &[Label], scorer: &str) -> Calibration {
    let by_key: BTreeMap<_, _> = results.iter().map(|r| (r.key(), r)).collect();
    let mut out = Calibration {
        labeled: labels.len(),
        ..Default::default()
    };
    for label in labels {
        let identity_matches = label.execution_sha256.as_ref().is_none_or(|hash| {
            by_key
                .get(&label.case_key)
                .and_then(|r| r.transcript.metadata.get("mira.execution_sha256"))
                .and_then(|v| v.as_str())
                == Some(hash.as_str())
        });
        out.judgments.push(Judgment {
            case_key: label.case_key.clone(),
            human_pass: label.pass,
            execution_sha256: if identity_matches {
                by_key
                    .get(&label.case_key)
                    .and_then(|r| r.transcript.metadata.get("mira.execution_sha256"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            } else {
                label.execution_sha256.clone()
            },
            judge_pass: by_key
                .get(&label.case_key)
                .filter(|_| identity_matches)
                .and_then(|r| verdict(r, scorer)),
            previous_pass: None,
        });
        let Some(actual) = by_key
            .get(&label.case_key)
            .filter(|_| identity_matches)
            .and_then(|r| verdict(r, scorer))
        else {
            out.unavailable.push(label.case_key.clone());
            continue;
        };
        out.evaluated += 1;
        match (label.pass, actual) {
            (true, true) => out.true_pass += 1,
            (false, false) => out.true_fail += 1,
            (false, true) => out.false_pass += 1,
            (true, false) => out.false_fail += 1,
        }
        if label.pass != actual {
            out.disagreements.push(label.case_key.clone());
        }
    }
    if out.evaluated > 0 {
        out.agreement = Some((out.true_pass + out.true_fail) as f64 / out.evaluated as f64);
    }
    if out.true_fail + out.false_pass > 0 {
        out.false_pass_rate = Some(out.false_pass as f64 / (out.true_fail + out.false_pass) as f64);
    }
    out
}

pub fn run(args: &CalibrateArgs) -> Result<i32, Box<dyn std::error::Error>> {
    for value in [args.min_agreement, args.max_false_pass_rate]
        .into_iter()
        .flatten()
    {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("calibration thresholds must be in 0..1".into());
        }
    }
    let labels: Vec<Label> = std::fs::read_to_string(&args.labels)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let keys: std::collections::BTreeSet<_> = labels.iter().map(|l| &l.case_key).collect();
    if labels.is_empty() || keys.len() != labels.len() {
        return Err("labels must be nonempty with unique case_key values".into());
    }
    let results = crate::compare::load(&args.run)?;
    let mut report = evaluate(&results, &labels, &args.scorer);
    if let Some(previous) = &args.previous {
        let previous = crate::compare::load(previous)?;
        report.previous_agreement = evaluate(
            &previous,
            &labels,
            args.previous_scorer.as_deref().unwrap_or(&args.scorer),
        )
        .agreement;
        let previous: BTreeMap<_, _> = previous.iter().map(|r| (r.key(), r)).collect();
        for r in results.iter().filter(|r| keys.contains(&r.key())) {
            let Some(old) = previous.get(&r.key()) else {
                report
                    .unavailable
                    .push(format!("{}: previous result missing", r.key()));
                continue;
            };
            // Repeated grading must use the same executed evidence. New runs
            // cannot masquerade as a rubric-only comparison.
            let identity = r.transcript.metadata.get("mira.execution_sha256");
            if identity.is_none()
                || identity != old.transcript.metadata.get("mira.execution_sha256")
            {
                report.unavailable.push(format!(
                    "{}: execution identity differs or is unverified",
                    r.key()
                ));
                continue;
            }
            if let Some(row) = report
                .judgments
                .iter_mut()
                .find(|row| row.case_key == r.key())
            {
                row.previous_pass =
                    verdict(old, args.previous_scorer.as_deref().unwrap_or(&args.scorer));
            }
            match (
                verdict(old, args.previous_scorer.as_deref().unwrap_or(&args.scorer)),
                verdict(r, &args.scorer),
            ) {
                (Some(a), Some(b)) if a != b => report.verdict_flips.push(r.key()),
                (None, _) | (_, None) => report
                    .unavailable
                    .push(format!("{}: unavailable old/new verdict", r.key())),
                _ => {}
            }
        }
    }
    if args
        .min_agreement
        .is_some_and(|min| report.agreement.is_none_or(|v| v < min))
    {
        report
            .gate_failures
            .push("agreement below threshold or unavailable".into());
    }
    if args
        .max_false_pass_rate
        .is_some_and(|max| report.false_pass_rate.is_none_or(|v| v > max))
    {
        report
            .gate_failures
            .push("false-pass rate above threshold or no negative labels".into());
    }
    eprintln!(
        "calibration: {}/{} labeled outputs evaluated; {} disagreements; {} verdict flips",
        report.evaluated,
        report.labeled,
        report.disagreements.len(),
        report.verdict_flips.len()
    );
    let bytes = serde_json::to_vec_pretty(&report)?;
    if let Some(out) = &args.out {
        std::fs::write(out, bytes)?;
    } else {
        println!("{}", String::from_utf8(bytes)?);
    }
    Ok(if !report.unavailable.is_empty() {
        2
    } else if !report.gate_failures.is_empty() {
        1
    } else {
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mismatched_label_identity_cannot_be_rebound_in_review_report() {
        let mut result = crate::compare::fixture("a", true, 0);
        result
            .transcript
            .metadata
            .insert("mira.execution_sha256".into(), "actual".into());
        let label = Label {
            case_key: result.key(),
            pass: true,
            execution_sha256: Some("other".into()),
        };
        let report = evaluate(&[result], &[label], "correct");
        assert_eq!(report.evaluated, 0);
        assert_eq!(report.judgments[0].judge_pass, None);
        assert_eq!(
            report.judgments[0].execution_sha256.as_deref(),
            Some("other")
        );
    }
    #[test]
    fn confusion_matrix_keeps_missing_verdicts_visible() {
        let a = crate::compare::fixture("a", true, 0);
        let b = crate::compare::fixture("b", false, 0);
        let labels = vec![
            Label {
                execution_sha256: None,
                case_key: a.key(),
                pass: false,
            },
            Label {
                execution_sha256: None,
                case_key: b.key(),
                pass: false,
            },
            Label {
                execution_sha256: None,
                case_key: "missing".into(),
                pass: true,
            },
        ];
        let out = evaluate(&[a, b], &labels, "correct");
        assert_eq!(out.false_pass, 1);
        assert_eq!(out.true_fail, 1);
        assert_eq!(out.agreement, Some(0.5));
        assert_eq!(out.false_pass_rate, Some(0.5));
        assert_eq!(out.unavailable, vec!["missing"]);
    }
}
