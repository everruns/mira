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
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  target matrix
```

- **`Sample`** — one dataset row: input turns, optional `expected` answer, seeded
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
- **`Target`** — one matrix case. **Provider-agnostic**: `(label, provider,
  model, available, metadata)`, no keys, no SDK types. Subjects interpret it.

### Matrix

The **target** (the model or harness under evaluation; see §15) is the
first-class axis. The runner expands `evals × targets × axes ×
samples` into independently-addressable cases. Missing API keys mark a case
`available: false`, so it is **skipped, not failed** — the default run is green
offline.

**Infrastructure errors vs. failures.** A case can go wrong two ways, kept apart
so evals measure the model, not the weather. A **failure** is the model under
test getting it wrong (a scorer doesn't pass). An **infrastructure error** is the
scaffolding breaking (budget/quota, rate limit, provider 5xx/outage,
network/timeout): a subject signals it with `Transcript::infra_error` (vs.
`failed`), setting `error_kind = Infra`. Scoring then **short-circuits to a single
N/A score** — the case-level dual of a scorer's `Score::na` — so the case is
excluded from the verdict and aggregate (neither pass nor fail, like a skip) and
reported N/A across every renderer. The host's concurrent executor **retries**
infra-errored cases (alongside rate-limited ones) up to `--max-retries`; one that
stays broken stays N/A, never counted against the model. `mira-everruns`
classifies provider error strings into `Infra` conservatively.

**Arbitrary axes** beyond the model ship in v0.1: `Eval::axis(name, values)`
adds a discrete axis (e.g. reasoning `effort`, harness variant), and the runner
crosses every axis with the target matrix. The chosen value per case reaches the
subject via `RunCx::param(name)`. Case identity is `eval/sample@target` with a
sorted `[k=v,…]` suffix when axes vary (e.g.
`reasoning/puzzle@sim[effort=high]`), computed identically by host and study
(`mira::case_key`).

**Trials & reproducibility.** pass@k, variance, and reproducibility are core eval
semantics, so N-sampling is first-class — not faked through an axis. `Eval::trials(n)`
(+ optional `Eval::seed(base)`), overridable per-run by `--trials` / `--seed`,
repeats each case `n` times. Crucially, trials are **not an axis**: they're
repetitions of the *same* logical case, so they don't cross-multiply with the
matrix and they're grouped back for aggregation. A trial carries `(index, count,
seed)` (`mira::Trial`) on the wire (`trial`/`trials`/`seed`, all additive) and
reaches the subject via `RunCx::seed()`; seeding is deterministic
(`seed + index`) so a repetition set replays identically. A repeated case's key
gains a `#index` suffix (`flaky/answer@sim#2`) while all trials share one *logical*
key (`RunResult::logical_key`); the host groups by that key. The **aggregation
contract** lives in `mira::aggregate`: a per-case `TrialAggregate` with pass-rate,
the unbiased pass@k estimator (Chen et al.), and score mean/σ — surfaced in the
terminal report and as a `trials` array in the JSON record. Whether a flaky run is
"green" is still per-trial (a failing trial is a failure); a pass-rate *threshold*
is a future knob alongside the deferred cost caps (§12).

### Selective evaluation

Mirrors `cargo test`: a substring `filter` on the case key, a `--tag` narrow, a
`--targets` restriction (sugar for `--axis target=…`), a general `--axis
NAME=v1,v2` subset on any declared axis, and `--preset NAME` (a saved selection
bundle from `mira.toml`). The **host** owns selection (it plans the grid from
`list` before running anything), and only ever *subsets* the declared grid —
independent of how evals are authored. See §15.3.

## 4. Execution model: two processes, one protocol

Eval *definitions* and the *runner* are split across a process boundary, talking
newline-delimited JSON over stdio, MCP-style. This is the core architectural
decision. Full wire reference: [`docs/protocol.md`](../docs/protocol.md).

- **study** — *your* eval program. Defines evals and calls
  `Study::new(…).serve()` / `Study::registered().serve()`. Owns subjects and
  scoring; knows nothing about selection, matrices, aggregation, saved runs, or
  rendering. **Provider API keys live only here and never cross the wire.**
