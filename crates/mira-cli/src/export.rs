//! `mira export <run_id> --format atif`: emit standalone ATIF-v1.7 documents
//! from a saved run.
//!
//! Export is **read-only over the run store** — no study process, no
//! re-execution, like [`report`](crate::report) and `publish`. For each case it
//! writes one self-contained ATIF trajectory document that external SFT / RL /
//! visualization tooling can consume directly.
//!
//! Source of a document's steps, in order:
//! 1. the case's real [`Transcript::trajectory`] — available only from an
//!    `execute` artifact (`run --execute-only --artifacts DIR`), passed via
//!    `--artifacts`, since a saved run folder keeps only the lightweight
//!    [`TranscriptSummary`];
//! 2. otherwise a **synthesized** trajectory projected from the flat summary
//!    fields ([`Trajectory::from_transcript`] — the lossy inverse of
//!    `project_into`).
//!
//! Mira's own verdicts never live on the ATIF wire; they are stamped into the
//! emitted document's root `extra` **on export only**:
//! * `extra.reward = { pass, score, scorers: [{ name, value, pass, reason }] }`
//!   from the case's [`RunResult`];
//! * `extra.mira = { eval, sample, target, run_id }` for provenance.
//!
//! Output: one `<case_key>.atif.json` per case (the case key
//! [encoded][crate::config::encode_key] for a collision-free filename) under
//! `results/<run_id>/export/`, or an explicit `--out DIR`. `--out -` streams all
//! documents as NDJSON to stdout instead. **Skipped (unexecuted) cases are
//! omitted** — they have no transcript to represent.
//!
//! [`Transcript::trajectory`]: mira::Transcript::trajectory
//! [`TranscriptSummary`]: mira::protocol::TranscriptSummary

use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;

use mira::Transcript;
use mira::protocol::RunResult;
use mira::trajectory::Trajectory;
use serde_json::{Value, json};

use crate::config;

/// The only export format today. Kept a named value so a future
/// `--format <other>` fails loudly rather than silently emitting ATIF.
pub const FORMAT_ATIF: &str = "atif";

/// `mira export`: load a saved run and emit ATIF documents. Resolves the results
/// base from `mira.toml` (like `report`/`publish`), then delegates to
/// [`run_at`].
pub fn run(
    run_id: &str,
    filter: Option<&str>,
    format: &str,
    out: Option<&str>,
    artifacts: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let base = config::Config::load().results_dir();
    run_at(&base, run_id, filter, format, out, artifacts)
}

/// [`run`] against an explicit results `base` — the testable core.
fn run_at(
    base: &str,
    run_id: &str,
    filter: Option<&str>,
    format: &str,
    out: Option<&str>,
    artifacts: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    if format != FORMAT_ATIF {
        return Err(
            format!("unknown export format {format:?}: only {FORMAT_ATIF:?} is supported").into(),
        );
    }

    let dir = config::run_dir(base, run_id);
    let mut results = config::load_case_results(&dir);
    if let Some(f) = filter {
        results.retain(|r| r.key().contains(f));
    }
    if results.is_empty() {
        return Err(format!(
            "no case results for run {run_id} (looked in {}/cases)",
            dir.display()
        )
        .into());
    }

    // Full transcripts (with trajectories) from execution artifacts, keyed by
    // case key. Empty without `--artifacts` — a run folder keeps only summaries.
    let full: BTreeMap<String, Transcript> = match artifacts {
        Some(adir) => crate::load_artifacts(adir)
            .into_iter()
            .map(|a| (a.key(), a.transcript))
            .collect(),
        None => BTreeMap::new(),
    };

    let (docs, skipped) = build_docs(&results, &full, run_id);
    if docs.is_empty() {
        eprintln!("run {run_id}: {skipped} case(s), all skipped — nothing to export");
        return Ok(());
    }

    // `--out -`: stream every document as NDJSON (one per line) to stdout.
    if out == Some("-") {
        let ndjson = render_ndjson(&docs)?;
        let stdout = std::io::stdout();
        let mut w = stdout.lock();
        w.write_all(ndjson.as_bytes())?;
        return Ok(());
    }

    // Otherwise write one pretty file per case.
    let out_dir = match out {
        Some(d) => PathBuf::from(d),
        None => dir.join("export"),
    };
    std::fs::create_dir_all(&out_dir)?;
    for (key, doc) in &docs {
        let path = out_dir.join(format!("{}.atif.json", config::encode_key(key)));
        std::fs::write(&path, serde_json::to_string_pretty(doc)?)?;
    }
    let note = if skipped > 0 {
        format!(" ({skipped} skipped case(s) omitted)")
    } else {
        String::new()
    };
    eprintln!(
        "exported {} ATIF document(s) to {}{note}",
        docs.len(),
        out_dir.display()
    );
    Ok(())
}

