//! Failures produce review records, not executable datasets. A reviewer must
//! inspect the evidence and extract the nested `sample` after approving its
//! reference; the wrapper deliberately cannot deserialize as a Sample.

use clap::Args;
use mira::protocol::RunResult;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Args, Debug)]
pub struct FailureArgs {
    pub run: String,
    #[arg(long)]
    pub filter: Option<String>,
    /// Include only failures of a matching scorer.
    #[arg(long)]
    pub scorer: Option<String>,
    /// Review-record JSONL destination; '-' writes stdout.
    #[arg(long)]
    pub out: String,
}

pub fn drafts(
    results: &[RunResult],
    run: &str,
    filter: Option<&str>,
    scorer: Option<&str>,
) -> Vec<Value> {
    let mut drafts: BTreeMap<String, Value> = BTreeMap::new();
    for r in results {
        if r.skipped
            || filter.is_some_and(|f| !mira::glob_match(f, &r.key()))
            || !r
                .scores
                .iter()
                .any(|s| !s.na && !s.pass && scorer.is_none_or(|p| mira::glob_match(p, &s.scorer)))
        {
            continue;
        }
        let sample = r
            .transcript
            .metadata
            .get(mira::provenance::SAMPLE)
            .cloned()
            .unwrap_or_else(|| json!({"id":r.sample,"input":r.input,"expected":r.expected}));
        let hash = mira::provenance::fingerprint(&serde_json::to_vec(&sample).unwrap());
        let key = format!("{}/{hash}", r.eval);
        let draft = drafts.entry(key).or_insert_with(|| json!({"status":"needs_review","sample":sample,"review_notes":"","source_run":run,"eval":r.eval,"evidence":[]}));
        draft["evidence"].as_array_mut().unwrap().push(json!({"case_key":r.key(),"final_response":r.transcript.final_response,"scores":r.scores,"execution_sha256":r.transcript.metadata.get("mira.execution_sha256")}));
    }
    drafts.into_values().collect()
}

pub fn run(args: &FailureArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut records = drafts(
        &crate::compare::load(&args.run)?,
        &args.run,
        args.filter.as_deref(),
        args.scorer.as_deref(),
    );
    let path = PathBuf::from(&args.run);
    let dir = if path.is_dir() {
        path
    } else {
        crate::config::run_dir(&crate::config::Config::load().results_dir(), &args.run)
    };
    for record in &mut records {
        for evidence in record["evidence"].as_array_mut().unwrap() {
            let key = evidence["case_key"].as_str().unwrap();
            let path = super::artifact_path(&dir.join("artifacts").to_string_lossy(), key);
            if path.exists() {
                let artifact: mira::protocol::ExecuteResult =
                    serde_json::from_slice(&std::fs::read(path)?)?;
                evidence["trajectory"] = serde_json::to_value(&artifact.transcript.trajectory)?;
                evidence["captured_files"] = serde_json::to_value(&artifact.transcript.files)?;
            }
        }
    }
    let mut output = String::new();
    for record in &records {
        output.push_str(&serde_json::to_string(record)?);
        output.push('\n');
    }
    if args.out == "-" {
        print!("{output}");
    } else {
        std::fs::write(&args.out, output)?;
    }
    eprintln!(
        "{} draft case(s); review references before extracting samples into a dataset",
        records.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failures_deduplicate_trials_and_require_review() {
        let results = vec![
            crate::compare::fixture("a", false, 0),
            crate::compare::fixture("a", false, 1),
            crate::compare::fixture("b", true, 0),
        ];
        let records = drafts(&results, "run", None, None);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["evidence"].as_array().unwrap().len(), 2);
        assert_eq!(records[0]["status"], "needs_review");
        assert!(serde_json::from_value::<mira::Sample>(records[0].clone()).is_err());
        assert!(drafts(&results, "run", None, Some("other")).is_empty());
    }
}
