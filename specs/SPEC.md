# Spec: Mira — a Rust-first, code-first evaluation framework

Status: **implemented** (v0.1). This is the design of record; code should comply
or propose changes here.

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

`models` is a first-class axis. The runner expands `evals × models × samples`
into independently-addressable cells (`greet/hi@sim`). Missing API keys mark a
cell `available: false`, so it is **skipped, not failed** — the default run is
green offline. The design extends to arbitrary axes (reasoning effort, harness
variant) as a cross-product; the model axis ships in v0.1.

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

**Authoring** — an explicit builder; `register_eval!` + `serve_registered()` for
`cargo test`-style discovery across modules; or an explicit `serve(vec![…])`.

**Running** — the `mira` CLI: `list`, `run [filter]`, `--tag`, `--models`,
`--format json|junit|md`, `--out`, `--checkpoint`/`--fresh`. Non-zero exit on
failure, so it drops into CI. In-process `Runner` for evals as `#[tokio::test]`s.

## 7. Reporting, checkpoints & resume

The host owns all of this; the server only returns per-cell results.

- **Terminal** — per-case list + a model×eval pass-rate matrix + totals.
- **Canonical JSON** (`--format json`) — the machine-readable record a future
  `report.html` viewer and trend aggregation consume.
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

## 9. Deferred (seams defined above)

The `#[eval(models=[…])]` attribute macro (over `register_eval!`), a
self-contained `report.html` transcript viewer, arbitrary matrix axes, cost
caps as a run limit, and historical trend aggregation. Each has a defined seam
and does not require a breaking change to land.
