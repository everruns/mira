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

**Infrastructure errors vs. failures.** A cell can go wrong two ways, kept apart
so evals measure the model, not the weather. A **failure** is the model under
test getting it wrong (a scorer doesn't pass). An **infrastructure error** is the
scaffolding breaking (budget/quota, rate limit, provider 5xx/outage,
network/timeout): a subject signals it with `Transcript::infra_error` (vs.
`failed`), setting `error_kind = Infra`. Scoring then **short-circuits to a single
N/A score** — the cell-level dual of a scorer's `Score::na` — so the cell is
excluded from the verdict and aggregate (neither pass nor fail, like a skip) and
reported N/A across every renderer. The host's concurrent executor **retries**
infra-errored cells (alongside rate-limited ones) up to `--max-retries`; one that
stays broken stays N/A, never counted against the model. `mira-everruns`
classifies provider error strings into `Infra` conservatively.

**Arbitrary axes** beyond the model ship in v0.1: `Eval::axis(name, values)`
adds a discrete axis (e.g. reasoning `effort`, harness variant), and the runner
crosses every axis with the model matrix. The chosen value per cell reaches the
subject via `RunCx::param(name)`. Cell identity is `eval/sample@model` with a
sorted `[k=v,…]` suffix when axes vary (e.g.
`reasoning/puzzle@sim[effort=high]`), computed identically by host and study
(`mira::cell_key`).

**Trials & reproducibility.** pass@k, variance, and reproducibility are core eval
semantics, so N-sampling is first-class — not faked through an axis. `Eval::trials(n)`
(+ optional `Eval::seed(base)`), overridable per-run by `--trials` / `--seed`,
repeats each cell `n` times. Crucially, trials are **not an axis**: they're
repetitions of the *same* logical cell, so they don't cross-multiply with the
matrix and they're grouped back for aggregation. A trial carries `(index, count,
seed)` (`mira::Trial`) on the wire (`trial`/`trials`/`seed`, protocol `1.5`, all
additive) and reaches the subject via `RunCx::seed()`; seeding is deterministic
(`seed + index`) so a repetition set replays identically. A repeated cell's key
gains a `#index` suffix (`flaky/answer@sim#2`) while all trials share one *logical*
key (`RunResult::logical_key`); the host groups by that key. The **aggregation
contract** lives in `mira::aggregate`: a per-cell `TrialAggregate` with pass-rate,
the unbiased pass@k estimator (Chen et al.), and score mean/σ — surfaced in the
terminal report and as a `trials` array in the JSON record. Whether a flaky run is
"green" is still per-trial (a failing trial is a failure); a pass-rate *threshold*
is a future knob alongside the deferred cost caps (§12).

### Selective evaluation

Mirrors `cargo test`: a substring `filter` on the case key plus a `--tag` narrow
and a `--models` restriction. The **host** owns selection (it plans the grid from
`list` before running anything), independent of how evals are authored.

## 4. Execution model: two processes, one protocol

Eval *definitions* and the *runner* are split across a process boundary, talking
newline-delimited JSON over stdio, MCP-style. This is the core architectural
decision. Full wire reference: [`docs/protocol.md`](../docs/protocol.md).

- **study** — *your* eval program. Defines evals and calls
  `Study::new(…).serve()` / `Study::registered().serve()`. Owns subjects and
  scoring; knows nothing about selection, matrices, aggregation, checkpoints, or
  rendering. **Provider API keys live only here and never cross the wire.**
- **host** — the `mira` CLI. Compiles + spawns the study, enumerates evals
  (`initialize` + `list`), plans the run (selection × matrix), drives execution
  (`run`), then aggregates / saves / checkpoints / renders.

Three core methods (`initialize`, `list`, `run`) plus fire-and-forget
`event`/`log` notifications, and optional capability-gated extensions
(`execute`/`score` for run-now-score-later; `list_samples` to page a large or
lazily generated dataset whose samples don't fit one `list` line — the host
follows `EvalInfo.next_cursor` until exhausted). Models are addressed by
**label**; an unavailable cell is skipped. The boundary is the natural seam for
**polyglot studies** — any program in any language that speaks the protocol is a
valid study.

