# How Mira works

A tour of the model behind Mira — the moving parts, how they fit together, and
why the framework is shaped the way it is. For a hands-on intro, start with
[getting started](getting-started.md); for the exact wire format, see the
[protocol reference](protocol.md).

## The core model

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix × axes
```

- **`Sample`** — one dataset row: input turns, an optional `target`, seeded
  `files`, `tags`, and `metadata`. Language-agnostic JSON; write it inline in
  Rust for small evals, or load `Dataset::{jsonl,json}` for larger sets.
- **`Subject`** — the thing under evaluation, one adapter per *shape*:
  - `subject_fn(closure)` — the in-process path.
  - `CliSubject` — an external binary; the **polyglot / other-language** path.
    It reads stdout or a canonical JSONL transcript and can seed/capture files.
  - `mira_everruns::RuntimeSubject` — drives a live `everruns-runtime` session.
- **`Transcript`** — the normalized result of a run: final response,
  iteration/tool counts, token + cost usage, tool names, captured files, raw
  events, and any error. Every subject produces the same shape, so scoring and
  reporting are shared.
- **`Scorer`** — `score(&Sample, &Transcript) -> Score` (a `value` in `0..1`, a
  `pass`, and a `reason`). Deterministic built-ins, operational budgets, an
  arbitrary-closure escape hatch, and `model_graded` LLM-as-judge — one open
  vocabulary, freely composed.
- **`Target`** — one matrix case. It is **provider-agnostic**: a `(label,
  provider, model, available, metadata)` tuple with no API keys and no SDK
  types. Subjects interpret it.

## Two processes, one protocol

The single most important design decision: eval *definitions* and the *runner*
live on opposite sides of a process boundary, talking newline-delimited JSON
over stdio (MCP-style).

- **study** — *your* eval program. It defines evals and calls
  `Study::registered().serve()` (or `Study::new(…).serve()`). It owns subjects
  and scoring and knows nothing about selection, matrices, saved runs, or
  rendering. **Provider API keys live only here and never cross the wire.**
- **host** — the `mira` CLI. It compiles and spawns the study, enumerates evals
  (`initialize` + `list`), plans the run (selection × matrix), drives execution
  (`run`), then aggregates, saves the run, and renders.

Three core methods (`initialize`, `list`, `run`) plus fire-and-forget
`event`/`log` notifications and optional capability-gated extensions
(`execute`/`score`, and `list_samples` to page large datasets). This boundary is
the natural seam for **polyglot studies** — any program in any language that
speaks the protocol is a valid study.

The protocol is versioned: `initialize` advertises a `MAJOR.MINOR`
`protocol_version` and a `capabilities` list. A major bump is breaking; a minor
bump is additive. Every payload tolerates unknown fields, so a newer study and
an older host interoperate.

## The matrix

The **target** is the first-class axis — the configured thing under evaluation.
For an LLM eval a `Target` *is* a model; for an agent eval it is a harness
(`Target::cli("yolop")`), optionally wrapping a model. The runner expands `evals
× targets × axes × samples` into independently-addressable cases. A missing API
key marks a case `available: false`, so it is **skipped, not failed** — a fresh
run is green offline.

**Arbitrary axes** beyond the target are first-class too: `Eval::axis(name,
values)` adds a discrete axis (reasoning effort, a retrieval setting, …) and the
runner crosses every axis with the target matrix. The chosen value per case
reaches the subject via `RunCx::param(name)`. (A harness like yolop-vs-codex can
be either a set of **targets** or its own **axis** — they compose.)

## Selecting what runs

Selection mirrors `cargo test`, and the **host** owns it — it plans the full grid
from `list` before running anything, independent of how the evals were authored:

- `filter` — a substring on the case key (`eval/sample@target`).
- `--tag` — only samples carrying the tag.
- `--targets a,b` — restrict the primary (target) axis; sugar for `--axis
  target=a,b`.
- `--axis NAME=v1,v2` (repeatable) — restrict **any** declared axis (`target` or
  a secondary axis). Values OR within a flag; multiple `--axis` flags AND. An
  unknown axis/value is a hard error.
- `--preset NAME` — apply a named selection bundle from `mira.toml`
  (`[presets.NAME]` = saved targets / axes / tag / filter / evals). Explicit
  flags override the preset.

Selection only ever **subsets** the grid the study declared — the host never adds
cases.

## Launching the study

The host has to *start* your study before it can enumerate or run it. Point it at
any study with `--bin NAME` / `--example NAME` (cargo), a non-Rust study via
`--uv` / `--python` / `--python3 SCRIPT` (or an arbitrary `--cmd "…"`), plus
`--package` / `--manifest-path`. To avoid retyping a repo's invocation on every
call, save it as a **named launcher** in `mira.toml`:

```toml
[launchers.greet]
bin = "greet"            # cargo run -q --bin greet  (+ optional package/manifest)

