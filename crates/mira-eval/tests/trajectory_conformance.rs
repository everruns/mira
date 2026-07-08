//! Rust is the source of truth for the ATIF trajectory contract (see
//! `crates/mira-eval/src/trajectory.rs`). This test runs the canonical vectors
//! in `schema/v1/conformance/trajectory.json` through the real types and
//! `Trajectory::project_into`, asserting each document (1) parses — or is
//! rejected when `rejects` is set, (2) round-trips (re-serialize → re-parse →
//! equal, `extra` maps included; unknown fields tolerated on parse), and
//! (3) projects onto the pinned Transcript flat fields. Each SDK runs the
//! *same* vectors against its hand-written projection mirror
//! (`sdks/python/mira/trajectory.py`, `sdks/typescript/src/trajectory.ts`) —
//! the `scorers.json` three-runner pattern.

use mira::trajectory::Trajectory;
use mira::{Transcript, Usage};
use serde_json::Value;

const VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schema/v1/conformance/trajectory.json"
));

fn cases() -> Vec<Value> {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    doc["cases"].as_array().unwrap().clone()
}

fn expected_usage(projection: &Value) -> Usage {
    serde_json::from_value(projection["usage"].clone()).unwrap()
}

#[test]
fn vectors_parse_or_reject() {
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let parsed = Trajectory::from_value(case["trajectory"].clone());
        if case["rejects"].as_bool().unwrap_or(false) {
            let err = parsed.expect_err(&format!("{name}: must be rejected"));
            assert!(
                err.contains("schema_version"),
                "{name}: rejection names the schema_version, got: {err}"
            );
        } else {
            parsed.unwrap_or_else(|e| panic!("{name}: must parse, got: {e}"));
        }
    }
}

#[test]
fn vectors_round_trip() {
    for case in cases() {
        if case["rejects"].as_bool().unwrap_or(false) {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let first = Trajectory::from_value(case["trajectory"].clone()).unwrap();
        let again = Trajectory::from_json(&serde_json::to_string(&first).unwrap())
            .unwrap_or_else(|e| panic!("{name}: round-trip re-parse failed: {e}"));
        assert_eq!(again, first, "{name}: round-trip must be lossless");
    }
}

#[test]
fn vectors_project_into_pinned_flat_fields() {
    for case in cases() {
        if case["rejects"].as_bool().unwrap_or(false) {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let trajectory = Trajectory::from_value(case["trajectory"].clone()).unwrap();
        let expect = &case["projection"];

        // The zero-burden constructor is the pinned path: trajectory in, fully
        // projected transcript out.
        let t = Transcript::from_trajectory(trajectory);
        assert_eq!(
            t.final_response,
            expect["final_response"].as_str().unwrap(),
            "{name}: final_response"
        );
        let tool_calls: Vec<String> = serde_json::from_value(expect["tool_calls"].clone()).unwrap();
        assert_eq!(t.tool_calls, tool_calls, "{name}: tool_calls");
        assert_eq!(
            t.tool_calls_count,
            expect["tool_calls_count"].as_u64().unwrap() as usize,
            "{name}: tool_calls_count"
        );
        assert_eq!(
            t.iterations,
            expect["iterations"].as_u64().unwrap() as usize,
            "{name}: iterations"
        );
        let usage = expected_usage(expect);
        assert_eq!(t.usage.input_tokens, usage.input_tokens, "{name}: input");
        assert_eq!(t.usage.output_tokens, usage.output_tokens, "{name}: output");
        assert_eq!(
            t.usage.cache_read_tokens, usage.cache_read_tokens,
            "{name}: cache_read"
        );
        assert_eq!(
            t.usage.reasoning_tokens, usage.reasoning_tokens,
            "{name}: reasoning"
        );
        assert!(
            (t.usage.cost_usd - usage.cost_usd).abs() < 1e-9,
            "{name}: cost {} != {}",
            t.usage.cost_usd,
            usage.cost_usd
        );
    }
}