**Concurrency & adaptive throttling.** The host multiplexes many `run`s over the
single pipe (responses correlate by `id`; progress `event` notifications correlate
by a `request_id` in their payload, since a notification can't carry the envelope
`id`), and the study dispatches them on independent tasks. How many run at once is the host's call, smallest-wins across
three knobs: a **global** cap (`-j/--max-concurrent`), a **per-provider** cap
(`--provider-concurrency anthropic=2,…`), and **adaptive reduction** — a cell
whose result carries a rate-limit signal (HTTP 429, "overloaded", quota; see
`mira::is_rate_limited`) halves that provider's in-flight limit (AIMD) and is
re-queued after an exponential backoff, recovering one slot per success streak.
Cells bucket by the `list` `provider`, so one provider's limits can't be flooded
while others run flat-out. The policy lives in `mira::exec` (host side); the
study stays oblivious. `--no-adaptive` disables it; `sim` (offline) runs at the
global cap.

**Versioning & forward compatibility.** `initialize` advertises a
`MAJOR.MINOR` `protocol_version` plus a `capabilities` list. The contract: a
**major** bump is breaking (the host refuses a mismatched major); a **minor**
bump is additive. Every payload tolerates unknown fields (no
`deny_unknown_fields`) and adds new fields as `#[serde(default)]`, so a newer
study and an older host interoperate. Hosts feature-detect additively via
`capabilities` (`axes`, `events`, `usage`, `execute`, `score`, `paginate`)
rather than version sniffing.

## 5. Crate architecture

Decoupling the core from any provider SDK is deliberate: the core is light and
publishable; heavy integrations are separate, optional crates.

| Crate | Lib/bin | Role | Heavy deps |
|-------|---------|------|-----------|
| `mira-eval` | lib `mira` | Core: types, traits, scorers, `subject_fn`/`CliSubject`, protocol, study, host, runner, report. | none |
| `mira-cli` | bin `mira` | The host CLI. | none |
| `mira-everruns` | lib | `RuntimeSubject` over published `everruns-runtime`. | everruns |

The core takes **no everruns dependency**; `ModelSpec` is provider-agnostic and
`mira-everruns` maps it to an everruns `ResolvedModel`. This keeps a `cargo
install mira-cli` and `cargo add mira-eval` cheap, and lets the polyglot
`CliSubject` evaluate everruns CLIs with no compile-time coupling at all.

## 6. Developer experience

**Authoring** — an explicit builder; the `#[eval]` attribute (or `register_eval!`)
+ `Study::registered().serve()` for `cargo test`-style discovery across modules;
or an explicit `Study::new().eval(…).serve()`. `#[eval]` ships in the proc-macro
crate `mira-macros`, re-exported as `mira::eval` behind the default `macros`
feature.

**Running** — the `mira` CLI: `list`, `run [filter]`, `--tag`, `--models`,
`--format json|junit|md|html`, `--out`, `--checkpoint`/`--fresh`, and concurrency
controls `-j/--max-concurrent`, `--provider-concurrency`, `--no-adaptive`,
`--max-retries`. Non-zero exit on failure, so it drops into CI. In-process
`Runner` for evals as `#[tokio::test]`s.

## 7. Reporting, checkpoints & resume

The host owns all of this; the study only returns per-cell results.

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
- **Progress** — on an interactive terminal the host renders a live bar
  (`done/total`, elapsed, ETA, current cell). The total is exact: the host plans
  the full grid up front, so it's a count, not an estimate. Hidden under
  CI/non-TTY so it never pollutes logs.
- **Sessions & checkpoints** (`--checkpoint`) — the checkpoint is a first-class
  *session* record (`mira::session::Session`): run metadata (study, planned
  `total`, created/updated timestamps, per-eval definition fingerprints) plus the
  per-cell results, rewritten after each cell. A re-run loads it, skips done
  cells, and resumes the progress bar at the right `done/total` (`--fresh`
  ignores it). The fingerprints let a resume **warn when an eval's definition
  changed** (scorers/axes/models/samples/metadata/`max_turns`) so stale cached
  cells aren't silently reused. Resumable long matrix runs fall out of the host
  owning the plan.

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
captured files. Usage and timing stay **typed** because the shared budget scorers
depend on their shape; anything else is an **open vocabulary** — `metrics`, a
`string → f64` map a subject fills with custom numeric signals (recall@k,
energy_joules, …) and grades with `metric_within`/`metric_at_least`. The map is a
versioned, additive part of the wire (it bumped the protocol to `1.2`), but new
metric *keys* need no further protocol change. Subjects populate what they can
measure (`CliSubject` and
`RuntimeSubject` time the run; the event walker totals usage from JSONL). Budget
scorers (`tokens_within`, `cost_within`, `latency_within`, `ttft_within`,
`metric_within`, `tools_used_exactly`, `tool_called_before`, …) turn these into
pass/fail, and the JSON/HTML reports surface them per cell and in aggregate.

## 10. Delivered since the initial cut

