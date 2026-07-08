# Metrics

Mira records the *operational* signals of a run alongside its correctness:
tokens, cost, latency, time-to-first-token, tool usage — and any custom numeric
metric you want to track. This page is the full model and how to extend it.

The shape: a **subject** measures metrics and puts them on the `Transcript`; a
**scorer** turns a metric into a pass/fail (a budget). There is no separate
metrics pipeline — metrics ride the transcript, surface in every report, and are
graded by ordinary scorers.

## The model

Two metric families are **typed fields**, because the shared budget scorers
depend on their exact shape:

```rust
pub struct Usage {            // summed across all turns
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,   // subset of input, when billed separately
    pub reasoning_tokens: u64,    // subset of output (thinking tokens)
    pub cost_usd: f64,
}

pub struct Timing {
    pub duration_ms: u64,                  // wall-clock
    pub time_to_first_token_ms: Option<u64>,
}
```

Plus the structural counters on the transcript itself: `iterations`,
`tool_calls_count`, and `tool_calls` (the ordered names).

Everything else is an **open vocabulary**: `Transcript.metrics`, a
`BTreeMap<String, f64>` for any metric the core doesn't model as a typed field.

```rust
pub struct Transcript {
    // …
    pub usage: Usage,                  // typed: tokens + cost
    pub timing: Timing,                // typed: duration, TTFT
    pub metrics: BTreeMap<String, f64>, // open: anything numeric you measure
    // …
}
```

### Why two tiers?

Typed fields give the built-in budgets (`tokens_within`, `cost_within`,
`latency_within`, `ttft_within`, …) a stable shape to read, and they roll up into
the report totals (total tokens, total cost). The open `metrics` map is the
extension seam: a subject reports a new metric *key* and grades it generically,
with **no new protocol version or core change** (the `metrics` map itself is an
additive, versioned part of the wire — see [the protocol](protocol.md)). Use
`metrics` (not `metadata`) for anything
you want to compare numerically — values stay `f64` and feed the generic scorers;
`metadata` is for free-form strings (links, ids, labels).

## Built-in metric scorers

| Scorer | Reads | Passes when |
|--------|-------|-------------|
| `tokens_within(n)` | `usage` | total (input+output) tokens ≤ `n` |
| `output_tokens_within(n)` | `usage` | completion tokens ≤ `n` |
| `cost_within(usd)` | `usage` | total cost ≤ `usd` |
| `turns_within(n)` | `iterations` | at most `n` reasoning iterations |
| `latency_within(ms)` | `timing` | wall-clock duration ≤ `ms` |
| `ttft_within(ms)` | `timing` | time-to-first-token ≤ `ms` (fails if unmeasured) |
| `metric_within(name, max)` | `metrics` | custom `name` ≤ `max` (fails if unreported) |
| `metric_at_least(name, min)` | `metrics` | custom `name` ≥ `min` (fails if unreported) |

A budget over an *unreported* metric **fails** rather than silently passing — a
budget you can't verify is not satisfied.

## Adding a custom metric

Two steps: record it from the subject, grade it with a scorer.

```rust
use mira::scorer::{metric_at_least, metric_within};
use mira::subject::subject_fn;
use mira::{Eval, Transcript};

let eval = Eval::new("retrieval")
    .sample("q1", "Find the relevant passages.")
    .subject(subject_fn(|_sample, _cx| async move {
        // 1. Measure whatever you care about and record it (f64, keyed by name).
        let recall = hits_at_k as f64 / relevant_total as f64;
        Transcript::response(answer)
            .with_metric("retrieval_recall@5", recall)
            .with_metric("rerank_ms", 18.0)
    }))
    // 2. Grade it like any budget — higher-is-better or lower-is-better.
    .scorer(metric_at_least("retrieval_recall@5", 0.80))
    .scorer(metric_within("rerank_ms", 50.0))
    .build();
```

`with_metric(name, value)` is the builder form; `record_metric(&mut self, …)` is
the in-place form for subjects that build the transcript mutably, and
`transcript.metric(name) -> Option<f64>` reads one back. Non-finite values
(`NaN`/`±inf`) are dropped on record — JSON can't represent them, so the metric
stays unreported rather than breaking the report.

Once recorded, a custom metric surfaces three ways: as a **pass/fail score** in
every report, in the **per-case `metrics` block** of the JSON and HTML reports,
and (because it rides the transcript) in any saved run. A working end-to-end
example is [`examples/metrics`](../examples/metrics) — it reports
`retrieval_recall@5` and grades it with `metric_at_least`.

### Beyond a scalar

If you want a *derived* verdict or your metric isn't a single number, use a
[closure scorer](scorers.md#closures-the-escape-hatch): it receives the whole
transcript, so it can combine `usage`/`timing`/`metrics`, read the structured
ATIF `trajectory` (per-step tool calls, observations, and metrics — the primary
structured contract), and emit a `Score::graded(...)`. For non-numeric
structured detail (per-step traces, retrieval hits), prefer
`Transcript.trajectory` over `metrics`; the raw `events` channel is
advanced-only, for producer-specific data the trajectory doesn't model.

## Where metrics go

- **CLI** — a per-case line (`68 tok · $0.0003 · 88ms · 3 tool calls`).
- **JSON** (`--format json`) — the full `RunResult` per case, including `usage`,
  `timing`, and the `metrics` map, plus rolled-up totals in `summary`.
- **HTML** (`--format html`) — summary cards, the pass/fail matrix, and a
  per-case breakdown that lists scores, tools, the `metrics` map, and metadata.
- **JUnit** (`--format junit`) — pass/fail per case for any CI.

## See also

- [Scorers](scorers.md) — the budget scorers and how to write your own.
- [Extensibility](extensibility.md) — the full map of extension seams.
- [The protocol](protocol.md) — how `metrics` rides the wire, and its
  forward-compatible versioning.
