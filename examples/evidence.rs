#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2024"
[dependencies]
mira-eval = { path = "../crates/mira-eval" }
serde_json = "1"
---
//! Offline recipes for grading tool evidence, conversation retention, and
//! retrieval separately from generation. Replace the fixture subjects with
//! your agent; keep the checks and data independent of its implementation.

use mira::{Eval, Sample, Transcript, eval};
use mira::scorer::*;
use mira::subject::subject_fn;
use serde_json::json;

#[eval]
fn tool_evidence() -> Eval {
    Eval::new("tool-evidence")
        .sample("order", "Find order 42 and report its status.")
        .subject(subject_fn(|_, _| async {
            let trajectory = serde_json::from_value(json!({
                "schema_version":"ATIF-v1.7", "agent":{"name":"fixture","version":"1"},
                "steps":[
                    {"step_id":1,"source":"agent","message":"", "tool_calls":[
                        {"tool_call_id":"lookup-1","function_name":"lookup_order","arguments":{"id":42}}],
                     "observation":{"results":[{"source_call_id":"lookup-1","content":"Order 42: shipped"}]}},
                    {"step_id":2,"source":"agent","message":"Order 42 has shipped."}
                ]
            })).unwrap();
            Transcript::from_trajectory(trajectory)
        }))
        .scorer(tool_called_with("lookup_order", "/id", json!(42)))
        .scorer(observation_contains("lookup_order", "shipped"))
        .scorer(contains("shipped"))
        .build()
}

#[eval]
fn conversation_retention() -> Eval {
    Eval::new("conversation-retention")
        .add_sample(Sample::turns("name", ["My name is Oleks.", "What is my name?"]).expected("Oleks"))
        .subject(subject_fn(|sample, _| async move {
            let name = sample.input[0].trim_start_matches("My name is ").trim_end_matches('.');
            Transcript::response(name)
        }))
        .scorer(matches_expected())
        .build()
}

#[eval]
fn retrieval() -> Eval {
    Eval::new("retrieval")
        .sample("docs", "Retrieve the two relevant documents.")
        .subject(subject_fn(|_, _| async {
            let relevant = ["doc-1", "doc-3"];
            let retrieved = ["doc-1", "noise", "doc-3"];
            let hits = retrieved.iter().filter(|id| relevant.contains(id)).count() as f64;
            Transcript::response(retrieved.join(", "))
                .with_metric("context_precision", hits / retrieved.len() as f64)
                .with_metric("context_recall", hits / relevant.len() as f64)
        }))
        .scorer(metric_at_least("context_precision", 0.6))
        .scorer(metric_at_least("context_recall", 1.0))
        .build()
}

#[eval]
fn grounding() -> Eval {
    Eval::new("grounding")
        .sample("claims", "Answer using only the retrieved evidence.")
        .subject(subject_fn(|_, _| async {
            // Exact claim membership is a deterministic fixture, not a general
            // semantic faithfulness metric. Real studies can use a calibrated judge.
            let context = ["Release 42 shipped", "Oleks owns the release"];
            let claims = ["Release 42 shipped", "Oleks owns the release"];
            let supported = claims.iter().filter(|claim| context.contains(claim)).count();
            Transcript::response(claims.join(". "))
                .with_metric("supported_claim_fraction", supported as f64 / claims.len() as f64)
        }))
        .scorer(metric_at_least("supported_claim_fraction", 1.0))
        .build()
}

fn main() -> std::io::Result<()> {
    mira::Study::registered().serve_blocking()
}
