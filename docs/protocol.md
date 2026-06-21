# The Mira Eval Protocol

The Mira Eval Protocol is how a **host** (the `mira` CLI) talks to an eval
**study** (your program). It is a small, MCP-style JSON-RPC dialect spoken over
a child process's stdio. Any program in any language that implements it is a
valid study — this is Mira's polyglot seam.

This page is the normative reference. For the Rust types, see the
[`mira::protocol`](https://docs.rs/mira-eval/latest/mira/protocol/) module. For a
machine-readable definition, see the generated **JSON Schema** under
[`schema/v1/`](../schema/v1/) (see [Machine-readable schema](#machine-readable-schema)).

## Overview

```text
┌────────────┐   spawn (cargo run / arbitrary cmd)   ┌────────────────┐
│            │ ────────────────────────────────────▶ │                │
│   host     │   stdin:  Request   (JSON, 1/line)     │     study      │
│ (mira CLI) │ ────────────────────────────────────▶ │ (your evals)   │
│            │   stdout: Response | Notification      │                │
│            │ ◀──────────────────────────────────────│                │
└────────────┘                                         └────────────────┘
```

- The **host** owns selection, the model matrix, aggregation, checkpoints, and
  rendering. It plans the whole run from `list` before executing anything.
- The **study** owns subjects and scoring. It answers requests and knows
  nothing about matrices, checkpoints, or reporting.
- **Provider API keys live only in the study's environment** and never cross
  the wire. The host addresses models by *label*; a cell whose model is
  unavailable is reported and skipped.

## Transport & framing

- Transport is the child process's **stdio**. `stdin` carries host→study
  messages; `stdout` carries study→host messages. `stderr` is free for the
  study's logs and build output and is never parsed.
- Framing is **newline-delimited JSON**: exactly one JSON object per line, UTF-8,
  no embedded newlines. Blank lines are ignored.
- `stdout` must carry **only** protocol JSON. Anything else (logging, `println!`)
  belongs on `stderr`.

## Message types

A line is classified by its fields:

| Has `id` | Has `method` | Type |
|---|---|---|
| ✓ | ✓ | **Request** (host → study) |
| ✓ | — | **Response** (study → host) |
| — | ✓ | **Notification** (study → host) |

### Request (host → study)

```json
{ "id": 1, "method": "run", "params": { "eval": "greet", "sample": "hi", "model": "sim" } }
```

| Field | Type | Notes |
|-------|------|-------|
| `id` | integer | Monotonic, unique per request. Correlates the response. |
| `method` | string | One of `initialize`, `list`, `run`, `execute`, `score`. |
| `params` | object | Method-specific; may be absent for parameterless methods. |

### Response (study → host)

```json
{ "id": 1, "result": { "...": "..." } }
{ "id": 1, "error": { "message": "no such eval: greet" } }
```

Exactly one of `result` or `error` is present. `id` echoes the request.

### Notification (study → host)

Fire-and-forget; no `id`, never acknowledged. Used for live progress.

```json
{ "method": "event", "params": { "eval": "greet", "sample": "hi", "model": "sim", "kind": "started" } }
{ "method": "log",   "params": { "message": "warming up driver" } }
```

The host may render or ignore notifications. A conforming study is not required
to emit any.

## Methods

### `initialize`

Always the first request. Negotiates the protocol version and announces the
study.

**Params**

```json
{ "protocol_version": "1.3", "host": "mira-cli" }
```

**Result**

```json
{
  "protocol_version": "1.3",
  "study": "my-evals",
  "evals": 3,
  "study_version": "0.1.0",
  "capabilities": ["axes", "events", "usage", "execute", "score"]
}
```

The study replies with the `protocol_version` it implements. Compatibility is
by **major**: a host refuses a study whose major differs from its own; a
differing minor is additive and tolerated (see [Versioning](#versioning)). The
current version is **`1.3`**.

`capabilities` lets a host feature-detect additively instead of sniffing
versions. Defined tokens: `axes` (study advertises extra axes and honours
`run.params`), `events` (emits `event` notifications), `usage` (reports
token/cost/timing), `execute` (answers `execute`), `score` (answers `score`).
`study_version` and `capabilities` are optional and default to empty. A `1.0`
study that only implements `run` interoperates unchanged — the host simply
won't see the `execute`/`score` capabilities.

### `list`

Enumerates every eval the study defines, with enough detail for the host to
plan the full `samples × models` grid and apply selection — without running
anything.

**Result**

```json
{
  "evals": [
    {
      "name": "greet",
      "description": "Greets the user and reports the answer",
      "samples": [ { "id": "hi", "tags": ["smoke"] } ],
      "scorers": ["succeeded", "contains(\"42\")"],
      "models": [
        { "label": "sim", "provider": "sim", "available": true },
        { "label": "anthropic/claude-opus-4-8", "provider": "anthropic", "available": false }
      ],
      "axes": [ { "name": "effort", "values": ["low", "high"] } ],
      "max_turns": 12,
      "metadata": { "suite": "smoke" }
    }
  ]
}
```

- `available: false` marks a cell the study cannot run (e.g. a missing API
  key). The host skips it rather than failing.
- `provider` (optional, default empty) is the model's provider id (`sim`,
  `anthropic`, …). The host uses it to bucket concurrency per provider and to
  back off a provider that returns rate-limit errors. A study that omits it
  groups all such cells under the empty provider.
- `axes` (optional, default empty) advertises **extra matrix axes** beyond the
  model. The host takes the cross-product of every axis with the model matrix and
  sends the chosen value per cell in `run.params`. A cell's identity is
  `eval/sample@model` with a sorted `[k=v,…]` suffix when axes vary.
- `metadata` is free-form `string → string` (provenance, observability links).

### `run`

Runs exactly one matrix cell, addressed by `(eval, sample, model label)`, and
returns the scored result. The study may emit `event` notifications before the
response.

**Params**

```json
{ "eval": "greet", "sample": "hi", "model": "sim", "params": { "effort": "high" } }
```

`params` (optional, default empty) carries the chosen value per extra axis, as
advertised in `list.axes`.

**Result**

```json
{
  "eval": "greet",
  "sample": "hi",
  "model": "sim",
  "params": { "effort": "high" },
  "passed": true,
  "aggregate": 1.0,
  "scores": [
    { "scorer": "contains", "value": 1.0, "pass": true, "reason": "found \"42\"" }
  ],
  "transcript": {
    "final_response": "Hi! The answer is 42.",
    "iterations": 1,
    "tool_calls_count": 0,
    "tool_calls": [],
    "usage": { "input_tokens": 12, "output_tokens": 8, "cost_usd": 0.0001 },
    "timing": { "duration_ms": 420, "time_to_first_token_ms": 180 },
    "metadata": {},
    "error": null
  },
  "skipped": false
}
```

| Field | Type | Notes |
|-------|------|-------|
| `params` | object | Echoes the cell's axis values (optional, default empty). |
| `passed` | bool | True iff every scorer passed (and at least one ran). |
| `aggregate` | number | Mean of score `value`s, `0.0..=1.0`. |
| `scores` | array | One [`Score`](#score) per scorer. |
| `transcript` | object | Lightweight summary (raw events omitted on the wire). |
| `skipped` | bool | True when the cell was not executed (e.g. unavailable model). |

The `transcript.usage` object may also carry `cache_read_tokens` and
`reasoning_tokens` (default 0), and `transcript.timing` carries `duration_ms`
and `time_to_first_token_ms` (omitted when unmeasured). The optional
`transcript.metrics` object is an open `string → number` map for custom metrics a
study reports (e.g. `{"retrieval_recall@5": 0.83}`); hosts that don't recognise a
key simply carry it through. All are optional and defaulted — older studies that
omit them still validate.

### `execute`

Runs one cell's subject **without scoring** and returns the **full** transcript
(raw `events` and captured `files` included, unlike `run`, which returns a
lightweight summary). This is the run-now-score-later half of `run`: a
long-running subject is executed once and its transcript persisted as an
execution artifact, to be scored — or re-scored — later. Advertised by the
`execute` capability.

**Params** — identical to `run` (`{ eval, sample, model, params }`).

**Result**

```json
{
  "eval": "greet",
  "sample": "hi",
  "model": "sim",
  "params": {},
  "transcript": {
    "final_response": "Hi! The answer is 42.",
    "iterations": 1,
    "tool_calls_count": 0,
    "usage": { "input_tokens": 12, "output_tokens": 8 },
    "events": [ "...full raw transcript..." ],
    "files": {}
  },
  "skipped": false
}
```

| Field | Type | Notes |
|-------|------|-------|
| `transcript` | object | The **full** transcript, including raw `events` and `files`. |
| `skipped` | bool | True when the cell was not executed (e.g. unavailable model). |

### `score`

Runs an eval's scorers over a **supplied** transcript and returns the same
`RunResult` as `run` — but without re-executing the subject. The transcript
travels in the request, so the host can replay a stored `execute` artifact.
Scoring depends only on the eval + sample, so the `model` label need not still
exist. Re-issuing `score` over the same transcript is a re-score (e.g. after a
scorer change). Advertised by the `score` capability.

**Params**

```json
{
  "eval": "greet",
  "sample": "hi",
  "model": "sim",
  "params": {},
  "transcript": { "final_response": "Hi! The answer is 42.", "...": "..." }
}
```

**Result** — a [`RunResult`](#run), identical in shape to the `run` response
(scores + lightweight transcript summary).

#### Score

```json
{ "scorer": "contains", "value": 1.0, "pass": true, "reason": "found \"42\"" }
```

`value` is a continuous score in `0.0..=1.0`; `pass` is the boolean verdict (for
graded scorers, typically `value >= threshold`). A score may also carry
`"na": true` — the scorer **could not be evaluated** (an unreachable judge, an
infra hiccup). N/A scores are excluded from the cell verdict and aggregate:
neither pass nor fail.

#### Infrastructure errors

A subject that fails for an **infrastructure** reason (budget/quota, rate limit,
provider 5xx/outage, network/timeout — not the model's fault) sets
`transcript.error` and `transcript.error_kind: "infra"` (the default,
`"subject"`, is omitted). The study then scores the cell with a single N/A score,
so it is excluded from the pass-rate — neither passed nor failed, like a skip.
The host **retries** infra-errored cells (keyed off `error_kind`) up to
`--max-retries`, and a cell whose every score is N/A is reported as N/A, not a
failure. `error_kind` is optional and defaulted (added in `1.3`), so a study that
omits it still interoperates.

## Run lifecycle

```text
host                                   study
 │ initialize ─────────────────────────▶│
 │◀──────────────── { protocol, evals } │
 │ list ───────────────────────────────▶│
 │◀───────────── { evals[…samples,…] }  │
 │                                       │
 │  (host plans grid: selection×matrix,  │
 │   subtracts checkpointed cells)       │
 │                                       │
 │ run {greet,hi,sim} ─────────────────▶ │   (many in flight at once)
 │ run {…cell 2…} ─────────────────────▶ │
 │◀──── event {kind:"started"}           │   (0+ notifications, any order)
 │◀──── { id:2, passed, … }              │   responses correlate by id
 │◀──── { id:1, passed, … }              │
 │            ⋮                          │
 │ (close stdin) ──────────────────────▶ │   EOF ⇒ study exits
```

The host issues one `run` per planned cell. Requests are **multiplexed**: the
host may keep many runs in flight over the single pipe, and the study dispatches
them concurrently — responses are correlated to requests by `id`, so they may
arrive in any order. The host bounds how many run at once with a global cap, a
per-provider cap, and **adaptive** per-provider backoff: a cell whose response
(or transcript) carries a rate-limit signal (HTTP 429, "overloaded", quota) is
re-queued after an exponential backoff while that provider's concurrency is
halved, recovering as cells succeed. Models are bucketed by their `list`
`provider`. Because the host owns the plan, **resume** falls out for free:
completed cells are persisted to a checkpoint and subtracted on the next
invocation.

## Errors

- A malformed request line that cannot be parsed (no recoverable `id`) should be
  reported via a `log` notification and skipped; the loop continues.
- A request that fails (unknown method, bad params, unknown eval/sample/model)
  returns an `error` response correlated by `id`. It does not terminate the
  connection.
- Closing the host's `stdin` (EOF) signals the study to exit cleanly.

## Versioning

The protocol uses `MAJOR.MINOR` (`PROTOCOL_VERSION`, currently `1.3`), all minors
additive over `1.0`: `1.1` added the optional `ModelInfo.provider` field and the
`execute`/`score` methods plus their capabilities; `1.2` added the optional
`transcript.metrics` map; `1.3` added the optional `transcript.error_kind`
(subject vs. infrastructure). A `1.0` study (or any study implementing only
`run`) interoperates with a `1.3` host.

- A **MINOR** bump is **additive**: new optional fields, new notification kinds,
  new capability tokens. A newer peer must keep talking to an older one.
- A **MAJOR** bump may change or remove fields. Peers with different majors are
  **incompatible**; the host rejects a mismatched major at `initialize`.

Forward compatibility is a hard requirement on both sides:

1. **Ignore unknown fields.** Every payload is parsed leniently (no strict
   "deny unknown fields"). A future study adding `transcript.energy_joules` must
   not break an older host.
2. **Default missing fields.** New fields are added as optional with sensible
   defaults (empty map/list, `0`, `null`), so an older study that omits them
   still validates against a newer host.
3. **Feature-detect via `capabilities`**, not version sniffing, for optional
   behaviour (`axes`, `events`, `usage`, `execute`, `score`).

This is why a `0.x`-era study (no `axes`, no `timing`) and a `1.0` host
interoperate: the host sees an empty `axes`/`capabilities` and a model-only
matrix, and the missing transcript fields default to zero.

## Machine-readable schema

The wire types have a generated, language-neutral definition under `schema/`:

- `schema/v1/schema.json` — a **JSON Schema 2020-12** document. The root is an
  `anyOf` over the three envelopes (`Request`, `Response`, `Notification`); every
  payload type (`InitializeResult`, `ListResult`/`EvalInfo`, `RunParams`,
  `RunResult`/`TranscriptSummary`, `ExecuteResult`/`ScoreParams` and the full
  `Transcript`, `Score`, …) is published under `$defs`.
- `schema/v1/meta.json` — a small index: the current `version`, `min_version`,
  the method list, and the defined `capabilities` tokens.

The directory is versioned by the protocol **major** (`v1`). The artifacts are
**generated from the Rust types** in `mira::protocol` by the `mira-schema-gen`
tool — they are not hand-edited and stay in lockstep with the wire format.
Regenerate with `just schema` (or `cargo run -p mira-schema-gen`); CI runs the
same generator with `--check` and fails if the committed files are stale, so a
protocol change can't merge without a matching schema update. A separate test
suite validates real serialized messages against the committed schema.

A non-Rust study can validate its messages against `schema.json` with any
standard JSON Schema validator instead of mirroring the Rust structs by hand.

### Staging unstable additions

New wire structure is developed behind the `mira-eval` crate's
`protocol-unstable` feature first — gated with `#[cfg(feature =
"protocol-unstable")]`. The schema generator builds **without** that feature, so
the committed `schema/` describes only the stable protocol; an addition reaches
the artifact (and a minor-version bump) only when promoted out of staging. This
covers *structural* changes — a new typed field or method — that the open
`metrics` / `metadata` / `capabilities` vocabularies can't express; those
already extend without a protocol bump. It lets such a change land and be
exercised in-tree without prematurely freezing the language-neutral contract.

## Implementing a study in another language

A minimal study is a stdio loop that:

1. reads a line, parses JSON;
2. on `initialize`, replies with `{ protocol_version, study, evals }`;
3. on `list`, replies with the eval catalogue;
4. on `run`, executes the subject, scores the transcript, and replies with a
   `RunResult`;
5. exits on EOF.

No Mira dependency is required — only the JSON shapes above. This is how
non-Rust agents (a Python SWE-bench harness, a Node agent) plug in as
first-class studies.

To support deferred / re-scoring, a study may additionally implement `execute`
(return the full transcript, no scoring) and `score` (score a supplied
transcript), advertising the matching capabilities. These are optional: a study
that implements only `run` is fully conforming.
