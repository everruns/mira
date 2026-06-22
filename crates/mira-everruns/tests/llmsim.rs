//! Integration tests driving a real `everruns-runtime` `InProcessRuntime`
//! against the offline `LlmSim` driver through Mira's [`RuntimeSubject`], and on
//! through the full eval engine ([`run_cell`] / [`Runner`]). No network, no
//! keys — exercises the everruns adapter end-to-end and deterministically.

use everruns_core::driver_registry::DriverRegistry;
use everruns_core::llmsim_driver::LlmSimConfig;
use everruns_core::{CapabilityRegistry, PlatformDefinition};
use everruns_runtime::InProcessRuntimeBuilder;
use mira::scorer::{contains, succeeded};
use mira::subject::Subject;
use mira::{Eval, RunCx, Runner, Sample, Target, Transcript};
use mira_everruns::{RuntimeSubject, target_to_resolved};

/// Build a `RuntimeSubject` whose factory spins up an `InProcessRuntime` with a
/// fixed `LlmSim` reply.
fn llmsim_subject(reply: &'static str) -> RuntimeSubject {
    RuntimeSubject::new(move |target| async move {
        let platform = PlatformDefinition::new(CapabilityRegistry::new(), DriverRegistry::new());
        let runtime = InProcessRuntimeBuilder::new()
            .platform_definition(platform)
            .llm_sim(LlmSimConfig::fixed(reply))
            .default_model(target_to_resolved(&target))
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
            .ok_or_else(|| "no default session".to_string())?;
        Ok((runtime, session_id))
    })
}

#[tokio::test]
async fn runtime_subject_produces_a_transcript() {
    let subject = llmsim_subject("Hi! The answer is 42.");
    let cx = RunCx::new(Target::sim());
    let t: Transcript = subject.run(&Sample::new("greet", "Say hi."), &cx).await;

    assert!(t.succeeded(), "unexpected error: {:?}", t.error);
    assert!(
        t.final_response.contains("42"),
        "response was {:?}",
        t.final_response
    );
    // The adapter timed the run.
    assert!(t.timing.duration_ms < 60_000);
}

#[tokio::test]
async fn full_eval_runs_green_against_llmsim() {
    let eval = Eval::new("llmsim")
        .case("greet", "Say hi and tell me the answer to life.")
        .targets([Target::sim()])
        .subject(llmsim_subject("Hi! The answer to life is 42."))
        .scorer(succeeded())
        .scorer(contains("42"))
        .build();

    let report = Runner::new().add(eval).run().await;
    assert_eq!(report.total(), 1);
    assert!(report.all_passed(), "skipped: {:?}", report.skipped);
}
