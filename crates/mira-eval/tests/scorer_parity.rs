//! Rust is the source of truth for scorer behaviour (see
//! `crates/mira-eval/src/scorer.rs`). This test runs the canonical parity
//! vectors in `schema/v1/conformance/scorers.json` through the real Rust
//! scorers and asserts the recorded expectations match — so a hand-authored
//! vector can never disagree with Rust. Each SDK runs the *same* vectors
//! against its own hand-written mirror, which is how cross-language parity is
//! maintained. The coverage test enforces the other direction: every known
//! deterministic scorer must have at least one vector, so adding a scorer in
//! Rust forces a vector (and therefore an SDK mirror).

use mira::scorer::*;
use mira::{Sample, Scorer, Transcript};
use serde_json::Value;

const VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schema/v1/conformance/scorers.json"
));

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap().to_string()
}
fn u(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap()
}
fn f(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap()
}

/// Build a scorer from a canonical descriptor (recursive for combinators).
fn build(spec: &Value) -> Box<dyn Scorer> {
    match spec["kind"].as_str().unwrap() {
        "contains" => contains(s(spec, "needle")),
        "not_contains" => not_contains(s(spec, "needle")),
        "equals" => equals(s(spec, "expected")),
        "regex" => regex(s(spec, "pattern")),
        "matches_expected" => matches_expected(),
        "non_empty" => non_empty(),
        "succeeded" => succeeded(),
        "file_exists" => file_exists(s(spec, "path")),
        "file_contains" => file_contains(s(spec, "path"), s(spec, "needle")),
        "tool_called" => tool_called(s(spec, "tool")),
        "tool_not_called" => tool_not_called(s(spec, "tool")),
        "tool_calls_within" => tool_calls_within(u(spec, "max") as usize),
        "turns_within" => turns_within(u(spec, "max") as usize),
        "tools_used_exactly" => tools_used_exactly(
            spec["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().to_string()),
        ),
        "tool_called_before" => tool_called_before(s(spec, "first"), s(spec, "second")),
        "cost_within" => cost_within(f(spec, "max_usd")),
        "tokens_within" => tokens_within(u(spec, "max")),
        "output_tokens_within" => output_tokens_within(u(spec, "max")),
        "latency_within" => latency_within(u(spec, "max_ms")),
        "ttft_within" => ttft_within(u(spec, "max_ms")),
        "metric_within" => metric_within(s(spec, "name"), f(spec, "max")),
        "metric_at_least" => metric_at_least(s(spec, "name"), f(spec, "min")),
        "json_valid" => json_valid(),
        "json_field_equals" => json_field_equals(s(spec, "key"), s(spec, "value")),
        "produced_modality" => produced_modality(s(spec, "modality")),
        "all_of" => all_of(
            s(spec, "name"),
            spec["of"].as_array().unwrap().iter().map(build).collect(),
        ),
        "any_of" => any_of(
            s(spec, "name"),
            spec["of"].as_array().unwrap().iter().map(build).collect(),
        ),
        "not" => not(build(&spec["of"])),
        other => panic!("unhandled scorer kind in vectors: {other}"),
    }
}

/// Every deterministic scorer that must be mirrored across SDKs. Adding a
/// scorer here without a vector fails `coverage`; the LLM-judge and the
/// `scorer(name, fn)` escape hatch are intentionally excluded (not portable).
const KINDS: &[&str] = &[
    "contains",
    "not_contains",
    "equals",
    "regex",
    "matches_expected",
    "non_empty",
    "succeeded",
    "file_exists",
    "file_contains",
    "tool_called",
    "tool_not_called",
    "tool_calls_within",
    "turns_within",
    "tools_used_exactly",
    "tool_called_before",
    "cost_within",
    "tokens_within",
    "output_tokens_within",
    "latency_within",
    "ttft_within",
    "metric_within",
    "metric_at_least",
    "json_valid",
    "json_field_equals",
    "produced_modality",
    "all_of",
    "any_of",
    "not",
];

fn collect_kinds(spec: &Value, acc: &mut std::collections::BTreeSet<String>) {
    acc.insert(spec["kind"].as_str().unwrap().to_string());
    match &spec["of"] {
        Value::Array(items) => items.iter().for_each(|s| collect_kinds(s, acc)),
        Value::Object(_) => collect_kinds(&spec["of"], acc),
        _ => {}
    }
}

#[tokio::test]
async fn vectors_match_rust() {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    let transcripts = &doc["transcripts"];

    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let scorer = build(&case["scorer"]);

        let tname = case["transcript"].as_str().unwrap();
        let transcript: Transcript = serde_json::from_value(transcripts[tname].clone()).unwrap();

        let mut sample = Sample::new("s", "q");
        if let Some(exp) = case["sample"].get("expected").and_then(Value::as_str) {
            sample = sample.expected(exp);
        }

        let score = scorer.score(&sample, &transcript).await;
        let expect = &case["expect"];
        assert_eq!(
            score.pass,
            expect["pass"].as_bool().unwrap(),
            "{name}: pass ({})",
            score.reason
        );
        assert_eq!(
            score.na,
            expect["na"].as_bool().unwrap(),
            "{name}: na ({})",
            score.reason
        );
        assert!(
            (score.value - expect["value"].as_f64().unwrap()).abs() < 1e-9,
            "{name}: value {} != {}",
            score.value,
            expect["value"]
        );
    }
}

#[tokio::test]
async fn coverage_every_known_scorer_has_a_vector() {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    let mut present = std::collections::BTreeSet::new();
    for case in doc["cases"].as_array().unwrap() {
        collect_kinds(&case["scorer"], &mut present);
    }
    let missing: Vec<&str> = KINDS
        .iter()
        .copied()
        .filter(|k| !present.contains(*k))
        .collect();
    assert!(
        missing.is_empty(),
        "deterministic scorers with no parity vector: {missing:?}"
    );
}