- **host** — the `mira` CLI. Compiles + spawns the study, enumerates evals
  (`initialize` + `list`), plans the run (selection × matrix), drives execution
  (`run`), then aggregates / saves the run / renders.

Three core methods (`initialize`, `list`, `run`) plus fire-and-forget
`event`/`log` notifications, and optional capability-gated extensions
(`execute`/`score` for run-now-score-later; `list_samples` to page a large or
lazily generated dataset whose samples don't fit one `list` line — the host
follows `EvalInfo.next_cursor` until exhausted). Models are addressed by
**label**; an unavailable case is skipped. The boundary is the natural seam for
**polyglot studies** — any program in any language that speaks the protocol is a
valid study.

**Concurrency & adaptive throttling.** The host multiplexes many `run`s over the
single pipe (responses correlate by `id`; progress `event` notifications correlate
by a `request_id` in their payload, since a notification can't carry the envelope
`id`), and the study dispatches them on independent tasks. How many run at once is the host's call, smallest-wins across
three knobs: a **global** cap (`-j/--max-concurrent`), a **per-provider** cap
(`--provider-concurrency anthropic=2,…`), and **adaptive reduction** — a case
whose result carries a rate-limit signal (HTTP 429, "overloaded", quota; see
`mira::is_rate_limited`) halves that provider's in-flight limit (AIMD) and is
re-queued after an exponential backoff, recovering one slot per success streak.
Cases bucket by the `list` `provider`, so one provider's limits can't be flooded
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

The core takes **no everruns dependency**; `Target` is provider-agnostic and
`mira-everruns` maps it to an everruns `ResolvedModel`. This keeps a `cargo
install mira-cli` and `cargo add mira-eval` cheap, and lets the polyglot
`CliSubject` evaluate everruns CLIs with no compile-time coupling at all.

## 6. Developer experience

**Authoring** — an explicit builder; the `#[eval]` attribute (or `register_eval!`)
+ `Study::registered().serve()` for `cargo test`-style discovery across modules;
or an explicit `Study::new().eval(…).serve()`. `#[eval]` ships in the proc-macro
crate `mira-macros`, re-exported as `mira::eval` behind the default `macros`
feature.

**Running** — the `mira` CLI: `list`, `run [filter]`, `report <run_id>`, `--tag`,
`--targets`, `--axis`, `--preset`, `--format json|junit|md|html`, `--out`,
`--dry-run`/`--resume <run_id>`, and concurrency controls `-j/--max-concurrent`,
`--provider-concurrency`, `--no-adaptive`, `--max-retries`. Non-zero exit on
failure, so it drops into CI. In-process `Runner` for evals as `#[tokio::test]`s.

## 7. Reporting, run folders & resume

The host owns all of this; the study only returns per-case results.

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
  (`done/total`, elapsed, ETA, current case). The total is exact: the host plans
  the full grid up front, so it's a count, not an estimate. Hidden under
  CI/non-TTY so it never pollutes logs.
- **Run folders & resume** — every `run`/`score` is **saved by default** under
  `<results_dir>/<run_id>/` (opt out with `--dry-run`). The run folder is the
  durable unit: `meta.json` (run identity — a header written at start, finalized
  at the end with the finish time and summary), `report.json`/`report.html`, and
  one `cases/<key>/result.json` per finished case, written atomically (temp +
  rename) as it lands. `mira run --resume <run_id>` reopens that folder, skips the
  cases already recorded under `cases/`, and runs only what's missing — so an
  interrupted long matrix run finishes in place. Resume is **explicit**: a fresh
  run mints a new id and reuses nothing, so there's no silent reuse of stale
  results. `mira report <run_id>` re-renders a saved run's reports from its stored
  results with no study process and no re-execution. Resumable runs fall out of
  the host owning both the plan and the run folder.

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
versioned, additive part of the wire, but new metric *keys* need no further
protocol change. Subjects populate what they can
measure (`CliSubject` and
`RuntimeSubject` time the run; the event walker totals usage from JSONL). Budget
scorers (`tokens_within`, `cost_within`, `latency_within`, `ttft_within`,
`metric_within`, `tools_used_exactly`, `tool_called_before`, …) turn these into
pass/fail, and the JSON/HTML reports surface them per case and in aggregate.

