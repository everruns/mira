#!/usr/bin/env -S cargo +nightly -Zscript
---
# Single-file Mira study (cargo-script frontmatter, RFC 3502). Run it with
# the host CLI — no per-study crate:
#
#   mira run --study examples/coding.rs
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
//! A coding-style eval with seeded files, a model matrix, and structural +
//! file-based scorers — the shape that replaces a bespoke bench harness.
//!
//! ```bash
//! mira list --study examples/coding.rs
//! mira run --study examples/coding.rs --tag smoke
//! ```
//!
//! The subject is an in-process closure that "edits" the seeded file; swap it
//! for `mira_everruns::RuntimeSubject` to drive a real agent session.

use mira::scorer::{cost_within, file_contains, succeeded, tool_called, turns_within};
use mira::subject::subject_fn;
use mira::{Dataset, Eval, Sample, Target, Transcript, Usage, eval};

fn dataset() -> Dataset {
    Dataset::new(vec![
        Sample::new(
            "add-fn",
            "Add a `greet` function to lib.rs that returns \"hello\".",
        )
        .file("lib.rs", "// add code here\n")
        .tag("smoke"),
        Sample::new("fix-bug", "Fix the off-by-one in sum().")
            .file(
                "lib.rs",
                "fn sum(xs: &[i32]) -> i32 { xs.iter().take(xs.len()-1).sum() }\n",
            )
            .tag("regression"),
    ])
}

#[eval]
fn coding() -> Eval {
    Eval::new("coding")
        .describe("Edits seeded files to satisfy a coding instruction")
        .dataset(dataset())
        .subject(subject_fn(|sample, _cx| async move {
            // Pretend the agent edited the file and used an edit tool.
            let mut t = Transcript::response("Done. Added the requested code.");
            t.iterations = 2;
            t.tool_calls = vec!["read_file".into(), "edit_file".into()];
            t.tool_calls_count = 2;
            t.usage = Usage {
                input_tokens: 320,
                output_tokens: 80,
                cost_usd: 0.002,
                ..Default::default()
            };
            let mut contents = sample.files.get("lib.rs").cloned().unwrap_or_default();
            contents.push_str("\nfn greet() -> &'static str { \"hello\" }\n");
            t.files.insert("lib.rs".into(), contents);
            t
        }))
        .scorer(succeeded())
        .scorer(tool_called("edit_file"))
        .scorer(turns_within(5))
        .scorer(cost_within(0.05))
        .scorer(file_contains("lib.rs", "fn greet"))
        .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
        .max_turns(8)
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
