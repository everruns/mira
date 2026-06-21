# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Live progress bar** for `mira run` — shows `done/total`, elapsed time, ETA,
  and the current cell on an interactive terminal. The total is exact (the host
  plans the full grid up front). Hidden under CI/non-TTY so it doesn't pollute
  logs.
- **Evaluation sessions** (`mira::session::Session`) — `--checkpoint` now writes a
  first-class session record (study, planned `total`, created/updated timestamps,
  per-eval definition fingerprints, and per-cell results) instead of a bare
  results array. A resume reports accurate `done/total` progress and **warns when
  a cached cell is stale** because its eval definition changed
  (scorers/axes/models/samples/metadata/`max_turns`); `--fresh` recomputes.
- **Bounded, provider-aware, adaptive concurrency** for matrix runs. The host now
  multiplexes many `run` requests over the single study pipe (responses correlate
  by `id`) and the study dispatches them concurrently. New `mira run` flags:
  `-j/--max-concurrent` (global cap), `--provider-concurrency` (per-provider caps,
  e.g. `anthropic=2,openai=4`), `--no-adaptive`, and `--max-retries`.
- **Adaptive throttling**: a cell whose result carries a rate-limit signal (HTTP
  429, "overloaded", quota — see `mira::is_rate_limited`) halves its provider's
  in-flight limit (AIMD) and is re-queued after an exponential backoff, growing
  back as cells succeed — so a busy provider is throttled instead of hammered.
- `mira::exec` module (`Concurrency`, `CellSpec`, `run_cells`), `mira::HostHandle`
  (cheaply-cloneable concurrent client), and `mira::is_rate_limited`.
- **Split execution and scoring** (additive). Execution and scoring are now
  separable phases, for long-running subjects and re-scoring:
  - Protocol gains `execute` (run the subject only, returning the **full**
    transcript) and `score` (score a supplied transcript without re-executing),
    advertised via the `execute` / `score` capabilities. A `1.0`/`run`-only
    study still interoperates.
  - `mira run --execute-only --artifacts <dir>` captures one full-transcript
    artifact per cell (resumable; skips existing artifacts unless `--fresh`).
  - `mira score --artifacts <dir>` (re-)scores captured artifacts and reports —
    re-running it is a re-score, with no subject re-execution.
  - Library: `runner::execute_cell` / `runner::score_transcript` (with `run_cell`
    composing them), `Host::execute` / `Host::score`, and the `ExecuteResult` /
    `ScoreParams` protocol types.
- **Extensible metrics.** `Transcript.metrics` (`string → f64`) is an open
  vocabulary for custom numeric metrics a subject reports beyond the typed
  `Usage`/`Timing` (recall@k, energy_joules, p95 latency, …), with builder
  helpers `with_metric` / `record_metric` / `metric`. New generic budget scorers
  `metric_within(name, max)` and `metric_at_least(name, min)` grade them — adding
  a custom metric *key* needs no new protocol version or core change. Non-finite
  values (`NaN`/`±inf`) are dropped on record so reports stay serializable. The
  map carries through the wire (`TranscriptSummary`) and surfaces in the JSON/HTML
  reports.
- **`docs/metrics.md`** — the metrics model (typed vs. open) and a walkthrough
  for adding a custom metric; linked from the README, getting-started,
  extensibility, and scorers docs. The `metrics` example now reports and grades a
  custom `retrieval_recall@5` metric.
- Protocol bumped additively over `1.0`: `1.1` adds the optional
  `ModelInfo.provider` field (concurrency bucketing) and the `execute`/`score`
  methods + capabilities; `1.2` adds the optional `transcript.metrics` map. A
  `1.0` study still interoperates; `MIN_PROTOCOL_VERSION` stays `1.0`.
- **`Score::na` — a third scorer state.** Scorers can now report **N/A**
  ("couldn't evaluate", e.g. an unreachable judge or missing credentials)
  instead of crashing or scoring a misleading `fail`. N/A scores are excluded
  from the cell verdict (`verdict`) and aggregate; combinators ignore them and
  become N/A only when all inputs are; reports render them with a `–` glyph and
  an all-N/A cell counts as skipped in JUnit (never an empty failure).
