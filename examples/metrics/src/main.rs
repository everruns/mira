//! Tracking the *operational* metrics of an agent run, not just correctness:
//! tokens (total / output / cache / reasoning), cost, wall-clock latency,
//! time-to-first-token, tool-call count, and the exact set and ordering of
//! tools used. Every one of these is a first-class field on the `Transcript`
//! and has a budget scorer. It also reports a *custom* metric
//! (`retrieval_recall@5`) through the open `Transcript::metrics` map and grades
//! it with the generic `metric_at_least` scorer — the seam for any metric the
//! core doesn't model as a typed field.
//!
//! ```bash
//! mira run --bin metrics
//! mira run --bin metrics --format html --out report.html
//! ```
//!
//! Runs offline against `sim` with a deterministic fake agent, so the numbers
//! (and therefore the pass/fail verdicts) are stable.

use mira::scorer::{
    all_of, contains, cost_within, latency_within, metric_at_least, output_tokens_within,
    succeeded, tokens_within, tool_called_before, tools_used_exactly, ttft_within, turns_within,
};
use mira::subject::subject_fn;
use mira::{Eval, eval};

mod support;
use support::fake_agent;

#[eval]
fn metrics() -> Eval {
    Eval::new("metrics")
        .describe("Budgets on tokens, cost, latency, TTFT, and tool usage")
        .meta(
            "dashboard",
            "https://observe.example/dashboards/agent-metrics",
        )
        .sample(
            "lookup",
            "Find the customer's latest order and summarize it.",
        )
        .subject(subject_fn(|_sample, _cx| async move {
            // A real subject reports these from the provider/runtime; here a
            // deterministic stand-in derives them from the response text.
            fake_agent(
                "Found order #4821: 3 items, shipped yesterday.",
                &["search", "read", "summarize"],
            )
        }))
        // Correctness.
        .scorer(succeeded())
        .scorer(contains("#4821"))
        // Token & cost budgets.
        .scorer(tokens_within(500))
        .scorer(output_tokens_within(64))
        .scorer(cost_within(0.01))
        // Latency budgets (wall-clock and time-to-first-token).
        .scorer(latency_within(2_000))
        .scorer(ttft_within(500))
        // Custom, open-vocabulary metric: a domain signal the core doesn't model
        // as a typed field, reported by the subject and graded generically.
        .scorer(metric_at_least("retrieval_recall@5", 0.8))
        // Tool usage: how many, exactly which, and in what order.
        .scorer(turns_within(5))
        .scorer(tools_used_exactly(["search", "read", "summarize"]))
        .scorer(tool_called_before("search", "summarize"))
        // Combinators compose budgets into a single "within SLA" verdict.
        .scorer(all_of(
            "within_sla",
            vec![latency_within(2_000), cost_within(0.01), tokens_within(500)],
        ))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
