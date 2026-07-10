//! A multi-axis matrix: the same eval run across several models **and** an extra
//! `effort` axis. The runner takes the cross-product, so this expands to
//! `samples × models × effort` independently-addressable cases, each with a
//! stable key like `reasoning/puzzle@sim[effort=high]`.
//!
//! ```bash
//! mira list --study-bin matrix
//! mira run --study-bin matrix
//! mira run --study-bin matrix 'effort=high'   # substring filter
//! mira run --study-bin matrix -j 4 --provider-concurrency anthropic=2     # bounded, per-provider
//! ```
//!
//! The host runs cases concurrently (bounded by `-j`, per-provider caps, and
//! adaptive backoff when a provider rate-limits), so a wide matrix finishes fast
//! without hammering any one provider.
//!
//! The subject reads the chosen axis value with `cx.param("effort")` and varies
//! its behaviour — here, "high" effort spends more tokens to get the answer
//! right. Offline against `sim` (plus key-gated cloud cases that skip).

use mira::scorer::{contains, succeeded, tokens_within};
use mira::subject::subject_fn;
use mira::{Eval, Target, eval};

mod support;
use support::fake_agent;

#[eval]
fn reasoning() -> Eval {
    Eval::new("reasoning")
        .describe("Same task across models × reasoning effort")
        .sample("puzzle", "What is 17 * 23? Think it through.")
        .axis("effort", ["low", "high"])
        .targets([
            Target::sim(),
            Target::anthropic("claude-opus-4-8"),
            Target::openai("gpt-5"),
        ])
        .subject(subject_fn(|_sample, cx| async move {
            // "low" effort answers fast but sometimes wrong; "high" reasons more.
            let high = cx.param("effort") == Some("high");
            if high {
                let mut t = fake_agent("Working through it: 17 * 23 = 391.", &["calc"]);
                t.usage.reasoning_tokens = 120;
                t
            } else {
                fake_agent("17 * 23 = 391.", &[])
            }
        }))
        .scorer(succeeded())
        .scorer(contains("391"))
        // A token budget both effort levels meet here — the matrix surfaces the
        // per-case token/cost numbers so you can compare the trade-off in the
        // report even when every case passes.
        .scorer(tokens_within(256))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