/// Build one ATIF document per non-skipped case, returning them paired with the
/// case key and the count of skipped cases omitted.
fn build_docs(
    results: &[RunResult],
    full: &BTreeMap<String, Transcript>,
    run_id: &str,
) -> (Vec<(String, Trajectory)>, usize) {
    let mut docs = Vec::new();
    let mut skipped = 0usize;
    for r in results {
        if r.skipped {
            skipped += 1;
            continue;
        }
        docs.push((r.key(), atif_document(r, full.get(&r.key()), run_id)));
    }
    (docs, skipped)
}

/// One case → one standalone ATIF document. Uses the case's real trajectory when
/// `full` carries one, else synthesizes from the flat summary fields; then
/// stamps `extra.reward` and `extra.mira` (export-only, never on the wire).
fn atif_document(result: &RunResult, full: Option<&Transcript>, run_id: &str) -> Trajectory {
    let mut traj = match full.and_then(|t| t.trajectory.clone()) {
        // Real trajectory (from an execute artifact): the faithful source.
        Some(t) => t,
        // No trajectory: reconstruct from flat fields — the full artifact
        // transcript's when we have it, else the saved summary's.
        None => {
            let synth = full
                .cloned()
                .unwrap_or_else(|| transcript_from_summary(result));
            Trajectory::from_transcript(&synth)
        }
    };

    // Sensible per-case identifiers — fill only when absent (a real trajectory
    // may already carry its own): session shared across the run, trajectory id
    // unique per case.
    if traj.session_id.is_none() {
        traj.session_id = Some(run_id.to_string());
    }
    if traj.trajectory_id.is_none() {
        traj.trajectory_id = Some(result.key());
    }

    // Mira-side data into ATIF root `extra` — export only.
    traj.extra.insert("reward".into(), reward_value(result));
    traj.extra
        .insert("mira".into(), provenance_value(result, run_id));
    traj
}

/// Reconstruct a [`Transcript`] from a saved run's [`TranscriptSummary`] flat
/// fields — the only trajectory-less source a run folder carries.
fn transcript_from_summary(r: &RunResult) -> Transcript {
    let s = &r.transcript;
    Transcript {
        final_response: s.final_response.clone(),
        iterations: s.iterations,
        tool_calls_count: s.tool_calls_count,
        tool_calls: s.tool_calls.clone(),
        usage: s.usage,
        ..Default::default()
    }
}

/// `extra.reward` for a case: the Mira verdict — overall `pass`/`score` plus each
/// scorer's `{ name, value, pass, reason }` (and `na` when set). This is the ATIF
/// RL reward slot, written only here, never on the study↔host wire.
fn reward_value(r: &RunResult) -> Value {
    let scorers: Vec<Value> = r
        .scores
        .iter()
        .map(|s| {
            let mut o = json!({
                "name": s.scorer,
                "value": s.value,
                "pass": s.pass,
                "reason": s.reason,
            });
            if s.na {
                o["na"] = Value::Bool(true);
            }
            o
        })
        .collect();
    json!({
        "pass": r.passed,
        "score": r.aggregate,
        "scorers": scorers,
    })
}

/// `extra.mira` for a case: provenance tying the document back to its run.
fn provenance_value(r: &RunResult, run_id: &str) -> Value {
    let mut o = json!({
        "eval": r.eval,
        "sample": r.sample,
        "target": r.target,
        "run_id": run_id,
    });
    if !r.params.is_empty() {
        o["params"] = json!(r.params);
    }
    if r.trials > 1 {
        o["trial"] = json!(r.trial);
        o["trials"] = json!(r.trials);
    }
    o
}

