//! Shared helpers for the Mira example eval servers.
//!
//! The examples are deliberately **offline and deterministic** — they run
//! against the `sim` model with no API keys so they stay green in CI and cost
//! nothing. Each `examples/*.rs` is a standalone server; drive them with the
//! host CLI:
//!
//! ```bash
//! cargo run -p mira-cli -- --package mira-examples --example greet run
//! ```

use mira::{Timing, Transcript, Usage};

/// A deterministic fake "agent" transcript with realistic-looking metrics, so
/// metric-oriented scorers (tokens, cost, latency, tools) have something to
/// grade without calling a real model. Tokens/cost are derived from the text so
/// the numbers are stable across runs.
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
    t
}