The following were seams in the first draft and now ship in v0.1: the `#[eval]`
attribute macro (`mira-macros`), the self-contained HTML transcript viewer,
arbitrary matrix axes (`Eval::axis`), protocol versioning + capability
negotiation, and the operational metrics above.

## 11. Split execution and scoring (execute / score / rescore)

Running a subject and scoring its transcript are **separable phases**. The
`Scorer` trait already depends only on `(Sample, Transcript)`, never on the
subject — so the only coupling was operational: `run_cell` did both in one call,
and the only persisted artifact (the checkpoint) carried a *summary* transcript
with the raw `events`/`files` dropped, so a stored cell could be resumed but
never re-scored.

This matters for two real workflows:

- **Long-running subjects.** An agent run can take minutes to hours. We want to
  execute once, durably capture the *full* transcript (events, files, usage),
  and score later — without holding the subject process open or risking losing
  the run if scoring changes.
- **Re-scoring.** Scorers evolve (a rubric is tightened, a judge model swapped,
  a bug fixed). We want to re-run scoring over already-captured transcripts
  without paying to re-execute the subject.

**Design.** Keep execution artifacts separate from eval results, and split the
protocol's single `run` into two additive methods (`run` stays as the fused
convenience path):

- **`execute`** ([`RunParams`] → [`ExecuteResult`]) — runs the subject only and
  returns the **full** [`Transcript`] (events and files included). No scoring.
- **`score`** ([`ScoreParams`] = cell identity + a full transcript →
  [`RunResult`]) — runs the eval's scorers over a supplied transcript and
  returns the scored result. Stateless w.r.t. execution: the transcript comes in
  over the wire, so the host can replay a stored one.

Both are advertised via new capability tokens (`execute`, `score`) and land as a
**minor** protocol bump (`1.1`) — older studies that only implement `run` keep
working. The shared in-process seam is `runner::execute_cell` +
`runner::score_transcript`, with `run_cell` composing the two so in-process and
over-the-wire runs score identically (as before).

**Artifacts.** The host owns persistence (as with checkpoints). `mira run
--execute-only --artifacts <dir>` writes one full-transcript `ExecuteResult`
JSON per cell into `<dir>` (resumable: an existing artifact is skipped unless
`--fresh`). `mira score --artifacts <dir>` loads those, replays each through
`score`, and produces the normal report — re-running it is a re-score. Execution
artifacts (full transcripts) are thus stored **separately** from eval results
(scores), and either can be regenerated from the other's inputs.

## 12. Deferred (seams defined above)

Cost caps as a hard run limit (vs. a scorer), historical trend aggregation
across runs, and a live-streaming transcript view. Each has a defined seam and
does not require a breaking change to land. The transcript-view seam is now
half-built: `event` notifications carry a typed, schematized payload
(`EventParams`) with a growing `kind` vocabulary (`started`/`turn`/`tool_call`/
`output`/`finished`) and a `request_id` correlating each event to its run, so a
host can render per-cell progress live (protocol `1.5`).

**Run archive (landed seam).** `mira run --save` / `mira score --save` archive
each invocation into `<results_dir>/<run_id>/` (`report.json`, `report.html`, and
`meta.json` = `mira::run::RunMeta`: a sortable `YYYYMMDDThhmmssZ-xxxx` run id,
study, start/finish timestamps, and the rolled-up summary). The results dir
resolves from `--save <dir>`, else `[results].dir` in the nearest `mira.toml`,
else `./results`. A run id is per *invocation* (not per checkpoint), so resuming
a `--checkpoint` is still a fresh run with its own id/timestamps. This is the
data foundation for *historical trend aggregation*: the deferred `list`/`compare`
commands read these `meta.json` records and don't change their shape.

## 13. Machine-readable protocol schema

The wire protocol has a generated, language-neutral definition: JSON Schema
(2020-12) artifacts under `schema/v<major>/` (`schema.json` + `meta.json`),
emitted from the `mira::protocol` Rust types — the single source of truth — by
the non-published `mira-schema-gen` tool. The Rust types stay authoritative; the
schema is derived, never hand-written, so a polyglot study can validate against
it instead of mirroring the structs. CI regenerates and diffs (`--check`) so a
protocol change can't merge without a matching schema, and a validation suite
checks real serialized messages against the committed artifact.

**Stable vs. staged.** schemars derives sit behind `mira-eval`'s optional
`schema` feature (so default builds stay dep-light). The protocol extends
primarily through *open vocabularies* — `metrics` (numeric), `metadata`
(string), and `capabilities` tokens — which need no version bump. For the rarer
*structural* change (a new typed field or method), a `protocol-unstable` feature
stages it behind `#[cfg(feature = "protocol-unstable")]`; the generator builds
without it, so the committed schema describes only the stable protocol until the
addition is promoted (and earns its minor bump).

