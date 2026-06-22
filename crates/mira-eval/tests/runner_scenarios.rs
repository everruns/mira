//! Integration tests for the in-process [`Runner`] across realistic scenarios:
//! multi-eval suites, the target matrix, extra axes (cross-product), selection
//! (filter/tag/targets), skip-not-fail, and metric-budget scoring.

use mira::scorer::{contains, latency_within, succeeded, tokens_within, tools_used_exactly};
use mira::subject::subject_fn;
use mira::{Eval, Runner, Sample, Target, Timing, Transcript, Usage};

/// A subject that echoes its prompt and reports usage/timing/tools, so budget
/// scorers have something concrete to grade.
fn echo_eval(name: &str) -> Eval {
    Eval::new(name)
        .sample(Sample::new("hi", "say hi").tag("smoke"))
        .sample(Sample::new("bye", "say bye").tag("regression"))
        .subject(subject_fn(|s, _| async move {
            let mut t = Transcript::response(s.input.join(" "));
            t.usage = Usage {
                input_tokens: 10,
                output_tokens: 5,
                cost_usd: 0.001,
                ..Default::default()
            };
            t.timing = Timing {
                duration_ms: 25,
                time_to_first_token_ms: Some(10),
            };
            t.tool_calls = vec!["echo".into()];
            t.tool_calls_count = 1;
            t
        }))
        .scorer(succeeded())
        .scorer(contains("say"))
        .scorer(tokens_within(100))
        .scorer(latency_within(1_000))
        .scorer(tools_used_exactly(["echo"]))
        .build()
}

#[tokio::test]
async fn multi_eval_suite_runs_every_cell() {
    let report = Runner::new()
        .add(echo_eval("alpha"))
        .add(echo_eval("beta"))
        .run()
        .await;
    // 2 evals × 2 samples × 1 (sim) target.
    assert_eq!(report.total(), 4);
    assert!(report.all_passed());
}

#[tokio::test]
async fn matrix_crosses_models_and_axes() {
    let eval = Eval::new("grid")
        .case("a", "x")
        .targets([Target::sim(), Target::sim().label("sim2")])
        .axis("effort", ["low", "high"])
        .subject(subject_fn(|_, cx| async move {
            Transcript::response(format!(
                "{}/{}",
                cx.target.label,
                cx.param("effort").unwrap_or("?")
            ))
        }))
        .scorer(succeeded())
        .build();

    // 1 sample × 2 targets × 2 effort values = 4 cells.
    let report = Runner::new().add(eval).run().await;
    assert_eq!(report.total(), 4);
    let mut keys: Vec<String> = report.outcomes.iter().map(|o| o.key()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "grid/a@sim2[effort=high]",
            "grid/a@sim2[effort=low]",
            "grid/a@sim[effort=high]",
            "grid/a@sim[effort=low]",
        ]
    );
}

#[tokio::test]
async fn selection_filter_tag_and_models() {
    // Filter on the case key.
    let r = Runner::new()
        .add(echo_eval("alpha"))
        .filter(Some("hi".into()))
        .run()
        .await;
    assert_eq!(r.total(), 1);
    assert_eq!(r.outcomes[0].sample_id, "hi");

    // Tag narrows to matching samples.
    let r = Runner::new()
        .add(echo_eval("alpha"))
        .tag(Some("regression".into()))
        .run()
        .await;
    assert_eq!(r.total(), 1);
    assert_eq!(r.outcomes[0].sample_id, "bye");

    // Model restriction selects a single matrix column.
    let eval = Eval::new("m")
        .case("a", "x")
        .targets([Target::sim(), Target::sim().label("sim2")])
        .subject(subject_fn(|_, _| async { Transcript::response("x") }))
        .scorer(succeeded())
        .build();
    let r = Runner::new()
        .add(eval)
        .targets(Some(vec!["sim2".into()]))
        .run()
        .await;
    assert_eq!(r.total(), 1);
    assert_eq!(r.outcomes[0].target, "sim2");
}

#[tokio::test]
async fn unavailable_cells_skip_and_stay_green() {
    let eval = Eval::new("cloud")
        .case("a", "x")
        // An unavailable cloud cell (no key) alongside the always-on sim.
        .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
        .subject(subject_fn(|_, _| async { Transcript::response("x") }))
        .scorer(contains("x"))
        .build();
    let report = Runner::new().add(eval).run().await;
    assert_eq!(report.total(), 1); // only sim ran
    assert_eq!(report.skipped.len(), 1); // cloud skipped
    assert!(report.all_passed()); // skip != fail
}

#[tokio::test]
async fn failing_budget_is_reported() {
    let eval = Eval::new("overspend")
        .case("a", "x")
        .subject(subject_fn(|_, _| async {
            let mut t = Transcript::response("x");
            t.usage = Usage {
                input_tokens: 1_000,
                output_tokens: 1_000,
                ..Default::default()
            };
            t
        }))
        .scorer(tokens_within(100))
        .build();
    let report = Runner::new().add(eval).run().await;
    assert_eq!(report.failed(), 1);
    assert!(!report.outcomes[0].passed);
}
