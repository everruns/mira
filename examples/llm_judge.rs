#!/usr/bin/env -S cargo +nightly -Zscript
---
# Single-file Mira study (cargo-script frontmatter, RFC 3502). Run it with
# the host CLI — no per-study crate:
#
#   mira run --study examples/llm_judge.rs
#
# The host shims cargo-script on **stable** (it's otherwise nightly-only
# `cargo -Zscript`); set MIRA_SCRIPT_NATIVE=1 to run it natively on nightly.
# Outside this repo, depend on the published crates: mira-eval = "0.3".
[package]
edition = "2024"

[dependencies]
mira-eval = { path = "../crates/mira-eval" }
mira-judge = { path = "../crates/mira-judge" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
---
//! Provider-backed **LLM-as-judge** scoring via `mira-judge`.
//!
//! ```bash
//! mira run --study examples/llm_judge.rs
//! # With a key, the judge case grades for real; without one it is N/A:
//! OPENAI_API_KEY=sk-... mira run --study examples/llm_judge.rs
//! ```
//!
//! The subject is a deterministic in-process stand-in, so the run is stable.
//! Deterministic scorers (`succeeded`, `contains`) always apply. The
//! `LlmJudge` scorer grades against the transcript when a key is set, and
//! returns **N/A** otherwise — so a key-free `mira run` stays green offline
//! (the CI examples job runs exactly this path).

use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Transcript, eval};
use mira_judge::{Include, LlmJudge};

#[eval]
fn llm_judge() -> Eval {
    Eval::new("llm_judge")
        .describe("Grade an answer with a real LLM judge (N/A without a key)")
        .sample("capital", "What is the capital of France?")
        .subject(subject_fn(|_sample, _cx| async move {
            // A real subject calls a model; this stand-in returns a good answer.
            Transcript::response("The capital of France is Paris.")
        }))
        // Deterministic gates always apply.
        .scorer(succeeded())
        .scorer(contains("Paris"))
        // LLM-as-judge over the transcript (response + tool calls). Swap in
        // `openai_responses` / `claude` to grade with a different provider.
        .scorer(
            LlmJudge::openai_completions("gpt-4o-mini")
                .include(Include::Transcript)
                .scorer("Does the response correctly and concisely name the capital of France?"),
        )
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