## 14. Multimodality, interactive evals, and capability parameters

Three limitations of the v0.1 cut, addressed together because they share a root:
the core types were *text-shaped* and *single-shot*. Status: multimodal **inputs**
and **interactive** evals are stable; multimodal **output** and structured
**capability parameters** are implemented but **staged behind `protocol-unstable`**
(they add typed fields to wire types — see §13 — and promote together once
concurrent protocol churn settles).

### 14.1 Content model (`Part`)

`mira::content::Part` is a small, typed vocabulary for one piece of content —
`Text`, `Image`, `Audio`, `File`, or `Json` (the structured-output escape
hatch). Media is **referenced, not embedded**: a media part carries a
`media_type` plus either a `uri` (URL or `data:` URI) or inline base64 `data`,
never raw bytes — so a `Part` is plain JSON that rides the wire and JSONL
datasets unchanged, and the core stays codec-free. The text fields
(`Sample::input`, `Transcript::final_response`) remain the canonical text path;
`Part` lists carry what text can't.

### 14.2 Multimodal inputs — stable, off-wire

A `Sample` gains `attachments: Vec<Part>` (images/audio/files/JSON alongside the
text turns); `Sample::prompt_parts()` fuses text turns + attachments into one
ordered `Part` list for a multimodal subject, and `Sample::modalities()` reports
the distinct kinds. This needs **no protocol change**: `Sample` is not a wire
type — the study owns the dataset and the host addresses samples by id — so the
schema, `PROTOCOL_VERSION`, and the SDKs are untouched. Example:
`examples/multimodal/`.

### 14.3 Multimodal outputs — staged behind `protocol-unstable`

`Transcript` (and its wire summary) gain `output: Vec<Part>` — the response as
typed parts, with `final_response` kept as the canonical text projection so
text-only scorers keep working. A modality scorer (`scorer::produced_modality`)
grades it. Because `Transcript` *is* a wire type (it rides in `execute`/`score`),
this lands behind `#[cfg(feature = "protocol-unstable")]` per §13 — exercised
in-tree (`cargo test --features protocol-unstable`, `clippy --all-features`) but
kept off the committed schema. **Promotion path:** drop the `cfg`s on
`Transcript::output` / `TranscriptSummary::output` / `produced_modality`,
regenerate `schema/`, mirror in the SDKs, and earn the minor bump — done as a
single focused change once concurrent protocol work settles, so it doesn't race
another version bump.

### 14.4 Interactive / multi-turn evals — implemented (in-process)

`Subject::run` still runs once per call, but an `Eval` may now carry a
`Responder` — a simulated user, `Fn(&[Message]) -> Option<Vec<Part>>`. When
present, `runner::execute_cell` drives a **turn exchange**: it invokes the
subject once per turn (handing it the running conversation via
`RunCx::conversation`), records the subject's `Assistant` turn, asks the
responder for the next `User` turn, and repeats until the responder returns
`None` or `max_turns` is hit. The turns are folded into one `Transcript` (last
response wins; usage/duration/tools/events/files/metrics accumulate), so:

- **Scoring is unchanged** — scorers grade the final accumulated `Transcript`.
- **No protocol change** — the study owns the loop; the host's `run`/`execute`
  call is identical, so this is stable and needs no wire feature. (A future
  *host-driven* exchange would add an additive `interactive` capability and
  per-turn `event` notifications, but the in-process driver covers the common
  simulated-user case.)

Example: `examples/interactive/` (a clarify-then-answer dialog). A
model-graded responder (an LLM playing the user) is just a closure that calls a
judge, no new machinery.

### 14.5 Capability parameters — implemented (staged)

`capabilities: Vec<String>` carries bare tokens; it can't express *config* (which
event kinds a study emits, supported input/output modalities, a concurrency
hint). `InitializeResult` gains a sibling `capability_params` map
(`token → JSON`, via `capability_param(token)`) — open-vocabulary like
`metadata`, so new keys never need a version bump. The study advertises it from
`initialize` (event kinds + supported modalities); a host reads it additively,
defaulting to today's behaviour when a token is absent. Staged behind
`protocol-unstable` (it's a new typed field on a wire type, and has no stable
consumer yet) per §13; **promotion path:** drop the `cfg`, regenerate `schema/`,
mirror in the SDKs, earn the minor bump — folded into the same promotion as
§14.3 once protocol churn settles.
