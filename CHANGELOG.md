# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-06-22

Initial public release. The crates, the Python SDK, and the protocol all start
at this version.

### Core framework (`mira-eval`, library `mira`)

- **The model** — `Eval = Dataset(Sample…) + Subject + [Scorer…]` crossed with a
  **target** matrix. The thing under evaluation is a `Target` — a model *or* a
  harness (`Target::cli("yolop")`) — carrying provider-agnostic `(provider,
  model)` labels and no SDK types.
- **Datasets** — `Sample` / `Dataset` with inline authoring and JSONL / JSON
  loaders; seeded files, tags, an `expected` gold answer, and free-form metadata.
- **Subjects** — the `Subject` trait with `subject_fn` (in-process) and
  `CliSubject` (the polyglot path: stdout or canonical JSONL `Event` transcripts,
  file seed/capture).
- **Transcripts & metrics** — token `Usage` (input/output plus `cache_read` /
  `reasoning` breakdowns and `cost_usd`), wall-clock `Timing` (`duration_ms`,
  `time_to_first_token_ms`), and an open `metrics` map (`string → f64`) for custom
  numeric metrics (recall@k, energy_joules, p95 latency, …).
- **Multimodal** — a typed content vocabulary (`mira::content::Part`:
  `text`/`image`/`audio`/`file`/`json`; media referenced or inline base64).
  Inputs ride `Sample::attachments` (fused via `Sample::prompt_parts()`); outputs
  ride `Transcript::output`, graded by the `produced_modality` scorer.
  `final_response` stays the canonical text projection.
- **Scorers** — a broad built-in vocabulary: text (`contains`, `not_contains`,
  `equals`, `regex`, `matches_expected`, `non_empty`, `json_valid`,
  `json_field_equals`); tools (`tool_called`, `tool_not_called`,
  `tool_calls_within`, `tools_used_exactly`, `tool_called_before`); budgets
  (`tokens_within`, `output_tokens_within`, `cost_within`, `turns_within`,
  `latency_within`, `ttft_within`, plus generic `metric_within` /
  `metric_at_least`); files (`file_exists`, `file_contains`); combinators
  (`all_of`, `any_of`, `not`); the `scorer(name, closure)` escape hatch; and
  `model_graded` LLM-as-judge.