## 10. Delivered since the initial cut

The following were seams in the first draft and now ship in v0.1: the `#[eval]`
attribute macro (`mira-macros`), the self-contained HTML transcript viewer,
arbitrary matrix axes (`Eval::axis`), protocol versioning + capability
negotiation, and the operational metrics above.

## 11. Split execution and scoring (execute / score / rescore)

Running a subject and scoring its transcript are **separable phases**. The
`Scorer` trait already depends only on `(Sample, Transcript)`, never on the
subject — so the only coupling was operational: `run_case` did both in one call,
and a saved case result carries a *summary* transcript with the raw
`events`/`files` dropped, so a stored case can be resumed but not re-scored from
the run folder alone (full re-scoring uses execution artifacts; see below).

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
- **`score`** ([`ScoreParams`] = case identity + a full transcript →
  [`RunResult`]) — runs the eval's scorers over a supplied transcript and
  returns the scored result. Stateless w.r.t. execution: the transcript comes in
  over the wire, so the host can replay a stored one.

Both are advertised via capability tokens (`execute`, `score`) and are additive —
older studies that only implement `run` keep working. The shared in-process seam is `runner::execute_case` +
`runner::score_transcript`, with `run_case` composing the two so in-process and
over-the-wire runs score identically (as before).

**Artifacts.** The host owns persistence (as with saved runs). `mira run
--execute-only --artifacts <dir>` writes one full-transcript `ExecuteResult`
JSON per case into `<dir>` (resumable: an existing artifact is skipped; delete the
dir to force a fresh run). `mira score --artifacts <dir>` loads those, replays each through
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
host can render per-case progress live.

**Reverse request channel (study→host) — reserved seam.** Every request flows
host→study today; the study is self-contained and keys live study-side by design.
The one direction the protocol doesn't carry is the *reverse* request — the study
asking the host for something mid-run. It is the single addition that introduces a
new envelope direction, and so the one most likely to force a breaking 2.0 if
retrofitted carelessly; we fix its design now even though we don't build it.
Motivating cases: **host-brokered model access** (central credentials, caching,
budgeting instead of per-study keys), **shared resources** the host owns (sandbox,
fixtures), and **human-in-the-loop** (pause a case to ask the operator). The
framing already admits it as a *minor*, additive change, guaranteed by three
invariants: (1) **field-based message classification** — a line bearing `method`
is a request/notification, never a response, so a reverse request on the study's
stdout is unambiguous to a host that predates it; (2) **independent `id` spaces
per direction**, correlated only with responses flowing back the same way, so host
and study ids can overlap without collision; (3) **two-way capability negotiation**
— off unless the host advertises support in `initialize.params` *and* the study
advertises the reserved `host_requests` capability. The host already classifies
inbound lines this way (`host::classify`) and safely ignores any reverse request
rather than letting its id corrupt response routing, so the seam is exercised, not
theoretical; the concrete reverse methods would stage behind `protocol-unstable`.
Full design: [`docs/protocol.md`](../docs/protocol.md#reverse-requests-studyhost).

**Run archive (landed seam).** `mira run` / `mira score` save every invocation
into `<results_dir>/<run_id>/` by default (opt out with `--dry-run`):
`report.json`, `report.html`, `meta.json` (= `mira::run::RunMeta`: a sortable
`YYYYMMDDThhmmssZ-xxxx` run id, study, start/finish timestamps, environment, and
the rolled-up summary — written as a header at start, finalized at the end), and
`cases/<key>/result.json` per finished case. The results dir resolves from
`[results].dir` in the nearest `mira.toml`, else `./results`. A run id names a run
folder: `mira run --resume <run_id>` reopens it and runs only the missing cases,
and `mira report <run_id>` re-renders it without a study. This is the data
foundation for *historical trend aggregation*: the deferred `list`/`compare`
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
the core types were *text-shaped* and *single-shot*. All are now on the stable
contract: multimodal **inputs** and **interactive** evals never needed the wire;
multimodal **output** and structured **capability parameters** were trialled
behind `protocol-unstable` and **promoted onto the committed `1.0` wire** (typed
`Part`s on the transcript + `InitializeResult.capability_params`).

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

### 14.3 Multimodal outputs — stable

`Transcript` (and its wire summary) carry `output: Vec<Part>` — the response as
typed parts, with `final_response` kept as the canonical text projection so
text-only scorers keep working. A modality scorer (`scorer::produced_modality`)
grades it. Because `Transcript` *is* a wire type (it rides in `execute`/`score`),
this was trialled behind `protocol-unstable` first, then **promoted onto the
committed `1.0` wire**: the committed `schema/` publishes `output` plus the
`Part` / `Source` defs,
and the SDKs mirror them (the Python codegen renders the `Part`/`Source` object
unions as pass-through dicts — the wire is `kind`-tagged JSON). `final_response`
stays the text projection throughout, so nothing text-only had to change.

### 14.4 Interactive / multi-turn evals — implemented (in-process)

`Subject::run` still runs once per call, but an `Eval` may now carry a
`Responder` — a simulated user, `Fn(&[Message]) -> Option<Vec<Part>>`. When
present, `runner::execute_case` drives a **turn exchange**: it invokes the
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

### 14.5 Capability parameters — stable

`capabilities: Vec<String>` carries bare tokens; it can't express *config* (which
event kinds a study emits, supported input/output modalities, a concurrency
hint). `InitializeResult` carries a sibling `capability_params` map
(`token → JSON`, via `capability_param(token)`) — open-vocabulary like
`metadata`, so new *keys* never need a version bump. The study advertises it from
`initialize` (event kinds + supported modalities); a host reads it additively,
defaulting to today's behaviour when a token is absent. Trialled behind
`protocol-unstable`, then **promoted onto the committed `1.0` wire** alongside
§14.3 (the field is a typed wire addition, so adding the field itself — unlike
adding keys — needed the staging path before promotion).

