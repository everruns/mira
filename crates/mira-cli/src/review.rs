//! Evidence review stays an offline, self-contained artifact. Untrusted run
//! content is embedded as escaped JSON and rendered only with textContent;
//! labels are downloaded explicitly and never modify the source executions.
use clap::Args;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Args, Debug)]
pub struct ReviewArgs {
    /// Candidate saved run directory or ID.
    pub run: String,
    /// Optional baseline saved run for side-by-side evidence.
    #[arg(long)]
    pub baseline: Option<String>,
    /// Optional mira calibrate JSON report to seed reviewed human labels.
    #[arg(long)]
    pub calibration: Option<PathBuf>,
    /// Restrict cases to a case-key glob.
    #[arg(long)]
    pub filter: Option<String>,
    /// Standalone HTML destination.
    #[arg(long)]
    pub out: PathBuf,
}
fn evidence(
    run: &str,
    filter: Option<&str>,
) -> Result<BTreeMap<String, Value>, Box<dyn std::error::Error>> {
    let results = crate::compare::load(run)?;
    let dir = crate::compare::directory(run);
    let artifacts = dir.join("artifacts");
    let full: BTreeMap<_, _> = if artifacts.exists() {
        crate::load_artifacts_checked(&artifacts)?
            .into_iter()
            .map(|a| (a.key(), a))
            .collect()
    } else {
        BTreeMap::new()
    };
    let mut out = BTreeMap::new();
    for r in results
        .iter()
        .filter(|r| filter.is_none_or(|f| mira::glob_match(f, &r.key())))
    {
        let artifact = full.get(&r.key());
        out.insert(r.key(),json!({"result":r,"trajectory":artifact.and_then(|a|a.transcript.trajectory.as_ref()),"files":artifact.map(|a|&a.transcript.files),"sample":r.transcript.metadata.get(mira::provenance::SAMPLE)}));
    }
    Ok(out)
}
pub fn run(args: &ReviewArgs) -> Result<(), Box<dyn std::error::Error>> {
    let candidate = evidence(&args.run, args.filter.as_deref())?;
    if candidate.is_empty() {
        return Err("no candidate cases matched review slice".into());
    }
    let baseline = if let Some(run) = &args.baseline {
        evidence(run, args.filter.as_deref())?
    } else {
        BTreeMap::new()
    };
    let comparison = if let Some(run) = &args.baseline {
        Some(crate::compare::compare(
            &crate::compare::load(run)?,
            &crate::compare::load(&args.run)?,
            args.filter.as_deref(),
            None,
            &Default::default(),
        ))
    } else {
        None
    };
    let calibration: Value = if let Some(path) = &args.calibration {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        Value::Null
    };
    // Imported human decisions must identify these exact captured executions.
    if !calibration.is_null() {
        let judgments = calibration
            .get("judgments")
            .and_then(Value::as_array)
            .ok_or("calibration report has no judgments")?;
        if judgments.iter().any(|j| {
            j.get("case_key")
                .and_then(Value::as_str)
                .is_none_or(|key| !candidate.contains_key(key))
                || j.get("human_pass").and_then(Value::as_bool).is_none()
                || j.get("execution_sha256")
                    .and_then(Value::as_str)
                    .is_none_or(|hash| {
                        j.get("case_key")
                            .and_then(Value::as_str)
                            .and_then(|key| candidate.get(key))
                            .and_then(|c| {
                                c.pointer("/result/transcript/metadata/mira.execution_sha256")
                            })
                            .and_then(Value::as_str)
                            != Some(hash)
                    })
        }) {
            return Err(
                "calibration judgments must identify candidate cases with boolean human_pass"
                    .into(),
            );
        }
    }
    let data = json!({"run":args.run,"baseline_run":args.baseline,"candidate":candidate,"baseline":baseline,"comparison":comparison,"calibration":calibration});
    std::fs::write(&args.out, render(&data)?)?;
    eprintln!("evidence review: {}", args.out.display());
    Ok(())
}
fn render(data: &Value) -> Result<String, serde_json::Error> {
    let safe = serde_json::to_string(data)?
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    Ok(include_str!("review.html")
        .replace("__MIRA_REVIEW_SCRIPT__", include_str!("review.js"))
        .replace("__MIRA_REVIEW_DATA__", &safe))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_html_cannot_escape_the_json_container() {
        let html = render(
            &json!({"candidate":"</script><img src=x onerror=alert(1)>","unicode":"\u{2028}"}),
        )
        .unwrap();
        assert!(!html.contains("</script><img"));
        assert!(html.contains("\\u003c/script\\u003e"));
        let encoded = html
            .split("id=\"data\"")
            .nth(1)
            .unwrap()
            .split_once('>')
            .unwrap()
            .1
            .split("</script>")
            .next()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(encoded).unwrap()["candidate"],
            "</script><img src=x onerror=alert(1)>"
        );
    }
}
