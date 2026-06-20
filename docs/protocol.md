# The Mira Eval Protocol

The Mira Eval Protocol is how a **host** (the `mira` CLI) talks to an eval
**server** (your program). It is a small, MCP-style JSON-RPC dialect spoken over
a child process's stdio. Any program in any language that implements it is a
valid eval server — this is Mira's polyglot seam.

This page is the normative reference. For the Rust types, see the
[`mira::protocol`](https://docs.rs/mira-eval/latest/mira/protocol/) module.

## Overview

```text
┌────────────┐   spawn (cargo run / arbitrary cmd)   ┌────────────────┐
│            │ ────────────────────────────────────▶ │                │
│   host     │   stdin:  Request   (JSON, 1/line)     │     server     │
│ (mira CLI) │ ────────────────────────────────────▶ │ (your evals)   │
│            │   stdout: Response | Notification      │                │
│            │ ◀──────────────────────────────────────│                │
└────────────┘                                         └────────────────┘
```

- The **host** owns selection, the model matrix, aggregation, checkpoints, and
  rendering. It plans the whole run from `list` before executing anything.
- The **server** owns subjects and scoring. It answers requests and knows
  nothing about matrices, checkpoints, or reporting.
- **Provider API keys live only in the server's environment** and never cross
  the wire. The host addresses models by *label*; a cell whose model is
  unavailable is reported and skipped.

## Transport & framing

- Transport is the child process's **stdio**. `stdin` carries host→server
  messages; `stdout` carries server→host messages. `stderr` is free for the
  server's logs and build output and is never parsed.
- Framing is **newline-delimited JSON**: exactly one JSON object per line, UTF-8,
  no embedded newlines. Blank lines are ignored.
- `stdout` must carry **only** protocol JSON. Anything else (logging, `println!`)
  belongs on `stderr`.

## Message types

A line is classified by its fields:

| Has `id` | Has `method` | Type |
|---|---|---|
| ✓ | ✓ | **Request** (host → server) |
| ✓ | — | **Response** (server → host) |
| — | ✓ | **Notification** (server → host) |

### Request (host → server)

```json
{ "id": 1, "method": "run", "params": { "eval": "greet", "sample": "hi", "model": "sim" } }
```

| Field | Type | Notes |
|-------|------|-------|
| `id` | integer | Monotonic, unique per request. Correlates the response. |
| `method` | string | One of `initialize`, `list`, `run`. |
| `params` | object | Method-specific; may be absent for parameterless methods. |

### Response (server → host)

```json
{ "id": 1, "result": { "...": "..." } }
{ "id": 1, "error": { "message": "no such eval: greet" } }
```

Exactly one of `result` or `error` is present. `id` echoes the request.

### Notification (server → host)

Fire-and-forget; no `id`, never acknowledged. Used for live progress.

```json
{ "method": "event", "params": { "eval": "greet", "sample": "hi", "model": "sim", "kind": "started" } }
{ "method": "log",   "params": { "message": "warming up driver" } }
```

The host may render or ignore notifications. A conforming server is not required
to emit any.

## Methods

### `initialize`

Always the first request. Negotiates the protocol version and announces the
server.

**Params**

```json
{ "protocol_version": "0.1", "host": "mira-cli" }
```

**Result**

```json
{ "protocol_version": "0.1", "server": "my-evals", "evals": 3 }
```

The server should reply with the `protocol_version` it implements. Hosts and
servers sharing the same `MAJOR.MINOR` are compatible. The current version is
**`0.1`**.

### `list`

Enumerates every eval the server defines, with enough detail for the host to
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
        { "label": "sim", "available": true },
        { "label": "anthropic/claude-opus-4-8", "available": false }
      ],
      "max_turns": 12,
      "metadata": { "suite": "smoke" }
    }
  ]
}
```

- `available: false` marks a cell the server cannot run (e.g. a missing API
  key). The host skips it rather than failing.
- `metadata` is free-form `string → string` (provenance, observability links).

### `run`

Runs exactly one matrix cell, addressed by `(eval, sample, model label)`, and
returns the scored result. The server may emit `event` notifications before the
response.

**Params**

```json
{ "eval": "greet", "sample": "hi", "model": "sim" }
```

**Result**

```json
{
  "eval": "greet",
  "sample": "hi",
  "model": "sim",
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
    "metadata": {},
    "error": null
  },
  "skipped": false
}
```

| Field | Type | Notes |
|-------|------|-------|
| `passed` | bool | True iff every scorer passed (and at least one ran). |
| `aggregate` | number | Mean of score `value`s, `0.0..=1.0`. |
| `scores` | array | One [`Score`](#score) per scorer. |
| `transcript` | object | Lightweight summary (raw events omitted on the wire). |
| `skipped` | bool | True when the cell was not executed (e.g. unavailable model). |

#### Score

```json
{ "scorer": "contains", "value": 1.0, "pass": true, "reason": "found \"42\"" }
```

`value` is a continuous score in `0.0..=1.0`; `pass` is the boolean verdict (for
graded scorers, typically `value >= threshold`). Keeping both lets a scorer
report a graded signal while still contributing a pass/fail to the matrix.

## Run lifecycle

```text
host                                   server
 │ initialize ─────────────────────────▶│
 │◀──────────────── { protocol, evals } │
 │ list ───────────────────────────────▶│
 │◀───────────── { evals[…samples,…] }  │
 │                                       │
 │  (host plans grid: selection×matrix,  │
 │   subtracts checkpointed cells)       │
 │                                       │
 │ run {greet,hi,sim} ─────────────────▶ │
 │◀──── event {kind:"started"}           │   (0+ notifications)
 │◀──── { passed, scores, transcript }   │
 │ run {…next cell…} ──────────────────▶ │
 │            ⋮                          │
 │ (close stdin) ──────────────────────▶ │   EOF ⇒ server exits
```

The host issues one `run` per planned cell. Because the host owns the plan,
**resume** falls out for free: completed cells are persisted to a checkpoint and
subtracted on the next invocation.

## Errors

- A malformed request line that cannot be parsed (no recoverable `id`) should be
  reported via a `log` notification and skipped; the loop continues.
- A request that fails (unknown method, bad params, unknown eval/sample/model)
  returns an `error` response correlated by `id`. It does not terminate the
  connection.
- Closing the host's `stdin` (EOF) signals the server to exit cleanly.

## Versioning

The protocol uses `MAJOR.MINOR` (`PROTOCOL_VERSION`, currently `0.1`). A `MINOR`
bump is additive (new optional fields, new notification kinds); a `MAJOR` bump
may change or remove fields. Servers and hosts should accept unknown fields and
unknown notification methods to stay forward-compatible.

## Implementing a server in another language

A minimal server is a stdio loop that:

1. reads a line, parses JSON;
2. on `initialize`, replies with `{ protocol_version, server, evals }`;
3. on `list`, replies with the eval catalogue;
4. on `run`, executes the subject, scores the transcript, and replies with a
   `RunResult`;
5. exits on EOF.

No Mira dependency is required — only the JSON shapes above. This is how
non-Rust agents (a Python SWE-bench harness, a Node agent) plug in as
first-class eval servers.
