# Mira — architecture

- Status: **implemented** (v0.1)
- Authors: Mykhailo Chalyi, Everruns
- Origin: the `proposals/mira` PoC in [everruns/everruns#2345][poc] — the
  Rust-first eval-framework spec + runtime prototype, handed over to this repo
  and productionized (the prototype's in-core `RuntimeSubject` moved to the
  separate `mira-everruns` crate; the spec's deferred items — `#[eval]`,
  `CliSubject`, the HTML viewer, JUnit, arbitrary axes — now implemented).

[poc]: https://github.com/everruns/everruns/pull/2345

This is the design of record for Mira's architecture; code should comply or
propose changes here. Spec documents live under `specs/`, named by topic.
Related design lives in the everruns platform's [`specs/evals.md`][evals] (the
product/online eval subsystem); Mira is the dev-tool counterpart and shares its
scorer vocabulary.

[evals]: https://github.com/everruns/everruns/blob/main/specs/evals.md

## 1. Problem

Teams run agents and tools against datasets to measure quality, and they do it in
incompatible ways: a Python SWE-bench harness, a bespoke Rust string-check bench,
an rstest matrix in a product runtime. Consequences: providers reimplemented from
scratch, a second scorer vocabulary maintained per repo, non-Rust harnesses
siloed.

**Goal:** one Rust-first framework — a *developer tool* shaped like a test runner
— that all three can adopt, with code-first authoring, selective runs, model
matrices, and built-in reporting. Polyglot/other-language targets are a supported
secondary via a subprocess subject.

**Non-goal:** a product/online eval subsystem. Mira is a dev tool. A product's
runtime eval features may *share the scorer vocabulary* but are otherwise
distinct.

## 2. Prior art we match

- **Inspect AI** — `Task = dataset + solver + scorer`, model chosen at runtime,
  composable scorers, a transcript UI. The closest model; we follow it.
- **vitest-evals / Flue** — evals are *just tests*; deterministic asserts +
  judge in one file. Lesson: make evals feel like the test runner.
- **promptfoo / braintrust** — YAML matrix + CI regression; the experiment/PR-diff
  visualization bar, minus the SaaS lock-in.

Consensus: code-first, runtime model selection, composable deterministic +
LLM-judge scorers, transcript-level reporting, CI-native output.

## 3. Core model

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
```

- **`Sample`** — one dataset row: input turns, optional `target`, seeded
  `files`, `tags`, `metadata`. Language-agnostic JSON; inline in Rust for small
  evals; `Dataset::{jsonl,json}` for larger sets.
- **`Subject`** (trait) — the thing under evaluation, `async fn run(&Sample,
  &RunCx) -> Transcript`. One adapter per *shape*:
  - `subject_fn(closure)` — the in-process path.
  - `CliSubject` — an external binary; the **polyglot / other-language** path
    (reads stdout or a canonical JSONL `Event` transcript, seeds/captures
    files).
  - `mira_everruns::RuntimeSubject` — drives a live `everruns-runtime` session.
- **`Transcript`** — normalized run result: final response, iteration/tool
  counts, token+cost `Usage`, tool names, captured files, raw events, metadata,
  optional error. All subjects produce the same shape, so scorers and reporting
  are shared.
- **`Scorer`** (trait) — `async fn score(&Sample, &Transcript) -> Score` (`value`
  `0..1`, `pass`, `reason`). Deterministic built-ins, the `scorer(name, closure)`
  escape hatch, and `model_graded(rubric, judge)`. One open vocabulary, not a
  closed enum.
- **`ModelSpec`** — one matrix cell. **Provider-agnostic**: `(label, provider,
  model, available, metadata)`, no keys, no SDK types. Subjects interpret it.

### Matrix

`models` is a first-class axis. The runner expands `evals × models × axes ×
samples` into independently-addressable cells. Missing API keys mark a cell
`available: false`, so it is **skipped, not failed** — the default run is green
offline.

**Arbitrary axes** beyond the model ship in v0.1: `Eval::axis(name, values)`
adds a discrete axis (e.g. reasoning `effort`, harness variant), and the runner
crosses every axis with the model matrix. The chosen value per cell reaches the
subject via `RunCx::param(name)`. Cell identity is `eval/sample@model` with a
sorted `[k=v,…]` suffix when axes vary (e.g.
`reasoning/puzzle@sim[effort=high]`), computed identically by host and server
(`mira::cell_key`).

### Selective evaluation

Mirrors `cargo test`: a substring `filter` on the case key plus a `--tag` narrow
and a `--models` restriction. The **host** owns selection (it plans the grid from
`list` before running anything), independent of how evals are authored.

## 4. Execution model: two processes, one protocol

Eval *definitions* and the *runner* are split across a process boundary, talking
newline-delimited JSON over stdio, MCP-style. This is the core architectural
decision. Full wire reference: [`docs/protocol.md`](../docs/protocol.md).

- **server** — *your* eval program. Defines evals and calls `mira::serve` /
  `serve_registered`. Owns subjects and scoring; knows nothing about selection,
  matrices, aggregation, checkpoints, or rendering. **Provider API keys live only
  here and never cross the wire.**
- **host** — the `mira` CLI. Compiles + spawns the server, enumerates evals
  (`initialize` + `list`), plans the run (selection × matrix), drives execution
  cell-by-cell (`run`), then aggregates / saves / checkpoints / renders.

Three methods (`initialize`, `list`, `run`) plus fire-and-forget `event`/`log`
notifications. Models are addressed by **label**; an unavailable cell is skipped.
The boundary is the natural seam for **polyglot servers** — any program in any
language that speaks the protocol is a valid server.

**Versioning & forward compatibility.** `initialize` advertises a
`MAJOR.MINOR` `protocol_version` plus a `capabilities` list. The contract: a
**major** bump is breaking (the host refuses a mismatched major); a **minor**
bump is additive. Every payload tolerates unknown fields (no
`deny_unknown_fields`) and adds new fields as `#[serde(default)]`, so a newer
server and an older host interoperate. Hosts feature-detect additively via
`capabilities` (`axes`, `events`, `usage`) rather than version sniffing.