- **`Score::na` — a third scorer state.** A scorer can report **N/A** ("couldn't
  evaluate") instead of a misleading `fail`. N/A scores are excluded from the
  verdict and aggregate; combinators ignore them and become N/A only when all
  inputs are; every reporter renders them as a skip.
- **Infrastructure errors → N/A, not failures.** `ErrorKind` (`Subject` |
  `Infra`) distinguishes a model getting it wrong from budget/quota, rate limit,
  provider 5xx, or network/timeout. Scoring short-circuits to a single N/A on an
  infra error; the executor re-queues infra-errored cells (alongside rate-limited
  ones) up to `--max-retries`.
- **Selection axes** — extra matrix axes beyond the target (`Eval::axis`), crossed
  with the target matrix; per-cell values reach subjects via `RunCx::param`.
  Stable cell keys via `mira::cell_key` (`eval/sample@target[k=v,…]`).
- **Trials / repetitions + seed** — first-class N-sampling for pass@k, pass-rate,
  variance, and reproducibility. `Eval::trials(n)` (+ optional `Eval::seed(base)`)
  or `mira run --trials N` / `--seed S`; trial `t` runs with seed `base + t`, read
  via `cx.seed()`. The `mira::aggregate` module is the aggregation contract
  (pass-rate, the unbiased `pass@k` estimator, score mean/σ).
- **Interactive / multi-turn evals** — an `Eval` may carry a `.responder(..)`
  (a simulated user); the runner drives the turn exchange and folds the dialog
  into one transcript the scorers grade. Types `Message`/`Role`.
- **`#[eval]` attribute** (crate `mira-macros`, re-exported as `mira::eval`) and
  `register_eval!` + `Study::registered()` for `cargo test`-style discovery.
- **Runner & sessions** — in-process `Runner` with substring / tag / target
  selection; `--checkpoint` writes a first-class `Session` record (planned total,
  per-eval definition fingerprints, per-cell results) that resumes accurately and
  warns when a cached cell is stale; `--fresh` recomputes.
- **Bounded, provider-aware, adaptive concurrency** — the host multiplexes many
  `run` requests over the single study pipe (responses correlate by `id`).
  `-j/--max-concurrent`, `--provider-concurrency` (e.g. `anthropic=2,openai=4`),
  `--no-adaptive`, `--max-retries`. A rate-limit signal halves a provider's
  in-flight limit (AIMD) and re-queues with exponential backoff
  (`mira::is_rate_limited`, `mira::exec`, `mira::HostHandle`).
- **Split execution and scoring** — `execute` (run the subject only, returning the
  full transcript) and `score` (score a supplied transcript) are separable phases:
  `mira run --execute-only --artifacts <dir>` captures one artifact per cell;
  `mira score --artifacts <dir>` (re-)scores them without re-executing.
- **Reporting** — terminal matrix (with metrics), canonical JSON (per-case
  usage/timing + rolled-up totals, `trials`, `groups`), JUnit XML, Markdown, and a
  self-contained HTML transcript viewer. `--group-by <key>` breaks resolve-rate
  down by a metadata or axis key. Saved-run archive (`--save`) lands each run in a
  timestamped `<results_dir>/<run_id>/` with an `environment` block (git checkout,
  box, host version, labels).

### The eval protocol (1.0)

- Newline-delimited JSON over stdio between the **study** and the **host**:
  `initialize` / `list` / `list_samples` / `run` / `execute` / `score` / `cancel`,
  typed and correlated `event` / `log` notifications, `MAJOR.MINOR` versioning with
  capability negotiation, and forward-compatible (default / ignore-unknown)
  payloads.
- **Cancellation** (`cancel`) aborts one in-flight run by request `id` without
  tearing down the connection. **Paginated sample listing** (`list_samples` +
  `EvalInfo.next_cursor`) advertises large/lazy datasets without one giant `list`.
  **Structured RPC errors** (`{ code, message, retryable, data }`) classify and
  retry protocol-level failures. **Structured capability parameters**
  (`capability_params`) advertise config a bare token can't carry. Per-sample and
  per-target metadata ride their own columns on the wire.
- **Reverse request channel (study→host)** — the one envelope direction the
  protocol doesn't yet carry is reserved (not built): the host reader classifies
  inbound lines by field so a future reverse request can't collide with response
  ids; the `host_requests` capability and invariants are the design of record.
- **Machine-readable schema** — the wire format is emitted as JSON Schema
  (2020-12) under `schema/v1/` (`schema.json`, `meta.json`) by the non-published
  `mira-schema-gen` tool, with a `--check` drift guard in CI. The
  `protocol-unstable` feature stages structural additions out of the committed
  schema until promoted.

### SDKs

- **Python SDK** (`sdks/python`, package `mira-eval`) — a native, pure-stdlib
  library for authoring Mira studies in Python (no Rust dependency). Speaks the
  protocol over stdio; its wire types and protocol metadata are **generated from
  `schema/v1/`** with a `codegen --check` drift guard, and the serve loop derives
  `PROTOCOL_VERSION` from the generated metadata so it can't silently drift.
  Ergonomic authoring (`Study`, `@study.eval`, `Sample`, `target`, scorers,
  `transcript`) and a `serve()` loop. Design of record: `specs/sdks.md`.
  (TypeScript SDK planned, same shape.)

### Integrations

- **everruns adapter (`mira-everruns`)** — `RuntimeSubject` over the published
  `everruns-runtime`, plus `target_to_resolved`; integration-tested against the
  offline `LlmSim` driver. `classify_runtime_error` maps transient provider error
  strings to `Infra`.
- **`mira-judge`** — provider-backed LLM-as-judge scorers. An `LlmJudge` wired to
  real endpoints and exposed as an ordinary `Scorer`, over three transports:
  OpenAI Chat Completions (`openai_completions`), OpenAI Responses
  (`openai_responses`), and Anthropic Messages (`claude`). `Include` selects the
  graded surface. Infra failures degrade to N/A, so key-free runs stay green.

### Host CLI (`mira-cli`, binary `mira`)

- `list` / `run` / `score` with selection, generalized axis selection
  (`--axis NAME=v1,v2`, `--targets` sugar), named presets (`--preset` from
  `mira.toml`), `--format json|junit|md|html`, `--out`, resumable `--checkpoint`,
  and a live progress bar (TTY only). `mira help --full` is an AI-friendly
  extended help screen.

### Distribution

- **Homebrew** (`brew install everruns/tap/mira`) as the default install, via the
  org-wide `everruns/homebrew-tap`: on release, prebuilt `mira` binaries (macOS
  arm64/x86_64, Linux x86_64) are published and the tap formula is updated. Also
  `cargo install mira-cli`.

### Docs & examples

- **Public docs** (`docs/`) — getting started, authoring, scorers, subjects,
  metrics, and the normative protocol reference (`docs/protocol.md`), indexed by
  `docs/README.md`.
- **Specs** (`specs/`) — architecture, docs, SDKs, and release-process as the
  design of record.
- **Examples** — `greet`, `coding`, `cli_subject`, `metrics`, `matrix`, `trials`,
  `swe_bench`, `llmsim`, `llm_judge`, `infra`, `multimodal`, `interactive`, and
  the polyglot `greet-python`.

[Unreleased]: https://github.com/everruns/mira/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/everruns/mira/releases/tag/v0.1.0
