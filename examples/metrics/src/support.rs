//! Local helper for this example: a deterministic fake "agent" transcript with
//! realistic-looking metrics, so metric-oriented scorers have something to grade
//! without calling a real model. Numbers are derived from the text so they are
//! stable across runs.

use mira::{Timing, Transcript, Usage};

pub fn fake_agent(response: &str, tools: &[&str]) -> Transcript {
    let output_tokens = (response.split_whitespace().count() as u64).max(1);
    let input_tokens = 40 + output_tokens * 3;
    let mut t = Transcript::response(response);
    t.iterations = tools.len().max(1);
    t.tool_calls = tools.iter().map(|s| s.to_string()).collect();
    t.tool_calls_count = t.tool_calls.len();
    t.usage = Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens: input_tokens / 4,
        reasoning_tokens: output_tokens / 5,
        cost_usd: (input_tokens as f64 * 3.0 + output_tokens as f64 * 15.0) / 1_000_000.0,
    };
    t.timing = Timing {
        duration_ms: 60 + output_tokens * 4,
        time_to_first_token_ms: Some(35 + input_tokens / 8),
    };
    // A custom, domain-specific metric the core doesn't model as a typed field:
    // retrieval recall@5 (higher is better). Graded with `metric_at_least`.
    t.record_metric("retrieval_recall@5", 0.83);
    t
}
