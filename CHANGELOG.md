# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Reverse request channel (study→host) — reserved seam.** Designed (not yet
  built) the one envelope direction the protocol doesn't carry: a study→host
  request, for host-brokered model access, shared resources, and
  human-in-the-loop. Fixing its design now keeps it an additive *minor* later
  instead of forcing a breaking 2.0. The host reader now classifies inbound lines
  by **field** (`method` ⇒ request/notification, never a response), so a future
  reverse request can't be mistaken for a response or collide with the host's
  pending request ids — it is logged and safely dropped today. Reserves the
  `host_requests` capability and documents the invariants (field-based framing,
  per-direction id spaces, two-way negotiation) as the design of record. See
  [`docs/protocol.md`](docs/protocol.md#reverse-requests-studyhost) and
  [`specs/architecture.md`](specs/architecture.md).
- **Run cancellation** (`cancel`, protocol `1.8`) — a host can now abort one
  in-flight `run`/`execute`/`score` by its request `id` without tearing down the
  connection (previously the only lever was closing stdin, which ended *every*
  run at once). The aborted run resolves promptly with a `cancelled` error. This
  is the foundation for per-cell timeouts, hard cost caps, and fail-fast. New
  `cancel` capability; additive, so a study that doesn't implement it still
  interoperates. The Rust host exposes an explicit `HostHandle::cancel(id)` and
  also sends a best-effort `cancel` automatically when a `run` future is dropped
  (e.g. via `tokio::time::timeout` or a fail-fast `select!`). See
  [`docs/protocol.md`](docs/protocol.md#cancel).
- **Trials / repetitions + seed** — first-class N-sampling for pass@k, pass-rate,
  variance, and reproducibility, instead of faking them through an axis. Declare
  `Eval::trials(n)` (+ optional `Eval::seed(base)`) or override per-run with
  `mira run --trials N` / `--seed S`. The host repeats each cell `n` times; trial
  `t` runs with seed `base + t` (reproducible) and the subject reads it via
  `cx.seed()`. Trials are repetitions of one *logical* cell (unlike an axis, which
  forms new cells): a repeated cell's key gains a `#trial` suffix
  (`greet/hi@sim#2`), and the host groups them back by logical key. New
  `mira::aggregate` module is the **aggregation contract** — per-cell
  `TrialAggregate` (pass-rate, the unbiased `pass@k` estimator, score mean/σ),
  surfaced in the terminal report and as a `trials` array in the JSON record.
  Additive **protocol `1.6`**: optional `trial`/`trials`/`seed` on
  `RunParams`/`ScoreParams`/`RunResult`/`ExecuteResult`, optional
  `EvalInfo.trials`/`EvalInfo.seed`, and the `trials` capability. `examples/trials`
  demonstrates a seed-driven flaky agent.
- **Structured RPC errors** (protocol `1.5`) — the response `error` object is
  promoted from `{ message }` to the JSON-RPC-shaped
  `{ code, message, retryable, data }`. A protocol-level failure can now be
  *classified* (a `code`) and *retried* (a `retryable` hint) without parsing the
  human message: the host re-attempts retryable RPC errors just like infra-errored
  transcripts, and the study tags bad params / unknown methods with the standard
  JSON-RPC codes. All new fields are optional and defaulted, so a `1.4`-era peer
  that sends bare `{ "message": "…" }` still parses. The host request path carries
  the structured `RpcError` end-to-end (`mira::protocol::RpcError`, `codes`).
- **Python SDK protocol-metadata drift guard** — `codegen.py` now also generates
  `mira/_meta.py` (protocol version, method list, capability tokens) from
  `schema/v1/meta.json`, and the serve loop derives `PROTOCOL_VERSION` from it
  instead of hardcoding. New tests bind the SDK's handled methods and advertised
  capabilities to the generated metadata, so a new protocol method/capability (or
  a version bump) fails CI until the SDK tracks it. Closes the version/method/
  capability drift gaps that wire-type codegen alone didn't cover; `specs/sdks.md`
  §3 now states exactly what the guards do and don't catch.
- **Per-sample and per-model metadata on the wire** (protocol `1.7`) — `list` now
  carries `samples[].metadata` (repo, difficulty, dataset split, …) and
  `models[].metadata` (agent, underlying model, effort, price, sandbox, …)
  alongside the existing eval-level metadata. `Sample::meta` / `ModelSpec::meta`
  already existed; they now ride their own column through the protocol instead of
  being dropped, so config detail rides the model column and provenance rides the
  sample. Both maps are optional and default to empty — a `1.0`–`1.6` study still
  interoperates. The host surfaces them in `mira list`.
- **`mira run --group-by <key>` / `mira score --group-by <key>`** — break
  resolve-rate down by a metadata (or axis) key, e.g. `--group-by difficulty` or
  `--group-by repo`. Each cell's group value is resolved in order: axis `params`,
  sample metadata, model metadata, then transcript metadata. The breakdown prints
  to the terminal and is folded into the JSON (`groups` block), Markdown, and HTML
  reports (and the saved-run bundle).
- **Multimodal content** (`mira::content::Part`) — a typed vocabulary for non-text
  content (`text` / `image` / `audio` / `file` / `json`); media is *referenced*
  (`media_type` + `uri` or inline base64 `data`), so a part is plain JSON with no
  codecs in the core. **Multimodal inputs** land stable: `Sample::attachments`
  carries images/audio/files alongside the text turns, `Sample::prompt_parts()`
  fuses them into one ordered list for a subject, and `Sample::modalities()`
  reports the kinds — no protocol change (a `Sample` isn't a wire type). New
  runnable example `examples/multimodal/`. **Multimodal outputs**
  (`Transcript::output`, the `produced_modality` scorer) are **staged behind the
  `protocol-unstable` feature** — `Transcript` is a wire type, so they stay off
  the committed schema until promoted (see `specs/architecture.md` §14).
  `final_response` remains the canonical text projection throughout.
- **Interactive / multi-turn evals** — an `Eval` may carry a `.responder(..)` (a
  simulated user, `Fn(&[Message]) -> Option<Vec<Part>>`). The runner then drives
  a turn exchange — invoking the subject once per turn with the running
  conversation in `RunCx::conversation`, appending the responder's reply, until
  it ends or `max_turns` is hit — and folds the dialog into one transcript the
  scorers grade. **Stable, no protocol change** (the study owns the loop). New
  types `Message`/`Role`; example `examples/interactive/`.
- **Structured capability parameters** — `InitializeResult.capability_params`
  (`token → JSON`, read via `capability_param(token)`) lets a study advertise
  *config* a bare capability token can't carry (event kinds, supported
  input/output modalities, concurrency hints). Open-vocabulary like `metadata`.
  **Staged behind `protocol-unstable`** (a new typed wire field with no stable
  consumer yet); see `specs/architecture.md` §14.5.
- **Typed, correlated progress notifications** (protocol `1.9`) — `event` and
  `log` notifications now have typed, schematized payloads (`EventParams`,
  `LogParams`, published in `schema/v1/`) instead of an ad-hoc JSON bag. Each
  `event` carries a **`request_id`** correlating it to the originating
  `run`/`execute` request — the same demultiplexing key responses use — so a host
  can bind progress to a specific in-flight call even when many cells (or repeated
  trials of one cell) are multiplexed over the single pipe. `event.kind` is an
  open, growing vocabulary (`started`, `turn`, `tool_call`, `output`, `finished`),
  indexed in `meta.json` as `event_kinds`. Fully backward-compatible: a pre-`1.9`
  study's untyped events still parse (`request_id` defaults to `0`). Foundation
  for the live-streaming transcript view (`specs/architecture.md` §12).
- **Python SDK** (`sdks/python`) — a native, pure-stdlib library for authoring
  Mira eval studies in Python (no Rust dependency). Speaks the protocol over
  stdio; its wire types are **generated from `schema/v1/`** (the same contract
  the Rust host is generated from) so they can't drift, with a `codegen --check`
  drift guard mirroring the Rust one. Ergonomic authoring (`Study`,
  `@study.eval`, `Sample`, `model`, scorers, `transcript`) and a `serve()` loop
  handling `initialize`/`list`/`run`/`execute`/`score`. `examples/greet-python`
  now uses it. Design of record: `specs/sdks.md`. (TypeScript SDK planned, same
  shape.)
- **Environment metadata in saved runs** — `meta.json` now records the context a
  run came from in an `environment` block: git checkout (`HEAD` commit, branch,
  `dirty` flag), the box (`os`, `arch`, `hostname`, `cpus`, `mem_total_mib`), the
  `mira` host version, and free-form `labels` (auto-detected CI context plus any
  you configure). So a result can be told apart and compared across machines,
  commits, and CI runs (`mira::run::Environment`). Capture is **on by default**
  and best-effort — it never fails a run. Configure under `[environment]` in
  `mira.toml`: `enabled = false` to opt out, or `[environment.labels]` to stamp
  static labels (team, region, …) on every run.
- **Saved run archive** (`mira run --save` / `mira score --save`) — archives a
  run into a timestamped, self-contained folder so runs accumulate in a stable
  place and can be compared over time. Each run lands in
  `<results_dir>/<run_id>/` (run id `YYYYMMDDThhmmssZ-xxxx`, sortable by time)
  with `report.json`, `report.html`, and `meta.json` (run id, study,
  start/finish timestamps, and summary — `mira::run::RunMeta`). The results dir
  comes from `--save <dir>`, else `[results].dir` in the nearest `mira.toml`,
  else `./results`. Foundation for listing/diffing past runs (see
  `specs/architecture.md` §12).
- `just install` recipe to build and install the local `mira` CLI binary.
- **`mira help --full`** — an AI-friendly extended help screen: high-level
  overview, the full flag set, worked examples, and contact links (repository,
  issues, docs). Bare `mira` and `mira --help` now point to it in a footer so an
  agent can self-orient. The default tagline was reworded — the CLI is the
  *host*, not just a runner.
- **Machine-readable protocol schema** — the host↔study wire format is now
  emitted as JSON Schema (2020-12) artifacts under `schema/v1/` (`schema.json`,
  `meta.json`), generated from the `mira::protocol` types by the dedicated
  non-published `mira-schema-gen` tool (`just schema`). Polyglot studies can
  validate against these instead of hand-mirroring the Rust structs. CI fails if
  a protocol change lands without regenerating them (`--check`), and a validation
  suite checks real serialized messages against the committed schema.
- **`protocol-unstable` feature** — staging ground for *structural* protocol
  additions (a new typed field or method, which the open `metrics`/`metadata`/
  `capabilities` vocabularies can't express). Such additions land behind
  `#[cfg(feature = "protocol-unstable")]` and are excluded from the generated
  schema until promoted, so they can be trialled in-tree without freezing the
  stable contract.
- `CONTRIBUTING.md` guidance for the `main` branch-protection gate: require a PR
  and the `CI / Check` status check so a red CI run can no longer be merged.
- **Infrastructure errors → N/A, not failures.** A run now distinguishes a
  *failure* (the model under test got it wrong) from an *infrastructure error*
  (budget/quota, rate limit, provider 5xx/outage, network/timeout — not the
  model's fault), reusing the [`Score::na`] machinery so the two are consistent.
  - `ErrorKind` (`Subject` | `Infra`) classifies `Transcript.error`;
    `Transcript::infra_error(..)` and `Transcript::errored_infra()` are the seam.
  - Scoring **short-circuits to a single N/A score** on an infra error (the
    cell-level dual of a scorer returning `Score::na`), so the cell is excluded
    from the verdict and aggregate — neither passed nor failed, like a skip.
  - All-N/A cells are now treated as N/A (not failed) across **every** reporter
    (terminal, JSON `na` count, JUnit `<skipped>`, Markdown, HTML), and don't make
    `mira run` exit non-zero. (Previously only JUnit handled the all-N/A case.)
  - The concurrent executor re-queues infra-errored cells (alongside rate-limited
    ones) up to `--max-retries`. Protocol `1.3`: optional `transcript.error_kind`
    (additive — a study that omits it still interoperates).
  - `mira-everruns::classify_runtime_error` maps transient provider error strings
    (rate limit, quota, 5xx, timeout, network) to `Infra`; ambiguous errors stay
    subject-attributed.
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

### Changed

- **Open-ended `metadata`** — `Metadata` values widened from `String` to
  open-ended JSON (`serde_json::Value`), so evals, samples, models, and
  transcripts can carry structured context (numbers, bools, nested
  objects/arrays) — not just strings. The `.meta(key, value)` builders accept
  anything `Into<serde_json::Value>`, so existing string calls are unchanged.
  Matrix-axis `params` keep their dedicated `Params` (`string → string`) type —
  they form part of a cell's identity. Protocol bumps to **`1.4`** (additive:
  the field already existed, only its value type relaxed).

### Documentation

- **`specs/docs.md`** — design of record for public docs: structure, the
  SVG-diagram convention (carried over from everruns/everruns), writing rules,
  and doc/code sync. Referenced from `AGENTS.md` and `CONTRIBUTING.md`.
- **`docs/README.md`** — a single docs index/reading order.
- Reconciled `docs/protocol.md` to the current `PROTOCOL_VERSION` (`1.3`, was
  stated as `1.2`), restored the missing `timing`/`metrics` fields in the
  `Transcript` shown in `docs/subjects.md`, and aligned the README quick-start
  to invoke the example via `--example`.

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
