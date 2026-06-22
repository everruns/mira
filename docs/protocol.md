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

- The **host** owns selection, the target matrix, aggregation, checkpoints, and
  rendering. It plans the whole run from `list` before executing anything.
- The **study** owns subjects and scoring. It answers requests and knows
  nothing about matrices, checkpoints, or reporting.
- **Provider API keys live only in the study's environment** and never cross
  the wire. The host addresses targets by *label*; a cell whose target is
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

A line is classified by its **fields**, not by which pipe it arrived on:

| Has `method` | Has `id` | Type |
|---|---|---|
| ✓ | ✓ | **Request** |
| ✓ | — | **Notification** |
| — | ✓ | **Response** (correlated by `id`) |

`method` is the discriminator: a line that has it is a request or notification,
never a response. Today requests flow host→study and responses/notifications flow
study→host, so direction and shape line up. But the rule is field-based on
purpose — it leaves room for a **reverse request** (study→host) without changing
the framing (see [Reverse requests](#reverse-requests-studyhost)).

### Request (host → study)

```json
{ "id": 1, "method": "run", "params": { "eval": "greet", "sample": "hi", "target": "sim" } }
```

| Field | Type | Notes |
|-------|------|-------|
| `id` | integer | Monotonic, unique per request. Correlates the response. |
| `method` | string | One of `initialize`, `list`, `list_samples`, `run`, `execute`, `score`, `cancel`. |
| `params` | object | Method-specific; may be absent for parameterless methods. |

### Response (study → host)

```json
{ "id": 1, "result": { "...": "..." } }
{ "id": 1, "error": { "code": -32602, "message": "no such eval: greet", "retryable": false } }
```

Exactly one of `result` or `error` is present. `id` echoes the request.

The `error` object is JSON-RPC-shaped so a protocol-level failure can be
classified without parsing the human message:

| Field | Type | Notes |
|-------|------|-------|
| `code` | integer | Failure class (JSON-RPC convention): `-32602` invalid params, `-32601` method not found, `-32603` internal. `0` = unclassified. Optional, defaults `0`. |
| `message` | string | Human-readable description. The only required field. |
| `retryable` | boolean | Hint that retrying the identical request may succeed (a transient infra fault, not the caller's mistake). The host re-attempts retryable cells up to `--max-retries`. Optional, defaults `false`. |
| `data` | any | Optional structured payload for programmatic handling. Omitted when absent. |

All fields beyond `message` are optional and defaulted, so a peer that sends
bare `{ "message": "…" }` still parses.

### Notification (study → host)

Fire-and-forget; no `id`, never acknowledged. Used for live progress.

```json
{ "method": "event", "params": { "request_id": 7, "eval": "greet", "sample": "hi", "target": "sim", "kind": "started" } }
{ "method": "log",   "params": { "request_id": 7, "message": "warming up driver" } }
```

Both payloads are typed and published in the schema (`EventParams`, `LogParams`).
A notification can't carry the envelope `id` — that field classifies a line as a
[Response](#response-study--host) — so an `event` correlates to the `run`/`execute`
request that triggered it via a **`request_id`** in the payload, the same
demultiplexing key responses use. This lets the host bind progress to a specific
in-flight call even when many cells (including repeated trials of one cell) are
multiplexed over the single pipe. `request_id` defaults to `0` ("uncorrelated"),
so a study that omits it still validates.

An `event`'s **`kind`** is drawn from an open, growing vocabulary (like
`capabilities`): `started` (run begun, emitted first), `turn` (a reasoning
iteration started — `turn` carries its index), `tool_call` (`tool` carries the
name), `output` (`text` carries a streamed delta), `finished` (run done, emitted
last). An older host carries an unrecognised future kind through verbatim rather
than failing. The current set is indexed in `schema/v1/meta.json` as
`event_kinds`.

The host may render or ignore notifications. A conforming study is not required
to emit any.

## Methods

### `initialize`

Always the first request. Negotiates the protocol version and announces the
study.

**Params**

```json
{ "protocol_version": "1.0", "host": "mira-cli" }
```

**Result**

```json
{
  "protocol_version": "1.0",
  "study": "my-evals",
  "evals": 3,
  "study_version": "0.1.0",
  "capabilities": ["axes", "events", "usage", "execute", "score", "trials", "cancel", "paginate"],
  "capability_params": {
    "events": { "kinds": ["started"] },
    "modalities": { "input": ["text", "image", "audio", "file", "json"],
                    "output": ["text", "image", "audio", "file", "json"] }
  }
}
```

The study replies with the `protocol_version` it implements. Compatibility is
by **major**: a host refuses a study whose major differs from its own; a
differing minor is additive and tolerated (see [Versioning](#versioning)). The
current version is **`1.0`**.

`capabilities` lets a host feature-detect additively instead of sniffing
versions. Defined tokens: `axes` (study advertises extra axes and honours
`run.params`), `events` (emits `event` notifications), `usage` (reports
token/cost/timing), `execute` (answers `execute`), `score` (answers `score`),
`trials` (threads the `seed` run param into the subject, so repetitions are
reproducible), `cancel` (answers `cancel`), `paginate` (answers `list_samples`
and may return `EvalInfo.next_cursor` from `list`). `study_version` and
`capabilities` are optional and default to empty. A study that implements only
the base methods (`initialize`, `list`, `run`) and advertises no capabilities
interoperates unchanged — the host simply won't see the
`execute`/`score`/`trials`/`cancel`/`paginate` capabilities.

`capability_params` (optional, defaulted) carries **structured config** for the
advertised capabilities, keyed by capability token — the data a bare token
can't: which `event` kinds the study emits, the input/output `modalities` it
understands, and so on. It is open-vocabulary like `metadata`, so a host reads
it additively and falls back to default behaviour when a token is absent.

### `list`

Enumerates every eval the study defines, with enough detail for the host to
plan the full `samples × targets` grid and apply selection — without running
anything. For large or lazily generated datasets, samples are **paginated**: an
eval carries the first page inline and a `next_cursor` the host follows with
[`list_samples`](#list_samples).

**Result**

```json
{
  "evals": [
    {
      "name": "greet",
      "description": "Greets the user and reports the answer",
      "samples": [
        { "id": "hi", "tags": ["smoke"], "metadata": { "repo": "example/repo", "difficulty": "easy" } }
      ],
      "next_cursor": null,
      "scorers": ["succeeded", "contains(\"42\")"],
      "targets": [
        { "label": "sim", "provider": "sim", "available": true },
        { "label": "anthropic/claude-opus-4-8", "provider": "anthropic", "available": false,
          "metadata": { "agent": "swe-agent", "effort": "high", "price_tier": "premium" } }
      ],
      "axes": [ { "name": "effort", "values": ["low", "high"] } ],
      "max_turns": 12,
      "trials": 8,
      "seed": 42,
      "metadata": { "suite": "smoke" }
    }
  ]
}
```

- `samples` is the **first page** of the eval's samples. `next_cursor`
  (optional, default absent/`null`) is an **opaque** continuation token: present
  iff more samples remain. The host pages the rest with `list_samples`, passing
  the token back verbatim, until it comes back absent. A study that fits its
  whole dataset inline omits `next_cursor` — identical to a non-paginated `list`,
  so an older host that ignores the field still works for non-paginated studies.
- `available: false` marks a cell the study cannot run (e.g. a missing API
  key). The host skips it rather than failing.
- `provider` (optional, default empty) is the model's provider id (`sim`,
  `anthropic`, …). The host uses it to bucket concurrency per provider and to
  back off a provider that returns rate-limit errors. A study that omits it
  groups all such cells under the empty provider.
- `axes` (optional, default empty) advertises **extra matrix axes** beyond the
  model. The host takes the cross-product of every axis with the target matrix and
  sends the chosen value per cell in `run.params`. A cell's identity is
  `eval/sample@target` with a sorted `[k=v,…]` suffix when axes vary.
- `metadata` is free-form, open-ended `string → JSON` (provenance, observability
  links, structured context). Values may be a string, number, bool, or a nested
  object/array. (Axis `params`, by
  contrast, stay `string → string`: they form part of a cell's identity.)
  Carried at three levels, each optional and defaulting to empty: on the **eval**
  (shown above), on each **sample** (`samples[].metadata` — repo, difficulty,
  dataset split, …), and on each **model** (`targets[].metadata` — agent,
  underlying model, effort, price, sandbox, …). The per-sample and per-target maps
  are optional; an older study that omits them still parses. The host
  surfaces them in `list` and can break resolve-rate down by any of their keys
  with `mira run --group-by <key>`.
- `trials` (optional, default 1) is how many times each cell should be **repeated**
  for pass@k / pass-rate / variance over a stochastic subject. Unlike an axis,
  trials don't form new cells — they're re-runs of one cell, grouped back by the
  host. `seed` (optional) is the study's base seed: trial `t` runs with `seed + t`,
  so the repetition set replays deterministically. The host may override both with
  `--trials` / `--seed`. See [`run`](#run) for how a trial is addressed.

### `list_samples`

Fetches the **next page** of one eval's samples, continuing from a cursor handed
back by `list` (`EvalInfo.next_cursor`) or a prior `list_samples`. Lets a study
advertise a dataset too large — or too lazily generated — to enumerate in a
single `list` line (e.g. SWE-bench full). Advertised by the `paginate`
capability.

**Params**

```json
{ "eval": "greet", "cursor": "500" }
```

| Field | Type | Notes |
|-------|------|-------|
| `eval` | string | The eval whose samples to continue. |
| `cursor` | string | Opaque token from the previous page; echoed back verbatim. |

**Result**

```json
{
  "samples": [ { "id": "case-500", "tags": [] } ],
  "next_cursor": "1000"
}
```

| Field | Type | Notes |
|-------|------|-------|
| `samples` | array | The next page of `SampleInfo` (`id` + optional `tags`). |
| `next_cursor` | string | Token for the page after this one; **absent on the last page**. |

The host loops `list_samples` until `next_cursor` is absent, concatenating the
pages onto the eval's `samples` to reconstruct the full grid before planning. The
cursor is opaque: only the study interprets it (the bundled study encodes a
sample offset, but a study may use any token — a DB keyset, an API page token).

### `run`

Runs exactly one matrix cell, addressed by `(eval, sample, target label)`, and
returns the scored result. The study may emit `event` notifications before the
response.

**Params**

```json
{ "eval": "greet", "sample": "hi", "target": "sim", "params": { "effort": "high" },
  "trial": 2, "trials": 8, "seed": 44 }
```

`params` (optional, default empty) carries the chosen value per extra axis, as
advertised in `list.axes`.

`trial`/`trials`/`seed` (all optional, default `0`/`1`/none) address one
**repetition** of the cell: `trial` is the 0-based index, `trials` the planned
count, and `seed` the per-trial seed the study threads into the subject. A
repeated cell's identity gains a `#trial` suffix (`greet/hi@sim[effort=high]#2`);
a single-trial cell keeps its plain key. The study **echoes** these back in the
result so its key matches the host's plan. The host groups results by the
*logical* key (without the `#trial` suffix) to aggregate pass@k / pass-rate /
variance.

**Result**

```json
{
  "eval": "greet",
  "sample": "hi",
  "target": "sim",
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
| `trial` / `trials` / `seed` | int / int / int | Echo the cell's trial identity (optional; omitted for a single, unseeded run). |
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

For a **multimodal** subject the transcript may also carry an `output` array —
the response as ordered, typed [`Part`](#multimodal-content)s — alongside
`final_response`. `final_response` stays the canonical *text* projection (so a
text-only scorer keeps working); `output` carries the non-text modalities. It is
optional and omitted for the common text-only case.

#### Multimodal content

A `Part` is one piece of content, a self-describing JSON object tagged by `kind`:

```json
{ "kind": "text",  "text": "a cat on a mat" }
{ "kind": "image", "media_type": "image/png", "source": { "uri": "https://x/cat.png" } }
{ "kind": "image", "media_type": "image/png", "source": { "data": "<base64>" } }
{ "kind": "file",  "name": "report.pdf", "media_type": "application/pdf", "source": { "uri": "…" } }
{ "kind": "json",  "json": { "label": "cat", "p": 0.91 } }
```

Media is **referenced, not embedded**: a media part carries a `media_type` plus a
`source` that is **exactly one** of `{ "uri": … }` (a URL or `data:` URI) or
`{ "data": … }` (inline base64) — never raw bytes, so a part is plain JSON. A
media part with neither source is invalid. Inputs (a study's dataset) use the
same vocabulary off-wire; `output` puts it on the wire.

### `execute`

Runs one cell's subject **without scoring** and returns the **full** transcript
(raw `events` and captured `files` included, unlike `run`, which returns a
lightweight summary). This is the run-now-score-later half of `run`: a
long-running subject is executed once and its transcript persisted as an
execution artifact, to be scored — or re-scored — later. Advertised by the
`execute` capability.

**Params** — identical to `run` (`{ eval, sample, target, params, trial, trials, seed }`).

**Result**

```json
{
  "eval": "greet",
  "sample": "hi",
  "target": "sim",
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
| `trial` / `trials` / `seed` | int / int / int | Echo the cell's trial identity (optional), so a per-trial artifact stays distinct. |
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
  "target": "sim",
  "params": {},
  "trial": 2,
  "trials": 8,
  "seed": 44,
  "transcript": { "final_response": "Hi! The answer is 42.", "...": "..." }
}
```

The `trial`/`trials`/`seed` fields (optional) are echoed into the resulting
`RunResult` so a re-scored trial keeps its identity.

**Result** — a [`RunResult`](#run), identical in shape to the `run` response
(scores + lightweight transcript summary).

### `cancel`

Aborts one in-flight `run`/`execute`/`score` by its request `id`, so a host can
enforce per-cell timeouts, hard cost caps, or fail-fast without tearing down the
whole connection. Advertised by the `cancel` capability.

**Params**

```json
{ "id": 7 }
```

`id` is the **request id** of the call to abort — the `id` the host put on the
`run` and is awaiting a response on — not a cell key. So a host can target one
specific outstanding call even when several runs of the same cell are in flight.

**Result**

```json
{ "cancelled": true }
```

`cancelled` is whether a matching in-flight request was found and aborted.
`false` is normal and benign: the targeted request had already completed (or was
never in flight) by the time the cancel arrived. Cancellation is **best-effort** —
a `run` that finishes first still returns its real result.

A cancelled run's own response arrives as an `error` correlated by its `id`
(message `cancelled`), so the host's pending call resolves promptly instead of
hanging until EOF. The study should stop work at the next opportunity; the cancel
response itself is immediate and independent.

A host that drives this over the protocol need not even hold the id explicitly:
the Rust [`mira`](https://docs.rs/mira-eval) host sends a best-effort `cancel`
automatically when a `run` future is dropped (e.g. by `tokio::time::timeout` or a
fail-fast `select!`), and also exposes an explicit `HostHandle::cancel(id)`.

### Reverse requests (study→host)

> **Status: reserved seam, not yet implemented.** No reverse method is defined
> and no host answers one today. This section is the *design of record* so the
> channel can be added later as a **minor** bump, not a breaking 2.0 — the one
> direction the protocol doesn't yet carry, and the one most likely to force a
> major version if retrofitted carelessly.

Today a study is fully self-contained: subjects and provider keys live study-side
by design, and every request flows host→study. Some capabilities want the
opposite direction — the study asking the host for something mid-run:

- **host-brokered model access** — central credentials, caching, and budgeting
  in the host instead of per-study keys;
- **shared resources** — a sandbox, fixture, or dataset the host owns;
- **human-in-the-loop** — pause a cell to ask the operator a question.

Each needs a study→host **request** (with a host **response**), a direction that
doesn't exist yet. The framing already admits it without a breaking change,
provided these invariants hold — they are the contract a future implementation
must keep:

1. **Field-based classification.** A line is a request/notification iff it bears
   `method` (see [Message types](#message-types)); only a `method`-less line is a
   response. So a reverse request (`{ "id": …, "method": …, "params": … }`) on
   the study's stdout is unambiguous and is **never** mistaken for a response —
   even by a host that predates the feature.
2. **Independent `id` spaces per direction.** Host-originated and study-originated
   request ids are separate sequences; each correlates only with responses
   flowing back the same way. They may overlap (both start at 1) without
   collision, because a response is matched to a pending request *on the same
   side*. A host that ignores the channel must therefore not route an inbound
   `method`-bearing line through its response table.
3. **Capability-negotiated, both ways.** The channel is off unless **both** peers
   opt in: the host advertises support in `initialize.params` and the study
   advertises the reserved `host_requests` capability (and only then emits reverse
   requests). A study must assume the channel is absent until it sees host
   support — exactly the additive, feature-detected pattern the rest of the
   protocol uses.

A conforming host that doesn't implement the channel simply never advertises it,
and safely ignores any reverse request it receives (it logs and drops it rather
than letting the id corrupt its own request routing). That graceful-ignore
behaviour is implemented today, so the seam is real, not theoretical.

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
failure. `error_kind` is optional and defaulted, so a study that omits it still
interoperates.

## Run lifecycle

```text
host                                   study
 │ initialize ─────────────────────────▶│
 │◀──────────────── { protocol, evals } │
 │ list ───────────────────────────────▶│
 │◀───────────── { evals[…page 1,cursor]}│
 │ list_samples {greet, cursor} ───────▶ │   (only while a cursor remains)
 │◀───────────── { samples, next_cursor }│
 │                                       │
 │  (host plans grid: selection×matrix,  │
 │   subtracts checkpointed cells)       │
 │                                       │
 │ run {greet,hi,sim} ─────────────────▶ │   (many in flight at once)
 │ run {…cell 2…} ─────────────────────▶ │
 │◀──── event {kind:"started"}           │   (0+ notifications, any order)
 │◀──── { id:2, passed, … }              │   responses correlate by id
 │ cancel { id:1 } ────────────────────▶ │   abort one run by request id
 │◀──── { id:3, cancelled:true }         │   (cancel ack)
 │◀──── { id:1, error:"cancelled" }      │   the aborted run resolves
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

A host can abort a single in-flight run with [`cancel`](#cancel) — addressing it
by request `id` — without disturbing the others or closing the connection. This
is the lever for per-cell timeouts, hard cost caps, and fail-fast. (Closing
stdin, by contrast, ends *every* in-flight run at once.)

## Errors

- A malformed request line that cannot be parsed (no recoverable `id`) should be
  reported via a `log` notification and skipped; the loop continues.
- A request that fails (unknown method, bad params, unknown eval/sample/model)
  returns an `error` response correlated by `id`. It does not terminate the
  connection.
- Closing the host's `stdin` (EOF) signals the study to exit cleanly.

## Versioning

The protocol uses `MAJOR.MINOR` (`PROTOCOL_VERSION`, currently `1.0`). `1.0` is
the initial stable baseline: the full method set (`initialize`, `list`,
`list_samples`, `run`, `execute`, `score`, `cancel`), typed and
`request_id`-correlated `event`/`log` notifications, JSON-RPC-shaped error
objects, trials/repetitions with seeds, cursor-paginated sample listing,
eval/sample/model `metadata` (open-ended JSON), and multimodal `output` plus
structured `capability_params`. A study that implements only the base methods
(`initialize`, `list`, `run`) — advertising no capabilities and emitting no
notifications — interoperates with a full `1.0` host: every method and field
beyond the base is feature-detected or defaulted. Future additions bump the minor.

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
   behaviour (`axes`, `events`, `usage`, `execute`, `score`, `trials`, `cancel`,
   `paginate`).

This is why a `0.x`-era study (no `axes`, no `timing`) and a `1.0` host
interoperate: the host sees an empty `axes`/`capabilities` and a target-only
matrix, and the missing transcript fields default to zero.

The [reverse request channel](#reverse-requests-studyhost) is the one *new
direction* the protocol reserves for a future minor: its framing and negotiation
are fixed now (field-based classification, per-direction `id` spaces, the
`host_requests` capability) so adding it stays additive. The concrete methods
would be staged behind `protocol-unstable` like any other structural addition.

## Machine-readable schema

The wire types have a generated, language-neutral definition under `schema/`:

- `schema/v1/schema.json` — a **JSON Schema 2020-12** document. The root is an
  `anyOf` over the three envelopes (`Request`, `Response`, `Notification`); every
  payload type (`InitializeResult`, `ListResult`/`EvalInfo`,
  `ListSamplesParams`/`ListSamplesResult`, `RunParams`,
  `RunResult`/`TranscriptSummary`, `ExecuteResult`/`ScoreParams`, the notification
  payloads `EventParams`/`LogParams`, and the full `Transcript`, `Score`, …) is
  published under `$defs`.
- `schema/v1/meta.json` — a small index: the current `version`, `min_version`,
  the method list, the defined `capabilities` tokens, and the `event_kinds`
  vocabulary.

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
Multimodal `output` and `capability_params` rode this path before being promoted
onto the `1.0` wire; `TranscriptSummary.experimental` is the reserved placeholder
for the next.

## Implementing a study in another language

A minimal study is a stdio loop that:

1. reads a line, parses JSON;
2. on `initialize`, replies with `{ protocol_version, study, evals }`;
3. on `list`, replies with the eval catalogue;
4. on `run`, executes the subject, scores the transcript, and replies with a
   `RunResult`;
5. exits on EOF.

No Rust dependency is required — only the JSON shapes above (validate against the
[machine-readable schema](#machine-readable-schema) instead of mirroring them by
hand). This is how non-Rust agents (a Python SWE-bench harness, a Node agent)
plug in as first-class studies.

For Python, the **[Mira Python SDK](../sdks/python)** does this for you: a native
library whose wire types are generated from `schema/v1/`, with an ergonomic
authoring API and a `serve()` loop. See [`specs/sdks.md`](../specs/sdks.md) for
the SDK design (native libraries, not FFI bindings).

To support deferred / re-scoring, a study may additionally implement `execute`
(return the full transcript, no scoring) and `score` (score a supplied
transcript), advertising the matching capabilities. To let a host abort a single
in-flight run, a study may implement `cancel` (advertising the `cancel`
capability): track runs by request `id`, and on `cancel` stop the named run and
reply to it with an `error` of `cancelled`. All of these are optional: a study
that implements only `run` is fully conforming.
