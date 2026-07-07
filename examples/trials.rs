#!/usr/bin/env -S cargo +nightly -Zscript
---
# Single-file Mira study (cargo-script frontmatter, RFC 3502). Run it with
# the host CLI — no per-study crate:
#
#   mira --script examples/trials.rs run
#
# The host shims cargo-script on **stable** (it's otherwise nightly-only
# `cargo -Zscript`); set MIRA_SCRIPT_NATIVE=1 to run it natively on nightly.
# Outside this repo, depend on the published crates: mira-eval = "0.3".
[package]
edition = "2024"

[dependencies]
mira-eval = { path = "../crates/mira-eval" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
---
//! Trials / repetitions + seed: run the *same* case N times so the host can
//! report pass@k, pass-rate, and score variance over a stochastic subject.
//!
//! ```bash
//! mira --script examples/trials.rs list                 # shows `trials=8, seed=…`
//! mira --script examples/trials.rs run                   # 8 trials per case, aggregated
//! mira --script examples/trials.rs run --trials 20       # override the count
//! mira --script examples/trials.rs run --seed 1          # different seed base, reproducible
//! mira --script examples/trials.rs run --format json --out report.json   # `trials` array in the record
//! ```
//!
//! Unlike an `axis` (which forms *new* cases), trials are repetitions of one
//! logical case, grouped back by the host. Each trial gets a deterministic seed
//! (`base + trial`), so the whole repetition set replays identically — the heart
//! of reproducibility. The subject reads it via `cx.seed()` to seed its sampling.
//!
//! This agent is **intentionally flaky** (~70% of seeds succeed), so some trials
//! fail and `mira run` exits non-zero — that's the point: pass@k < 1. It's not in
//! the green `run-examples` smoke for that reason; run it by hand to see the
//! aggregation.

use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Transcript, eval};

/// A deterministic "stochastic" agent: a flaky tool whose success depends only on
/// the seed, so a fixed seed always reproduces the same outcome but different
/// seeds (different trials) vary — exactly what trials are meant to measure.
#[eval]
fn flaky() -> Eval {
    Eval::new("flaky")
        .describe("A seed-driven flaky agent — measured over repeated trials")
        // Repeat each case 8 times, seeded so the runs are reproducible.
        .trials(8)
        .seed(42)
        .sample("answer", "What is the capital of France?")
        .subject(subject_fn(|_sample, cx| async move {
            // Seed the (pretend) sampling. Real subjects would feed this into a
            // provider's `seed`/temperature; here it drives a tiny PRNG so the
            // outcome is reproducible per (case, seed).
            let seed = cx.seed().unwrap_or(0);
            // ~70% of seeds succeed — enough spread to make pass@k interesting.
            let succeeds = splitmix64(seed) % 100 < 70;
            if succeeds {
                Transcript::response("The capital of France is Paris.")
            } else {
                Transcript::response("I think it might be Lyon?")
            }
        }))
        .scorer(succeeded())
        .scorer(contains("Paris"))
        .build()
}

/// A tiny, dependency-free PRNG (SplitMix64) so a seed maps to a stable pseudo-
/// random outcome. Not for cryptography — just reproducible flakiness in a demo.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
