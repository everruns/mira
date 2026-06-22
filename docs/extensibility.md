# Extensibility

Mira is deliberately small at the core and open at the edges. Almost everything
interesting is a **trait you implement** or a **free-form field you fill** —
there are no closed enums to fork. This page is the map of those seams; each
links to the page with the full detail.

The mental model: a **study** owns the open parts (subjects, scorers, what goes
in a transcript); the **host** is a fixed orchestrator (selection, the matrix,
aggregation, reporting). So you extend *behaviour* on the study side, and you
extend *data* by carrying it through the transcript and protocol.

## The seams at a glance

| Want to… | Seam | Where |
|----------|------|-------|
| Evaluate a new kind of system/agent | `Subject` trait (or `subject_fn` / `CliSubject`) | [subjects.md](subjects.md) |
| Grade a transcript a new way | `Scorer` trait (or the `scorer(name, closure)` hatch) | [scorers.md](scorers.md) |
| Judge with an LLM | `model_graded(rubric, judge)` — just a scorer | [scorers.md](scorers.md#llm-as-judge) |
| Attach provenance / links / labels | `metadata` (open-ended JSON) | [authoring.md](authoring.md#metadata--observability) |
| Carry a custom **metric** | `Transcript.metrics` (numeric) + `metric_within`/`metric_at_least` | [metrics.md](metrics.md#adding-a-custom-metric) |
| Stream structured run detail | `Transcript.events` (raw JSON) | [below](#events-the-structured-channel) |
| Vary a cell on a non-model dimension | extra matrix **axes** (`.axis(name, values)`) | [authoring.md](authoring.md#extra-matrix-axes) |
| Plug in a non-Rust study | implement the wire protocol in any language | [protocol.md](protocol.md#implementing-a-study-in-another-language) |
| Advertise an optional behaviour to hosts | `capabilities` tokens | [protocol.md](protocol.md#initialize) |

## Behaviour: subjects & scorers

The two core traits are open vocabularies, not fixed sets:

- **`Subject`** — turns a `Sample` into a `Transcript`. In-process closure,
  external binary (any language), or a stateful adapter you write. See
  [subjects.md](subjects.md).
- **`Scorer`** — turns a `Transcript` into a `Score` (a continuous `value` plus a
  boolean `pass`). A closure for one-offs, an `impl` for reusable/stateful
  scorers, `model_graded` for LLM-as-judge, and combinators (`all_of`, `any_of`,
  `not`) to compose. See [scorers.md](scorers.md).

Every scorer on an eval runs against every cell, so cross-cutting checks are just
scorers you add to each eval (a small helper that appends a shared set is the
idiom — there is no host-injected scoring).

## Data: carrying your own information

A `Transcript` is the currency between subject and scorers, and several of its
fields exist precisely so you can carry information the core doesn't model:

```rust
pub struct Transcript {
    pub final_response: String,
    pub iterations: usize,
    pub tool_calls_count: usize,
    pub usage: Usage,                     // typed metrics: tokens + cost
    pub timing: Timing,                   // typed metrics: duration, TTFT
    pub metrics: BTreeMap<String, f64>,   // open metrics: any numeric you measure
    pub tool_calls: Vec<String>,          // tool names, in call order
    pub files: BTreeMap<String, String>,  // workspace after the run
    pub events: Vec<serde_json::Value>,   // free-form structured stream
    pub metadata: Metadata,               // free-form, open-ended JSON
    pub error: Option<String>,
    pub error_kind: ErrorKind,            // Subject (default) | Infra (→ N/A, retried)
}
```

`Metadata` (a `BTreeMap<String, serde_json::Value>`) is also available on **evals**,
**samples**, and **targets** — and it flows end-to-end into the JSON record and
the HTML report (values that look like URLs render as links).

### Custom metrics

Mira models two metric families as typed fields — `Usage` (input/output/cache/
reasoning tokens, `cost_usd`) and `Timing` (`duration_ms`,
`time_to_first_token_ms`) — graded by the budget scorers (`tokens_within`,
`cost_within`, `latency_within`, …). For **your own metric**, record it on the
open `Transcript.metrics` map and grade it generically — no new type, and no new
protocol version for a custom metric key (the map itself is an additive, versioned
part of the wire):

```rust
// In the subject: record whatever you measured (here, retrieval recall@k).
let recall = hits_at_k as f64 / relevant_total as f64;
let t = Transcript::response(response_text)
    .with_metric("retrieval_recall@5", recall);

// Grade it like any budget (higher-is-better here):
use mira::scorer::metric_at_least;
let recall_scorer = metric_at_least("retrieval_recall@5", 0.80);
```

The metric then surfaces as a **pass/fail score** in every report and in the
per-case **`metrics` block** of the JSON/HTML. `metadata` (open-ended JSON) stays
the channel for non-numeric provenance; `events` for non-scalar structured
detail. See [metrics.md](metrics.md) for the full model.

### Events: the structured channel

`Transcript.events` is a `Vec<serde_json::Value>` — an open stream for anything
structured you want to keep: per-step traces, retrieval hits, intermediate tool
I/O. Subjects that already speak the canonical JSONL `Event` format (e.g.
everruns coding CLIs via `CliSubject`) populate it for free; you can also push
your own objects. Scorers receive the whole transcript, so a structural scorer
can walk `t.events` and grade on what it finds.

## Protocol-level extension

Because the host ↔ study boundary is **newline-delimited JSON with
forward-compatible payloads**, you can extend across the process line too:

- **New languages.** Anything that implements `initialize` / `list` / `run` is a
  valid study — no Mira dependency. See
  [Implementing a study in another language](protocol.md#implementing-a-study-in-another-language).
- **New fields.** Payloads ignore unknown fields and default missing ones, so a
  study can add fields an older host won't break on.
- **Optional behaviours.** Advertise `capabilities` tokens (`axes`, `events`,
  `usage`, `execute`, `score`, `paginate`) at `initialize` so hosts
  feature-detect additively instead of sniffing versions.

## What is *not* (yet) pluggable

Honest boundaries, so you don't fight the grain:

- **Host-side scoring.** The host never sees a raw transcript to grade — scoring
  lives in the study. Shared scorers are an authoring-time helper, not a host
  feature.
- **Host-defined matrix.** The study defines targets/axes; the host can *subset*
  (`--targets`, `--axis`, `--preset`, `--tag`, filter) but not add cells.
- **Run-to-run comparison.** Each run emits a stable JSON record (cells keyed by
  `eval/sample@target[k=v,…]`), but diffing two runs is left to a consumer on top.

If you need one of these, it's a feature add rather than a configuration knob —
open an issue describing the use case.
