//! Drive a **real** `everruns-runtime` session as the subject, against the
//! offline `LlmSim` driver — no API keys, deterministic, free in CI. This is the
//! in-process path for evaluating everruns-based agents; swap the driver/model
//! for a cloud provider and the same eval runs for real.
//!
//! ```bash
//! mira --bin llmsim run
//! ```
//!
//! The embedder owns runtime wiring inside the [`RuntimeSubject`] factory: build
//! an `InProcessRuntime` with a harness/agent/session and return the session to
//! drive. Mira normalizes the runtime's result into a `Transcript`, so every
//! scorer and report works unchanged.

use everruns_core::driver_registry::DriverRegistry;
use everruns_core::llmsim_driver::LlmSimConfig;
use everruns_core::{CapabilityRegistry, PlatformDefinition};
use everruns_runtime::InProcessRuntimeBuilder;
use mira::scorer::{contains, succeeded};
use mira::{Eval, Target, eval};
use mira_everruns::{RuntimeSubject, target_to_resolved};

#[eval]
fn llmsim() -> Eval {
    let subject = RuntimeSubject::new(|model| async move {
        let platform = PlatformDefinition::new(CapabilityRegistry::new(), DriverRegistry::new());
        let runtime = InProcessRuntimeBuilder::new()
            .platform_definition(platform)
            // The offline simulator: a fixed assistant reply, no network.
            .llm_sim(LlmSimConfig::fixed("Hi! The answer to life is 42."))
            .default_model(target_to_resolved(&model))
            .single_session(|s| {
                s.harness("assistant", "You are a helpful assistant.")
                    .agent("assistant-agent", "Answer concisely.")
                    .agent_max_iterations(4)
                    .session_title("Eval Session")
            })
            .build()
            .await
            .map_err(|e| e.to_string())?;
        let session_id = runtime
            .default_session_id()
            .ok_or_else(|| "runtime built no default session".to_string())?;
        Ok((runtime, session_id))
    });

    Eval::new("llmsim")
        .describe("Drives an everruns InProcessRuntime against the LlmSim driver")
        .case("greet", "Say hi and tell me the answer to life.")
        // `Target::sim()` routes to the LlmSim driver via `target_to_resolved`.
        .targets([Target::sim()])
        .subject(subject)
        .scorer(succeeded())
        .scorer(contains("42"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
