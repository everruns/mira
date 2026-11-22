//! Drive a **real** everruns `Session` as the subject, against the offline
//! simulated model — no API keys, deterministic, free in CI. This is the
//! in-process path for evaluating everruns-based agents; swap the simulated
//! model for `target_to_model(&target)?` and the same eval runs for real.
//!
//! ```bash
//! mira run --study-bin llmsim
//! ```
//!
//! The embedder owns agent wiring inside the [`RuntimeSubject`] factory: build
//! an `Agent` and return a fresh `Session` to drive. Mira normalizes each turn
//! and the session's events into a `Transcript`, so every scorer and report
//! works unchanged.

use everruns::{Agent, Engine, Model};
use mira::scorer::{contains, succeeded};
use mira::{Eval, Target, eval};
use mira_everruns::{RuntimeSubject, target_to_model};

#[eval]
fn llmsim() -> Eval {
    let subject = RuntimeSubject::new(|target| async move {
        // `Target::sim()` gets the offline simulator: a fixed assistant reply,
        // no network. Any provider target maps to its live model.
        let model = if target == Target::sim() {
            Model::simulated("Hi! The answer to life is 42.")
        } else {
            target_to_model(&target)?
        };
        let agent = Agent::builder()
            .name("assistant")
            .instructions("You are a helpful assistant. Answer concisely.")
            .model(model)
            .max_iterations(4)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Engine::new().create(agent))
    });

    Eval::new("llmsim")
        .describe("Drives an everruns Session against the simulated model")
        .sample("greet", "Say hi and tell me the answer to life.")
        .targets([Target::sim()])
        .subject(subject)
        .scorer(succeeded())
        .scorer(contains("42"))
        .build()
}

fn main() -> std::io::Result<()> {
    mira::Study::registered().serve_blocking()
}
