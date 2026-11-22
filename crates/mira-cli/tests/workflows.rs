use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output},
};

fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_mira"))
        .current_dir(dir)
        .args(args)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(
        out.status.code().is_some(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn success(out: Output) -> Output {
    assert!(
        out.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn setup(dir: &Path) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/study.py");
    std::fs::write(
        dir.join("mira.toml"),
        format!(
            "default_launcher = \"fixture\"\n[launchers.fixture]\npython3 = {:?}\n",
            fixture.to_string_lossy()
        ),
    )
    .unwrap();
}
fn newest(dir: &Path) -> String {
    let mut ids: Vec<_> = std::fs::read_dir(dir.join("results"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("meta.json").exists())
        .collect();
    ids.sort_by_key(|p| {
        std::fs::metadata(p.join("meta.json"))
            .unwrap()
            .modified()
            .unwrap()
    });
    ids.last()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}
fn json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn saved_execution_rescoring_calibration_and_review_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup(dir);
    assert_eq!(
        run(dir, &["run"], &[("MIRA_TEST_ANSWER", "wrong")])
            .status
            .code(),
        Some(1)
    );
    let original = newest(dir);
    let saved = dir.join("results").join(&original);
    assert!(saved.join("provenance.json").exists());
    assert_eq!(
        std::fs::read_dir(saved.join("artifacts")).unwrap().count(),
        1
    );
    success(run(
        dir,
        &["failures", &original, "--out", "drafts.jsonl"],
        &[],
    ));
    let draft: Value = serde_json::from_str(
        std::fs::read_to_string(dir.join("drafts.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(draft["status"], "needs_review");
    assert_eq!(draft["sample"]["expected"], "42");
    let key = draft["evidence"][0]["case_key"].as_str().unwrap();
    std::fs::write(
        dir.join("labels.jsonl"),
        format!("{{\"case_key\":{key:?},\"pass\":false}}\n"),
    )
    .unwrap();
    success(run(
        dir,
        &[
            "calibrate",
            &original,
            "--labels",
            "labels.jsonl",
            "--scorer",
            "judge:v1",
            "--min-agreement",
            "1",
            "--max-false-pass-rate",
            "0",
            "--out",
            "calibration.json",
        ],
        &[],
    ));
    assert_eq!(json(&dir.join("calibration.json"))["agreement"], 1.0);
    // A changed reference cannot silently change the archived execution's grade.
    assert_eq!(
        run(
            dir,
            &["score", "--run-id", &original],
            &[("MIRA_TEST_EXPECTED", "wrong")]
        )
        .status
        .code(),
        Some(1)
    );
    success(run(
        dir,
        &["score", "--run-id", &original, "--use-current-samples"],
        &[("MIRA_TEST_EXPECTED", "wrong")],
    ));
    // Re-grade exactly the same execution with another judge version.
    success(run(
        dir,
        &["score", "--run-id", &original],
        &[("MIRA_TEST_JUDGE", "judge:v2"), ("MIRA_TEST_REVERSE", "1")],
    ));
    let rescored = newest(dir);
    success(run(
        dir,
        &[
            "calibrate",
            &rescored,
            "--labels",
            "labels.jsonl",
            "--scorer",
            "judge:v2",
            "--previous",
            &original,
            "--previous-scorer",
            "judge:v1",
            "--out",
            "flips.json",
        ],
        &[],
    ));
    assert_eq!(
        json(&dir.join("flips.json"))["verdict_flips"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        !run(
            dir,
            &["run", "--resume", &original],
            &[("MIRA_TEST_EXPECTED", "changed")]
        )
        .status
        .success()
    );
}

#[test]
fn missing_judge_is_incomplete_under_strict_gates() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup(dir);
    let out = run(
        dir,
        &["run", "--require-scorer", "judge:*", "--min-coverage", "1"],
        &[("MIRA_TEST_JUDGE_NA", "1")],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = json(
        &dir.join("results")
            .join(newest(dir))
            .join("evaluation.json"),
    );
    assert_eq!(report["quality_passed"], true);
    assert_eq!(report["complete"], false);
}

#[test]
fn failing_baseline_still_runs_candidate_and_compares() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup(dir);
    std::fs::write(dir.join("input.txt"), "pinned prompt").unwrap();
    std::fs::write(
        dir.join("experiment.toml"),
        r#"
study = "workflow"
launcher = "fixture"
[[treatments]]
name = "baseline"
env.MIRA_TEST_ANSWER = "wrong"
files = ["./input.txt"]
[[treatments]]
name = "candidate"
env.MIRA_TEST_ANSWER = "42"
files = ["./input.txt"]
[compare]
baseline = "baseline"
"#,
    )
    .unwrap();
    success(run(dir, &["run", "--experiment", "experiment.toml"], &[]));
    let experiment = std::fs::read_dir(dir.join("results/experiments"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let report = json(&experiment.join("comparison.json"));
    assert_eq!(report["comparisons"]["candidate"]["improvements"], 1);
    assert_eq!(report["comparisons"]["candidate"]["regressions"], 0);
    assert!(report["runs"]["candidate"].as_str().is_some());
    let baseline = report["runs"]["baseline"].as_str().unwrap();
    assert!(
        json(&Path::new(baseline).join("treatment-files.json"))
            .as_object()
            .unwrap()
            .values()
            .all(|v| v.as_str().unwrap().starts_with("sha256:"))
    );
    let candidate = report["runs"]["candidate"].as_str().unwrap();
    success(run(
        dir,
        &[
            "compare",
            baseline,
            candidate,
            "--max-regressions",
            "0",
            "--out",
            "comparison.json",
        ],
        &[],
    ));
}

#[test]
fn unavailable_targets_stay_skipped_with_full_artifact_capture() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup(dir);
    success(run(dir, &["run"], &[("MIRA_TEST_UNAVAILABLE", "1")]));
    let meta = json(&dir.join("results").join(newest(dir)).join("meta.json"));
    assert_eq!(meta["summary"]["skipped"], 1);
    assert_eq!(meta["summary"]["scored"], 0);
}

#[test]
fn malformed_gate_config_cannot_fall_back_to_an_ungated_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(
        dir.join("mira.toml"),
        "[gates]\nmin_coverage = \"invalid\"\n",
    )
    .unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/study.py");
    let out = run(
        dir,
        &["run", "--study-python", fixture.to_str().unwrap()],
        &[],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid"));
    assert!(!dir.join("results").exists());
}

#[test]
fn coverage_evidence_survives_every_report_format_and_rerender() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup(dir);
    let out = run(
        dir,
        &[
            "run",
            "--require-scorer",
            "judge:*",
            "--format",
            "json",
            "--out",
            "explicit.json",
        ],
        &[("MIRA_TEST_JUDGE_NA", "1")],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        json(&dir.join("explicit.json"))["evaluation"]["gates_passed"],
        false
    );
    let id = newest(dir);
    assert_eq!(
        json(&dir.join("results").join(&id).join("meta.json"))["evaluation"]["complete"],
        false
    );
    for (format, file) in [
        ("html", "report.html"),
        ("md", "report.md"),
        ("junit", "report.xml"),
        ("jsonl", "report.jsonl"),
        ("csv", "report.csv"),
    ] {
        success(run(
            dir,
            &["report", &id, "--format", format, "--out", file],
            &[],
        ));
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        match format {
            "html" | "md" => {
                assert!(text.contains("Evaluation incomplete"), "{format}: {text}");
                assert!(text.contains("Coverage gates failed"));
            }
            "junit" => {
                assert!(text.contains("name=\"mira.coverage\""));
                assert!(text.contains("<failure"));
            }
            "jsonl" => {
                let row: Value = serde_json::from_str(text.trim()).unwrap();
                assert_eq!(
                    row["transcript"]["metadata"]["mira.evaluation"]["gates_passed"],
                    false
                );
            }
            "csv" => assert!(
                text.lines()
                    .next()
                    .unwrap()
                    .contains("meta.mira.evaluation")
            ),
            _ => unreachable!(),
        }
    }
    assert_eq!(
        json(&dir.join("results").join(&id).join("report.json"))["evaluation"]["gates_passed"],
        false
    );
}

#[test]
fn repeated_sdk_cases_keep_distinct_artifacts_and_trial_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup(dir);
    success(run(dir, &["run", "--trials", "3", "--seed", "10"], &[]));
    let id = newest(dir);
    let saved = dir.join("results").join(&id);
    assert_eq!(
        std::fs::read_dir(saved.join("artifacts")).unwrap().count(),
        3
    );
    let report = json(&saved.join("report.json"));
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["trial"].as_u64().unwrap_or(0), index as u64);
        assert_eq!(case["trials"], 3);
        assert_eq!(case["seed"], 10 + index);
    }
    success(run(dir, &["score", "--run-id", &id], &[]));
}
