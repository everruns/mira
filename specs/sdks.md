# Mira — non-Rust study SDKs

- Status: **implemented** (Python and TypeScript)
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

## 3. The protocol layer is generated from the schema

The one real cost of native SDKs — each re-declaring the protocol — is removed by
generating it from the canonical artifacts under `schema/v<major>/` (themselves
generated from `mira::protocol` by `mira-schema-gen`). Each SDK ships a small
codegen with a `--check` drift mode — the per-language dual of the Rust schema
`--check` — that emits, and guards:

- **wire types** from `schema.json` — the typed payload layer; and
- **protocol metadata** from `meta.json` — the `PROTOCOL_VERSION`, the method
  list, and the capability tokens.

So the version string and method/capability vocabulary are *not* hardcoded in the
SDK (which silently drifts on a minor bump); they are generated, and the serve
loop derives `PROTOCOL_VERSION` from them.

**What the drift guards cover, and what they don't.** Three guards
(`mira-schema-gen --check`, plus one `codegen --check` per SDK, in the `Check`
gate) keep the *generated* layer in lockstep with the wire. On top of that, each
SDK adds tests that bind its *hand-written* layer to the generated metadata:

| Drift | Guard |
|-------|-------|
| Field/type shape | `codegen --check` (generated wire types) ✅ |
| Protocol version string | generated `_meta`, derived by the serve loop ✅ |
| New method unhandled | test: `meta` methods ⊆ the serve loop's handled set ✅ |
| Capability typo / unknown token | test: advertised capabilities ⊆ `meta` tokens ✅ |
| Emitted messages malformed | conformance test validates them against `schema.json` ✅ |
| **Scoring semantics** (verdict/aggregate/NA) | **not** codegen-able — covered only by behaviour tests + the cross-language golden (`greet` vs `greet-python`) ⚠️ |

The last row is the residual: an SDK mirrors `crate::runner`'s scoring by hand,
so a change to those rules is caught by tests, not by a generated `--check`.

Forward/backward compatibility rides the protocol's existing contract (ignore
unknown fields, default missing fields, capability negotiation), so an SDK and an
older/newer host interoperate without lockstep on *versions* — only on the
*major*.

## 4. Layout

```
sdks/<lang>/        one native SDK per language
  schema codegen    reads ../../schema/v1/{schema.json,meta.json} → generated
                    wire types + protocol metadata (version/methods/capabilities)
  <package>         the library: generated protocol layer + authoring API + serve loop
  tests             schema-conformance (validate emitted messages) + metadata
                    coverage (methods/capabilities/version) + behaviour
```

- The runtime library has **zero third-party dependencies** where the language
  allows (Python: stdlib only; TypeScript: Node built-ins only), so a study runs
  anywhere the interpreter does; validation/codegen/test tools are dev-only.
- **Example studies stay under `examples/`** (the single `just run-examples`
  entry point), importing the SDK; the SDK dir holds only the library + its own
  tests. `examples/greet-python` and `examples/greet-typescript` are the worked
  examples and mirror the Rust `greet`.
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
`initialize`/`list`/`list_samples`/`run`/`execute`/`score` (with
`Study(page_size=…)` paging large datasets). Scoring semantics match
`crate::runner` exactly: an N/A score is excluded from the cell verdict and
aggregate; an unavailable model / infra error short-circuits to a single N/A.

## 6. Status & deferred

- **Python** (`sdks/python`) — implemented: schema-driven codegen, full serve
  loop (incl. the `execute`/`score` split and `list_samples` pagination),
  conformance + behaviour tests.
- **TypeScript** (`sdks/typescript`) — implemented, same shape: a self-contained
  `codegen.mjs` (no external lib, for a hermetic `--check`) generating typed wire
  interfaces + protocol metadata from `schema/v1/`, a `serve()` loop (incl. the
  `execute`/`score` split and `list_samples` pagination), parity authoring API,
  and an npm package (`mira-eval`) with **zero runtime deps** (`ajv` /
  `typescript` are dev-only). Worked example: `examples/greet-typescript`.
- **Publishing:** both SDKs publish to their registries (`mira-eval` on PyPI and
  npm) via OIDC trusted publishing in `publish.yml`, gated on a one-time
  trusted-publisher registration — see [`release-process`](release-process.md).
- **Deferred:** emitting `event` progress notifications from SDK studies.
