//! Provider-backed **LLM-as-judge** scoring via `mira-judge`.
//!
//! ```bash
//! mira --bin llm_judge run
//! # With a key, the judge cell grades for real; without one it is N/A:
//! OPENAI_API_KEY=sk-... mira --bin llm_judge run
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
        .case("capital", "What is the capital of France?")
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