## 15. Targets, not models (the comparison axis) and axis selection

- Status: **implemented** (supersedes the `ModelSpec` / `--models` naming used in
  the historical prose of §3/§6; the code, the `1.0` wire, `schema/`, the SDKs,
  docs, and examples are renamed to match).

### 15.1 Problem — `model` is the wrong name for the privileged axis

`ModelSpec` does **two** jobs and only one justifies the name:

1. **Provider/cost descriptor** — `(provider, model)` drives real machinery:
   availability gating on API keys (`available:false` ⇒ skip), per-provider
   concurrency (`--provider-concurrency`), and cost/usage accounting. This part
   genuinely *is* about models.
2. **The privileged comparison axis** — the one dimension the host lets you
   select from the CLI (`--models`) and that gets the prominent `@…` slot in the
   case key. This has nothing to do with providers or keys.

Job 2 is welded to the *name* "model", so anything you want to compare and
select from the CLI has to masquerade as a model. Comparing two coding agents
reads wrong: `mira run --models yolop,codex`. yolop and codex aren't models —
they're harnesses / *individual configs* (yolop's term). The smell: only the
model axis is host-selectable, and it's misnamed.

### 15.2 B — rename the privileged axis to `Target`

The case of the privileged axis is the **configured thing under evaluation** —
for an LLM eval it's a model; for an agent eval it's a harness (optionally
wrapping a model). Call it a **`Target`**. (`subject` is taken — that's the
trait that *executes* a sample into a transcript; the target is the *config* it
runs.)

`Target` keeps the same shape as `ModelSpec` — the provider/cost fields earn
their keep (job 1) — and the LLM constructors are unchanged bar the type name. A
new `Target::cli(label)` covers the harness case (a `CliSubject` dispatches on
`cx.target.label`):

```rust
pub struct Target {
    pub label: String,      // selection + display key: "yolop", "anthropic/opus"
    pub provider: String,   // routing / concurrency bucket / gating key
    pub model: String,      // underlying model id; "" for a pure harness
    pub available: bool,    // false (e.g. missing key) ⇒ skipped, not failed
    pub metadata: Metadata,
}

Target::sim()                          // offline default
Target::anthropic("claude-opus-4-8")   // gated on ANTHROPIC_API_KEY (unchanged)
Target::cli("yolop")                   // harness target; provider="cli", available
```

