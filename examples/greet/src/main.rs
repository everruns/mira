//! A minimal eval study, registered with the `#[eval]` attribute and served
//! with `Study::registered().serve()`. Run it under the host CLI:
//!
//! ```bash
//! mira --bin greet list
//! mira --bin greet run
//! ```
//!
//! The subject here is a deterministic in-process closure, so the whole thing
//! runs offline against the `sim` model with no API key.

use mira::scorer::{contains, model_graded, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Score, Target, Transcript, eval};

/// A tiny greeting eval across the offline sim plus a (key-gated) cloud case.
#[eval]
fn greet() -> Eval {
    Eval::new("greet")
        .describe("Greets the user and reports the answer to life")
        .meta("suite", "smoke")
        .add_sample(
            mira::Sample::new("hi", "Say hi and tell me the answer to life.")
                .tag("smoke")
                .meta("trace", "https://observe.example/greet/hi"),
        )
        .subject(subject_fn(|sample, _cx| async move {
            // A real subject would call a model; this one fakes a good answer.
            Transcript::response(format!(
                "Hi! In response to {:?}: the answer is 42.",
                sample.input.join(" ")
            ))
        }))
        .scorer(succeeded())
        .scorer(contains("42"))
        .scorer(model_graded(
            "Is the reply a friendly greeting?",
            Box::new(|rubric, t| {
                Box::pin(async move {
                    // A stand-in judge; a real one would call a cheaper model.
                    let ok = t.final_response.to_lowercase().contains("hi");
                    Score::graded("judge", if ok { 1.0 } else { 0.0 }, 0.5, rubric)
                })
            }),
        ))
        .targets([Target::sim(), Target::anthropic("claude-haiku-4-5")])
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
