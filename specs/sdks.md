# Mira — non-Rust study SDKs

- Status: **implemented** (Python; TypeScript planned)
- Authors: Everruns

Design of record for native study SDKs in other languages. Code complies with
this or proposes a change here. Related: [`architecture.md`](architecture.md)
(execution model, the protocol seam) and [`docs/protocol.md`](../docs/protocol.md)
(the normative wire reference).

## 1. Problem

Mira's architecture splits the **study** (defines evals, owns subjects and
scoring, holds provider keys) from the **host** (the `mira` CLI: selection,
matrix, concurrency, checkpoints, reporting), talking newline-delimited JSON over
stdio. "Implementing a study in another language" already works at the raw
protocol level — but hand-rolling the stdio loop and mirroring the wire structs
by hand (as the original `greet-python` did) is tedious and drifts from the Rust
types.

**Goal:** an ergonomic, first-class authoring surface per target language
(Python, TypeScript) without compromising the clean process boundary.

## 2. Decision: native libraries, not bindings

Each SDK is a **standalone, native library** that speaks the protocol — *not* an
FFI binding to `mira-eval` (no PyO3/maturin, no napi/wasm).

Rationale:

- **The protocol is the seam, by design.** Bindings would route around the
  boundary the architecture deliberately invests in, and re-couple at the FFI
  layer what the wire decouples.
- **The study side is small.** Wire types + an authoring API (eval registry,
  subject/scorer helpers) + a stdio `serve()` loop is a few hundred lines of
  idiomatic code per language — cheaper to build and maintain than an FFI bridge.
- **The heavy parts stay Rust and are reached over the wire.** Selection, the
  matrix cross-product, adaptive per-provider throttling, checkpoints/resume, and
  HTML/JUnit/MD reporting live host-side; an SDK author gets all of it for free
  via the CLI, none of it crossing a binding.
- **Bindings fight the trait model and distribution.** The core abstractions are
  `async` traits authors *implement*; exposing them ergonomically over FFI is the
  painful part, and shipping per-platform native wheels/addons adds a build
  matrix and version lockstep with the crate. A pure-stdlib library installs
  with no compile step and runs anywhere the interpreter does.

`mira-everruns`-style provider integrations remain separate concerns; an SDK
study makes its own model calls (keys live in the study), so the host shares only
the scorer *vocabulary*, never an implementation.

## 3. Wire types are generated from the schema

The one real cost of native SDKs — each re-declaring the wire shapes — is removed
by generating them from the canonical **JSON Schema** under `schema/v<major>/`
(itself generated from `mira::protocol` by `mira-schema-gen`). Each SDK ships a
small codegen that reads `schema/v1/schema.json` and emits its typed wire layer,
plus a `--check` mode that fails on drift — the per-language dual of the Rust
schema `--check`. So there is a single source of truth (the Rust types →
schema), and three drift guards (Rust, and one per SDK) keep every language in
lockstep with the wire.

Forward/backward compatibility rides the protocol's existing contract (ignore
unknown fields, default missing fields, capability negotiation), so an SDK and an
older/newer host interoperate without lockstep on *versions* — only on the
*major*.

## 4. Layout

```
sdks/<lang>/        one native SDK per language
  schema codegen    reads ../../schema/v1/schema.json → generated wire types
  <package>         the library: wire types (generated) + authoring API + serve loop
  tests             schema-conformance (validate emitted messages) + behaviour
```

- The runtime library has **zero third-party dependencies** where the language
  allows (Python: stdlib only), so a study runs anywhere the interpreter does;
  validation/codegen/test tools are dev-only.
- **Example studies stay under `examples/`** (the single `just run-examples`
  entry point), importing the SDK; the SDK dir holds only the library + its own
  tests. `examples/greet-python` is the worked example and mirrors the Rust
  `greet`.
- CI gates each SDK: schema-in-sync (`codegen --check`) + the test suite, wired
  into the `Check` gate, and the polyglot example runs end-to-end through the
  host like every other example.

## 5. Authoring surface (parity target)

Each SDK exposes, in idiomatic form: a `Study` registry with an `eval`
decorator/builder; `Sample`, `model(...)`, and a `RunCx` (model/provider/
max_turns/axis params); a `Transcript` builder with `Usage`/`Timing`; built-in
scorers (`succeeded`, `contains`, `equals`, `regex`) plus a `scorer(name, fn)`
escape hatch returning a bool or a full `Score` (incl. `na`); `axis(name,
values)` for extra matrix axes; and `serve()` handling
`initialize`/`list`/`run`/`execute`/`score`. Scoring semantics match
`crate::runner` exactly: an N/A score is excluded from the cell verdict and
aggregate; an unavailable model / infra error short-circuits to a single N/A.

## 6. Status & deferred

- **Python** (`sdks/python`) — implemented: schema-driven codegen, full serve
  loop (incl. the `execute`/`score` split), conformance + behaviour tests.
- **TypeScript** (`sdks/typescript`) — planned, same shape: codegen from
  `schema/v1/` (`json-schema-to-typescript`), a `serve()` loop, parity authoring
  API, npm package with zero runtime deps.
- **Deferred:** emitting `event` progress notifications from SDK studies;
  publishing the SDKs to PyPI/npm (tied to [`release-process`](release-process.md)).