For yolop-on-opus vs codex-on-opus as *one individual config each*, enumerate
targets directly (yolop's model — when it matters for cost — rides `model` or
`metadata`). To cross harness × model instead, keep harness on `target` and put
model on an **axis** (§15.3) — they compose.

Renames (pre-1.0, no back-compat per AGENTS.md — clean rename, no aliases):

| Was | Now |
|-----|-----|
| `ModelSpec` | `Target` |
| `Eval::models([…])` | `Eval::targets([…])` |
| `RunCx::model` / `cx.model` | `RunCx::target` / `cx.target` |
| `--models a,b` | `--targets a,b` |
| case key `eval/sample@model` | `@<target label>` (mechanically unchanged: still the label) |

`provider` stays a **field of a target**: concurrency bucketing, gating, and
cost attribution still key on it (a harness target sets `provider="cli"` or its
own bucket). The core stays provider-agnostic — `mira-everruns` maps a `Target`
to a `ResolvedModel` exactly as before.

### 15.3 A — make any axis host-selectable (`--axis`)

Today only the model axis is selectable. Generalize: a `--axis <name>=<v1>,<v2>`
flag (repeatable) subsets **any** declared axis, with `--targets` as sugar for
the primary one.

```text
--axis effort=high,low      # restrict the "effort" axis
--axis agent=yolop,codex    # restrict a harness axis crossed with the model
--targets sim,anthropic/opus  ==  --axis target=sim,anthropic/opus
```

Semantics:

- Values within one flag **OR**; different `--axis` flags **AND** (intersect).
- `name` is `target` (the primary axis) or any `Eval::axis(name, …)` name.
- An unknown axis name or value is a **hard error** (typo protection), listing
  the valid axes/values — consistent with how `--group-by` already names an
  axis.
- Host-side only: like `filter`/`--tag`/`--targets`, it *subsets* the grid the
  study declared (the host subsets, never adds cases — see
  [`docs/extensibility.md`](../docs/extensibility.md)). The study still owns the
  matrix.

Sketch against the current `RunArgs` (`crates/mira-cli/src/main.rs`):

```rust
/// Restrict the primary (target) axis to these labels (comma-separated).
/// Sugar for `--axis target=…`.
#[arg(long)]
targets: Option<String>,            // was: models
/// Restrict a matrix axis to a subset: `--axis effort=high,low` (repeatable).
/// `name` is `target` or any declared axis; values OR within a flag, flags AND.
#[arg(long = "axis", value_name = "NAME=V1,V2")]
axes: Vec<String>,
```

The two collapse into one selection pass: `--targets X` is folded into `axes` as
`target=X`, then the planner keeps a case iff, for every constrained axis, the
case's value is in the allowed set. `--group-by` and the case key are unaffected.

### 15.4 The `target` name clash — Sample's gold answer → `expected`

`Sample` already carried a `target` field (the gold/reference answer for
answer-comparison scorers), so promoting the comparison axis to `Target` made
"target" mean two things. Resolved by renaming the **sample** field: `Sample::target`
→ `Sample::expected` (`Sample::expected()`, `expected_str()`), and the scorer
`matches_target` → `matches_expected`. "Target" now unambiguously means the
comparison axis; "expected" is the gold answer. (`Sample` is not a wire type, so
this is a study-side rename with no protocol impact.)

### 15.5 Why both, why now

B removes the misnomer (the named concept matches what's being compared); A
removes the privilege asymmetry (selecting a harness/effort no longer requires
faking a model). Together they answer `yolop vs codex` directly — they're
**targets** you pick with `--targets yolop,codex`, or an **axis** you cross with
the model and pick with `--axis agent=yolop,codex` — with no masquerade either
way. Both are pre-1.0 internal renames/additions: no protocol change (selection
is host-side; `Target` serializes onto the same wire fields the schema already
publishes).
