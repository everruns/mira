# mira-eval

The core of [Mira](https://github.com/everruns/mira) — a Rust-first, code-first
evaluation framework for agents and tools, built for multi-turn, tool-using,
long-running agent trajectories. The library is imported as `mira`.

[![crates.io](https://img.shields.io/crates/v/mira-eval.svg)](https://crates.io/crates/mira-eval)
[![docs.rs](https://img.shields.io/docsrs/mira-eval)](https://docs.rs/mira-eval)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

You define evals in Rust; the [`mira` host CLI](https://crates.io/crates/mira-cli)
runs them across a model matrix, scores the results, and reports. This crate is
**provider-agnostic** — it carries `(provider, model)` labels and no SDK types,
so it has no heavy dependencies.

## The model

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix × axes
```

- **`Sample`** — one dataset row: input turns, an optional `target`, seeded
  `files`, `tags`, and `metadata`. Write it inline, or load `Dataset::{jsonl,json}`.
- **`Subject`** — the thing under evaluation, one adapter per *shape*:
  `subject_fn(closure)` (in-process), `CliSubject` (an external binary — the
  polyglot path), or `mira_everruns::RuntimeSubject` (a live runtime session).
- **`Transcript`** — the normalized result every subject produces: final
  response, iteration/tool counts, token + cost usage, tool names, captured
  files, raw events, and any error. One shape, so scoring and reporting are shared.
- **`Scorer`** — `score(&Sample, &Transcript) -> Score`. Deterministic built-ins
  (`contains`, `regex`, `tool_called`, `file_contains`, …), operational budgets
  (`tokens_within`, `cost_within`, `latency_within`, `ttft_within`,
  `tools_used_exactly`, …), combinators (`all_of`/`any_of`/`not`), an
  arbitrary-closure escape hatch, and LLM-as-judge (`model_graded`) — one open
  vocabulary, freely composed.
- **`Target`** — one matrix case: a provider-agnostic `(label, provider, model,
  available, metadata)` tuple with no API keys. Add arbitrary axes
  (`.axis("effort", ["low","high"])`) and the runner takes the cross-product.

## Example

```rust
use mira::scorer::{contains, succeeded, latency_within};
use mira::subject::subject_fn;
use mira::{Eval, Transcript, register_eval};

fn greet() -> Eval {
    Eval::new("greet")
        .sample("hi", "Say hi and tell me the answer to life.")
        .subject(subject_fn(|_s, _cx| async move {
            // A real subject calls a model; this one fakes a good answer.
            Transcript::response("Hi! The answer is 42.")
        }))
        .scorer(succeeded())
        .scorer(contains("42"))
        .scorer(latency_within(2_000))
        .build()
}
register_eval!(greet);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
```

The `#[eval]` attribute (re-exported from
[`mira-macros`](https://crates.io/crates/mira-macros) via the default `macros`
feature) gives `cargo test`-style discovery as an alternative to
`register_eval!`. Run the study with the
[`mira-cli`](https://crates.io/crates/mira-cli) host — as a crate `[[bin]]`, or
as a single-file [cargo-script](https://github.com/everruns/mira/blob/main/docs/how-it-works.md#single-file-studies)
study with no `Cargo.toml` at all:

```bash
mira run --study study.rs    # a single-file study (deps in frontmatter)
mira run --study-bin NAME    # …or a crate bin/example study
```

## Two processes, one protocol

Your eval program (the *study*) and the runner (the *host*) live on opposite
sides of a process boundary, talking newline-delimited JSON over stdio
(MCP-style). The study owns subjects and scoring — and **provider API keys,
which never cross the wire**. The host owns selection, the matrix, saved runs,
and rendering. The [protocol](https://github.com/everruns/mira/blob/main/docs/protocol.md)
is versioned and forward-compatible, which makes any program in any language a
valid study.

## Learn more

- [Getting started](https://github.com/everruns/mira/blob/main/docs/getting-started.md)
- [How it works](https://github.com/everruns/mira/blob/main/docs/how-it-works.md)
- [Authoring evals](https://github.com/everruns/mira/blob/main/docs/authoring.md)
  · [Scorers](https://github.com/everruns/mira/blob/main/docs/scorers.md)
  · [Subjects](https://github.com/everruns/mira/blob/main/docs/subjects.md)
- [The eval protocol](https://github.com/everruns/mira/blob/main/docs/protocol.md)

Licensed under MIT — see [LICENSE](../../LICENSE).