- **`mira-judge` crate — provider-backed LLM-as-judge scorers.** An `LlmJudge`
  wired to real endpoints and exposed as an ordinary `Scorer`, over three
  transports: OpenAI Chat Completions (`openai_completions`), OpenAI Responses
  (`openai_responses`), and Anthropic Messages (`claude`). `Include` selects the
  graded surface (response / transcript+tools / full+metrics). Infra failures
  (no key, non-2xx, transport error, unparseable reply) degrade to N/A, so
  key-free runs stay green. Live-API tests are `#[ignore]`d and run in CI with
  keys from Doppler.
- **`examples/llm_judge`** — runnable example wiring `LlmJudge` alongside
  deterministic scorers (green offline, where the judge is N/A).

## [0.1.0] - 2026-06-20

Initial release.

### Added

- **Core framework (`mira-eval`, library `mira`)**
  - `Eval = Dataset(Sample…) + Subject + [Scorer…] × model matrix` model.
  - `Sample` / `Dataset` with inline authoring and JSONL / JSON loaders;
    seeded files, tags, targets, and free-form metadata.
  - Provider-agnostic `ModelSpec` (sim, anthropic, openai, gemini, custom) with
    API-key availability gating — unavailable cells skip rather than fail.
  - `Subject` trait with `subject_fn` (in-process) and `CliSubject` (the
    polyglot path: stdout or canonical JSONL `Event` transcripts, file
    seed/capture).
  - Rich `Transcript` metrics: token `Usage` (input/output plus `cache_read` /
    `reasoning` breakdowns and `cost_usd`) and wall-clock `Timing`
    (`duration_ms`, `time_to_first_token_ms`), populated by `CliSubject` /
    `RuntimeSubject` and the JSONL event walker.
  - `Scorer` trait with a broad built-in vocabulary: text (`contains`,
    `not_contains`, `equals`, `regex`, `matches_target`, `non_empty`,
    `json_valid`, `json_field_equals`); tools (`tool_called`, `tool_not_called`,
    `tool_calls_within`, `tools_used_exactly`, `tool_called_before`); budgets
    (`tokens_within`, `output_tokens_within`, `cost_within`, `turns_within`,
    `latency_within`, `ttft_within`); files (`file_exists`, `file_contains`);
    combinators (`all_of`, `any_of`, `not`); the `scorer(name, closure)` escape
    hatch; and `model_graded` LLM-as-judge.
  - Extra matrix axes beyond the model (`Eval::axis`), crossed with the model
    matrix; per-cell values reach subjects via `RunCx::param`. Stable cell keys
    via `mira::cell_key` (`eval/sample@model[k=v,…]`).
  - `#[eval]` attribute (crate `mira-macros`, re-exported as `mira::eval`) and
    `register_eval!` + `Study::registered()` for `cargo test`-style discovery.
  - The eval protocol (newline-delimited JSON over stdio): `Study` (the study)
    and `Host` (host), with `initialize` / `list` / `run`, progress notifications,
    `MAJOR.MINOR` versioning + capability negotiation, and forward-compatible
    (default/ignore-unknown) payloads.
  - In-process `Runner` with substring / tag / model selection.
  - Reporting: terminal matrix (with metrics), canonical JSON (per-case
    usage/timing + rolled-up totals), JUnit XML, Markdown, and a self-contained
    HTML transcript viewer.
- **Host CLI (`mira-cli`, binary `mira`)** — `list` / `run` with selection,
  `--models`, `--format json|junit|md|html`, `--out`, and resumable
  `--checkpoint`.
- **`#[eval]` proc-macro (`mira-macros`)** — the ergonomic registration attribute.
- **everruns adapter (`mira-everruns`)** — `RuntimeSubject` over the published
  `everruns-runtime`, plus `model_to_resolved`; integration-tested against the
  offline `LlmSim` driver.
- **Install** — Homebrew (`brew install everruns/tap/mira`) as the default, via
  the org-wide `everruns/homebrew-tap`: on release, prebuilt `mira` binaries
  (macOS arm64/x86_64, Linux x86_64) are published and the tap formula is
  updated. Also `cargo install mira-cli`.
- **Docs** — getting started, authoring, scorers, subjects, and a full protocol
  reference (`docs/protocol.md`).
- **Examples (`mira-examples`)** — `greet`, `coding`, `cli_subject`, `metrics`,
  `matrix`, `swe_bench`, `llmsim`.

[Unreleased]: https://github.com/everruns/mira/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/everruns/mira/releases/tag/v0.1.0
