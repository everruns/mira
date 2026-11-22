//! Integration tests driving a real everruns `Session` against the offline
//! simulated model through Mira's [`RuntimeSubject`], and on through the full
//! eval engine ([`Runner`]). No network, no keys — exercises the everruns
//! adapter end-to-end and deterministically.

use everruns::{Agent, Engine, Model};
use mira::scorer::{contains, succeeded};
use mira::subject::Subject;
use mira::{Eval, RunCx, Runner, Sample, Target, Transcript};
use mira_everruns::RuntimeSubject;

/// Build a `RuntimeSubject` whose factory spins up a session on an agent with
/// a fixed simulated reply.
fn llmsim_subject(reply: &'static str) -> RuntimeSubject {
    RuntimeSubject::new(move |_target| async move {
        let agent = Agent::builder()
            .name("assistant")
            .instructions("Answer concisely.")
            .model(Model::simulated(reply))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Engine::new().create(agent))
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

    // The adapter folds the real session event stream into an ATIF trajectory
    // (the primary structured contract) alongside the raw events channel.
    let trajectory = t.trajectory.as_ref().expect("trajectory attached");
    assert_eq!(trajectory.agent.name, "everruns");
    // The model name comes from the session's own reason/output metadata
    // (the simulated provider's id), not the target fallback — proof the
    // canonical envelopes decode into typed events.
    assert_eq!(trajectory.agent.model_name.as_deref(), Some("llmsim-model"));
    assert!(
        trajectory.steps.iter().any(
            |s| s.source == mira::trajectory::StepSource::User && s.message.text() == "Say hi."
        ),
        "user step folded from input.message"
    );
    let last_agent = trajectory
        .steps
        .iter()
        .rev()
        .find(|s| s.source == mira::trajectory::StepSource::Agent)
        .expect("agent step folded from the reasoning iteration");
    assert!(last_agent.message.text().contains("42"));
    assert!(!t.events.is_empty(), "raw events channel kept as-is");
}

#[tokio::test]
async fn full_eval_runs_green_against_llmsim() {
    let eval = Eval::new("llmsim")
        .sample("greet", "Say hi and tell me the answer to life.")
        .targets([Target::sim()])
        .subject(llmsim_subject("Hi! The answer to life is 42."))
        .scorer(succeeded())
        .scorer(contains("42"))
        .build();

    let report = Runner::new().add(eval).run().await;
    assert_eq!(report.total(), 1);
    assert!(report.all_passed(), "skipped: {:?}", report.skipped);
}

#[tokio::test]
async fn real_tool_calls_reach_the_transcript_and_trajectory() {
    // A scripted model calls a real `FunctionTool`, then answers. Pins the
    // live event wire shape the adapter reads: `tool.completed` names for
    // tool-selection scorers, and structured calls + observations in ATIF.
    use everruns::{FunctionTool, LlmSimConfig, SimToolCall, SimTurn};
    use serde_json::json;

    let subject = RuntimeSubject::new(|_target| async {
        let price = FunctionTool::new(
            "price",
            "Look up a ticker price.",
            json!({"type": "object", "properties": {"ticker": {"type": "string"}}}),
            |_args: serde_json::Value| async move { Ok::<_, String>(json!({"usd": 42})) },
        );
        let model = Model::simulated_with_config(LlmSimConfig::scripted(vec![
            SimTurn::ToolCalls(vec![SimToolCall {
                name: "price".into(),
                arguments: json!({"ticker": "GOOGL"}),
                id: Some("call_1".into()),
            }]),
            SimTurn::Assistant("GOOGL trades at 42.".into()),
        ]));
        let agent = Agent::builder()
            .instructions("Use price for tickers.")
            .model(model)
            .tool(price)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Engine::new().create(agent))
    });

    let t = subject
        .run(
            &Sample::new("price", "What does GOOGL trade at?"),
            &RunCx::new(Target::sim()),
        )
        .await;
    assert!(t.succeeded(), "unexpected error: {:?}", t.error);
    assert!(t.final_response.contains("42"), "{:?}", t.final_response);
    assert_eq!(t.tool_calls, vec!["price"]);

    let trajectory = t.trajectory.expect("trajectory attached");
    let call = trajectory
        .steps
        .iter()
        .flat_map(|s| &s.tool_calls)
        .find(|c| c.function_name == "price")
        .expect("structured tool call folded");
    assert_eq!(call.arguments, json!({"ticker": "GOOGL"}));
    assert!(
        trajectory
            .steps
            .iter()
            .filter_map(|s| s.observation.as_ref())
            .flat_map(|o| &o.results)
            .any(|r| r.source_call_id.as_deref() == Some(call.tool_call_id.as_str())),
        "observation correlated to the call"
    );
}