/// Render documents as NDJSON: one compact ATIF object per line.
fn render_ndjson(docs: &[(String, Trajectory)]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for (_key, doc) in docs {
        out.push_str(&serde_json::to_string(doc)?);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mira::Score;
    use mira::protocol::TranscriptSummary;
    use mira::trajectory::{ATIF_VERSION, Agent, Step, StepSource, ToolCall};

    fn summary_result(sample: &str) -> RunResult {
        RunResult {
            eval: "greet".into(),
            sample: sample.into(),
            target: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            input: Vec::new(),
            expected: None,
            passed: true,
            aggregate: 1.0,
            scores: vec![
                Score::pass("exact", "matched"),
                Score::na("judge", "no api key"),
            ],
            transcript: TranscriptSummary {
                final_response: "hello there".into(),
                iterations: 2,
                tool_calls_count: 1,
                tool_calls: vec!["search".into()],
                ..Default::default()
            },
            skipped: false,
        }
    }

    #[test]
    fn document_is_valid_atif_with_reward_and_provenance() {
        let r = summary_result("hi");
        let doc = atif_document(&r, None, "run-1");

        // Valid ATIF: serializes and re-parses through the strict loader.
        let json = serde_json::to_string(&doc).unwrap();
        let back = Trajectory::from_json(&json).unwrap();
        assert_eq!(back, doc);
        assert_eq!(doc.schema_version, ATIF_VERSION);
        assert_eq!(doc.session_id.as_deref(), Some("run-1"));
        assert_eq!(doc.trajectory_id.as_deref(), Some(&*r.key()));

        // Reward stamped from the RunResult.
        let reward = &doc.extra["reward"];
        assert_eq!(reward["pass"], json!(true));
        assert_eq!(reward["score"], json!(1.0));
        let scorers = reward["scorers"].as_array().unwrap();
        assert_eq!(scorers.len(), 2);
        assert_eq!(scorers[0]["name"], "exact");
        assert_eq!(scorers[0]["pass"], json!(true));
        assert_eq!(scorers[1]["name"], "judge");
        assert_eq!(scorers[1]["na"], json!(true)); // N/A preserved

        // Provenance.
        let mira = &doc.extra["mira"];
        assert_eq!(mira["eval"], "greet");
        assert_eq!(mira["sample"], "hi");
        assert_eq!(mira["target"], "sim");
        assert_eq!(mira["run_id"], "run-1");
    }

    #[test]
    fn synthesizes_steps_from_flat_summary() {
        // The projection path: a trajectory-less case becomes a synthesized doc.
        let doc = atif_document(&summary_result("hi"), None, "run-1");
        assert_eq!(doc.agent.name, "mira-export");
        assert_eq!(doc.tool_call_names(), vec!["search"]);
        assert_eq!(doc.final_agent_text().as_deref(), Some("hello there"));
    }

    #[test]
    fn prefers_real_trajectory_from_full_transcript() {
        // When a full transcript carries a real trajectory, export uses it
        // verbatim (arguments and all) rather than synthesizing.
        let mut traj = Trajectory::new(Agent::new("everruns-runtime", "1.2.3"));
        let mut step = Step::new(1, StepSource::Agent, "done");
        step.tool_calls = vec![ToolCall::new("c1", "grep", json!({"q": "needle"}))];
        traj.steps.push(step);
        let full = Transcript::from_trajectory(traj);

        let doc = atif_document(&summary_result("hi"), Some(&full), "run-1");
        assert_eq!(doc.agent.name, "everruns-runtime"); // not the synth agent
        assert_eq!(doc.steps.len(), 1);
        assert_eq!(doc.steps[0].tool_calls[0].arguments, json!({"q": "needle"}));
        // Reward is still stamped over the real trajectory.
        assert_eq!(doc.extra["reward"]["pass"], json!(true));
    }

    #[test]
    fn end_to_end_writes_atif_files_and_skips_unexecuted() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        let run_id = "20260711T000000Z-test";
        let dir = config::run_dir(base, run_id);

        let ok = summary_result("hi");
        let mut skipped = summary_result("bye");
        skipped.skipped = true;
        config::write_case_result(&dir, &ok.key(), &ok).unwrap();
        config::write_case_result(&dir, &skipped.key(), &skipped).unwrap();

        let out = tmp.path().join("out");
        run_at(
            base,
            run_id,
            None,
            "atif",
            Some(out.to_str().unwrap()),
            None,
        )
        .unwrap();

        // One file (the skipped case is omitted); it parses as ATIF with reward.
        let files: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        assert_eq!(files.len(), 1, "skipped case omitted");
        let text = std::fs::read_to_string(&files[0]).unwrap();
        assert!(files[0].to_string_lossy().ends_with(".atif.json"));
        let doc = Trajectory::from_json(&text).unwrap();
        assert_eq!(doc.extra["reward"]["pass"], json!(true));
        assert_eq!(doc.extra["mira"]["run_id"], run_id);
    }

    #[test]
    fn ndjson_mode_emits_one_atif_object_per_line() {
        let full = BTreeMap::new();
        let results = vec![summary_result("hi"), summary_result("bye")];
        let (docs, skipped) = build_docs(&results, &full, "run-1");
        assert_eq!(skipped, 0);

        let ndjson = render_ndjson(&docs).unwrap();
        let lines: Vec<&str> = ndjson.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            // Each line is a complete, valid ATIF document with reward stamped.
            let doc = Trajectory::from_json(line).unwrap();
            assert!(doc.extra.contains_key("reward"));
            assert!(doc.extra.contains_key("mira"));
        }
    }

    #[test]
    fn unknown_format_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        let err = run_at(base, "run-1", None, "sft", None, None).unwrap_err();
        assert!(err.to_string().contains("sft"), "{err}");
    }
}