## 5. Crate architecture

Decoupling the core from any provider SDK is deliberate: the core is light and
publishable; heavy integrations are separate, optional crates.

| Crate | Lib/bin | Role | Heavy deps |
|-------|---------|------|-----------|
| `mira-eval` | lib `mira` | Core: types, traits, scorers, `subject_fn`/`CliSubject`, protocol, server, host, runner, report. | none |
| `mira-cli` | bin `mira` | The host CLI. | none |
| `mira-everruns` | lib | `RuntimeSubject` over published `everruns-runtime`. | everruns |

The core takes **no everruns dependency**; `ModelSpec` is provider-agnostic and
`mira-everruns` maps it to an everruns `ResolvedModel`. This keeps a `cargo
install mira-cli` and `cargo add mira-eval` cheap, and lets the polyglot
`CliSubject` evaluate everruns CLIs with no compile-time coupling at all.

## 6. Developer experience

**Authoring** — an explicit builder; the `#[eval]` attribute (or `register_eval!`)
+ `serve_registered()` for `cargo test`-style discovery across modules; or an
explicit `serve(vec![…])`. `#[eval]` ships in the proc-macro crate `mira-macros`,
re-exported as `mira::eval` behind the default `macros` feature.

**Running** — the `mira` CLI: `list`, `run [filter]`, `--tag`, `--models`,
`--format json|junit|md|html`, `--out`, `--checkpoint`/`--fresh`. Non-zero exit
on failure, so it drops into CI. In-process `Runner` for evals as
`#[tokio::test]`s.

## 7. Reporting, checkpoints & resume

The host owns all of this; the server only returns per-cell results.

- **Terminal** — per-case list (with token/cost/latency/tool metrics) + a
  model×eval pass-rate matrix + totals.
- **Canonical JSON** (`--format json`) — the machine-readable record, with
  per-case usage/timing and rolled-up totals, that the HTML viewer and trend
  aggregation consume.
- **HTML** (`--format html`) — a self-contained, dependency-free transcript
  viewer (inline CSS, the JSON record embedded): summary banner, matrix, and a
  per-case breakdown of scores, usage, timing, tools, and metadata links. Open
  it straight from a CI artifact.
- **JUnit XML** (`--format junit`) — surfaces evals in any CI test UI.
- **Markdown** (`--format md`) — for PR job summaries.
- **Checkpoints** (`--checkpoint`) — each completed cell persists as it finishes;
  a re-run loads it and skips done cells (`--fresh` ignores it). Resumable long
  matrix runs fall out of the host owning the plan.

## 8. Migration paths

- **everruns** — collapse the `llm-tests` matrix into evals using
  `mira_everruns::RuntimeSubject`; keep any product eval subsystem separate but
  share the scorer vocabulary.
- **bashkit** — a `Tool`-in-a-loop becomes a `ToolSubject` (a thin custom
  `Subject`); JSONL datasets load unchanged.
- **yolop / SWE-bench** — the harness runs *as* a `CliSubject` during transition
  (proving the polyglot path immediately); the Docker `FAIL_TO_PASS` check
  becomes a custom scorer.

## 9. Metrics

A `Transcript` carries the operational signals of a run, not just its text:
token `Usage` (input/output plus `cache_read`/`reasoning` breakdowns and
`cost_usd`), wall-clock `Timing` (`duration_ms`, `time_to_first_token_ms`), the
ordered list of tool calls (so the exact set and ordering are scorable), and
captured files. Subjects populate what they can measure (`CliSubject` and
`RuntimeSubject` time the run; the event walker totals usage from JSONL). Budget
scorers (`tokens_within`, `cost_within`, `latency_within`, `ttft_within`,
`tools_used_exactly`, `tool_called_before`, …) turn these into pass/fail, and the
JSON/HTML reports surface them per cell and in aggregate.

## 10. Delivered since the initial cut

The following were seams in the first draft and now ship in v0.1: the `#[eval]`
attribute macro (`mira-macros`), the self-contained HTML transcript viewer,
arbitrary matrix axes (`Eval::axis`), protocol versioning + capability
negotiation, and the operational metrics above.

## 11. Deferred (seams defined above)

Cost caps as a hard run limit (vs. a scorer), historical trend aggregation
across runs, and a live-streaming transcript view. Each has a defined seam and
does not require a breaking change to land.