[launchers.py]
python3 = "study.py"     # a polyglot study (python3 study.py)

default_launcher = "greet"
```

- `--launcher NAME` selects `[launchers.NAME]`.
- `default_launcher` is used when neither a launch flag nor `--launcher` picks
  one, so a bare `mira run` just works.
- Explicit launch flags override the named launcher, mirroring `--preset`: an
  explicit **mode** (`--cmd`/`--bin`/`--example`/`--uv`/`--python`/`--python3`)
  replaces the named mode (the modes are mutually exclusive), and
  `--package`/`--manifest-path` overlay on top.

## Concurrency & adaptive throttling

The host multiplexes many `run`s over the single pipe (responses correlate by
`id`) and the study dispatches them on independent tasks. How many run at once
is the host's call, smallest-wins across three knobs: a **global** cap
(`-j/--max-concurrent`), a **per-provider** cap (`--provider-concurrency`), and
**adaptive reduction** — a case whose result carries a rate-limit signal halves
that provider's in-flight limit (AIMD) and is re-queued after exponential
backoff, recovering one slot per success streak. `--no-adaptive` disables it.

## Per-case timeout

A case can be given a **wall-clock budget**: when exceeded, the host gives up —
it drops the case's future (which best-effort `cancel`s the in-flight run over the
protocol, so an abandoned run stops burning cost) and records the case as failed
with a timeout error. A timeout is *not* retried (a retry would just burn the same
budget again) and counts as a target failure (red CI), distinct from an infra
error.

Set it three ways, first set wins:

- `mira run --timeout SECONDS` — applies to every target this run.
- `mira.toml` `[targets.LABEL].timeout` — per target (seconds).
- `mira.toml` `[presets.NAME].timeout` — a preset default.

```toml
[targets."anthropic/claude-opus-4-8"]
timeout = 300            # give up on a case for this target after 5 minutes

[presets.smoke]
timeout = 120            # preset default (overridden by the two above)
```

Unset everywhere ⇒ no time limit. The CLI flag wins over saved config (as
explicit flags do elsewhere); among saved config the more specific per-target
setting beats the preset default.

## Operational metrics

A `Transcript` carries the operational signals of a run, not just its text:
token usage (input/output plus cache-read and reasoning breakdowns and
`cost_usd`), wall-clock timing (`duration_ms`, `time_to_first_token_ms`), the
ordered list of tool calls, and captured files. Budget scorers
(`tokens_within`, `cost_within`, `latency_within`, `ttft_within`,
`tools_used_exactly`, …) turn these into pass/fail, and the JSON/HTML reports
surface them per case and in aggregate.

## Reporting, saved runs & resume

The host owns all reporting; the study only returns per-case results.

- **Terminal** — a per-case list with metrics, a model×eval pass-rate matrix,
  and totals. On an interactive terminal it also renders a live progress bar
  (the total is exact — the host planned the whole grid up front).
- **Canonical JSON** (`--format json`) — the machine-readable record the HTML
  viewer and trend aggregation consume.
- **HTML** (`--format html`) — a self-contained, dependency-free transcript
  viewer you can open straight from a CI artifact.
- **JUnit XML** (`--format junit`) and **Markdown** (`--format md`) — for CI
  test UIs and PR job summaries. Non-zero exit on failure drops it into CI.
- **Saved runs** (default) — every `run`/`score` writes a run folder
  `<results_dir>/<run_id>/` (`--dry-run` opts out): `meta.json` (run identity),
  `report.json`, `report.html`, and one `cases/<key>/result.json` per case,
  written atomically as it completes. `--resume <run_id>` reopens a run folder,
  subtracts the cases already recorded under `cases/`, and runs only what's
  missing. A fresh `run` mints a new id and reuses nothing, so stale results are
  never silently reused. `mira report <run_id>` re-renders a saved run's reports
  from its stored cases without spawning the study or re-executing anything.

## Crate layout

The core is deliberately decoupled from any provider SDK: light and
publishable, with heavy integrations as separate optional crates.

| Crate | Lib/bin | Role |
|-------|---------|------|
| `mira-eval` | lib `mira` | Core: types, traits, scorers, `subject_fn`/`CliSubject`, protocol, study, host, runner, report. |
| `mira-cli` | bin `mira` | The host CLI. |
| `mira-everruns` | lib | `RuntimeSubject` over the published `everruns-runtime`. |

The core takes **no everruns dependency** — `Target` is provider-agnostic and
`mira-everruns` maps it to an everruns model. This keeps `cargo install
mira-cli` and `cargo add mira-eval` cheap, and lets the polyglot `CliSubject`
evaluate everruns CLIs with no compile-time coupling at all.
