# mira-eval — TypeScript SDK

Author a Mira eval **study** in TypeScript and run it with the `mira` host CLI.

This is **not** a binding to the Rust core — it's a native, zero-dependency
Node library that speaks the [Mira eval protocol](../../docs/protocol.md)
(newline-delimited JSON over stdio). The host owns selection, the target matrix,
concurrency, saved runs, and reporting; the study owns subjects and scoring. Any
language that speaks the protocol is a first-class study — this SDK just makes
the TypeScript side ergonomic and fully typed.

The protocol layer is **generated** from the canonical artifacts under
[`schema/v1/`](../../schema/v1/) — the same language-neutral contract the Rust
host is generated from — so it never drifts from the wire format:
[`src/wire.ts`](src/wire.ts) (wire types, from `schema.json`) and
[`src/meta.ts`](src/meta.ts) (protocol version, methods, capability tokens, from
`meta.json`).

## Install

```bash
npm install mira-eval
```

Runtime dependencies: **none**. A study runs anywhere Node ≥ 18 does. (`ajv` and
`typescript` are dev-only — for the conformance test and the build.)

## Use

```ts
import { Study, sample, target, succeeded, contains, transcript, usage } from "mira-eval";

const study = new Study("my-evals", { version: "0.1.0" });

study.eval({
  name: "greet",
  samples: [sample("hi", { prompt: "Say hi and the answer to life.", tags: ["smoke"] })],
  targets: [target("sim")],
  scorers: [succeeded(), contains("42")],
  run: (s, cx) => {
    // A real subject calls a model; route on cx.target / cx.provider.
    return transcript(`Hi! The answer is 42. (${s.text})`, {
      usage: usage({ inputTokens: 40, outputTokens: 8 }),
    });
  },
});

study.serve();
```

Subjects can be **async** — return a `Promise<Transcript>` to `await` a model
call:

```ts
study.eval({
  name: "qa",
  samples: [sample("capital", { prompt: "Capital of France?" })],
  targets: [target("gpt-4o-mini", { provider: "openai" })],
  scorers: [contains("Paris")],
  run: async (s, cx) => {
    const reply = await myModel(cx.provider, cx.target, s.text);
    return transcript(reply);
  },
});
```

Drive it with the host (writing the study to `study.mjs` after `tsc`, or running
a `.ts` entry with your loader of choice):

```bash
mira --cmd "node study.mjs" list
mira --cmd "node study.mjs" run
# run-now, score-later (split execute/score path):
mira --cmd "node study.mjs" run --execute-only --artifacts art/
mira --cmd "node study.mjs" score --artifacts art/
```

A complete, runnable example lives in
[`examples/greet-typescript`](../../examples/greet-typescript).

## API

- **`new Study(name, { version?, pageSize? })`** — the registry. `study.eval({…})`
  registers an eval (chainable); `study.serve()` runs the stdio loop (handling
  `initialize`/`list`/`list_samples`/`run`/`execute`/`score`/`cancel`).
  `pageSize` (default `500`) paginates large datasets across `list` +
  `list_samples`; `0` disables it (every sample inline).
- **`study.eval({ name, samples, targets, run, scorers?, description?, axes?, maxTurns?, metadata? })`**
  — `run(sample, cx) => Transcript | Promise<Transcript>` is the subject.
- **`sample(id, { prompt? | input?, tags?, expected?, files?, metadata? })`** —
  one dataset row; `sample.text` is the prompt, or the input turns joined.
- **`target(label, { provider?, available?, metadata? })`** — a matrix case (the
  model or harness under evaluation). An unavailable target is reported as
  **N/A** (infra), not a failure.
- **`RunCx`** — the per-case context: `cx.target`, `cx.provider`, `cx.maxTurns`,
  `cx.param(name, default?)` (axis values).
- **`transcript(finalResponse, { usage?, timing?, iterations?, toolCalls?, metrics?, metadata?, error?, errorKind?, … })`**
  plus the `usage({…})` and `timing({…})` builders.
- **Scorers** — `succeeded()`, `contains(text)`, `equals(text)`,
  `regex(pattern)`, and `scorer(name, fn)` for an arbitrary predicate (return a
  boolean, or a fully-formed `Score` including `na: true`). `makeScore(name,
  value, pass, reason, na?)` builds one by hand.
- **`axis(name, values)`** — an extra matrix axis (crossed with the target
  matrix); read it in a subject via `cx.param(name)`.

Scoring semantics match the Rust `crate::runner` exactly: an N/A score is
excluded from the case verdict and the aggregate; an unavailable target / infra
error short-circuits to a single N/A (neither pass nor fail).

## How it stays in sync

The wire types and protocol metadata are **generated**, never hand-mirrored, so
a protocol bump can't silently drift this SDK:

| Drift | Guard |
|-------|-------|
| Field / type shape | `codegen.mjs --check` (generated `wire.ts`) |
| Protocol version string | generated `meta.ts`, derived by the serve loop |
| New method left unhandled | test: `METHODS` ⊆ the serve loop's handled set |
| Capability typo / unknown token | test: advertised capabilities ⊆ `meta` tokens |
| Emitted messages malformed | conformance test validates them against `schema.json` |

## Develop

```bash
npm install
npm run codegen          # regenerate src/wire.ts + src/meta.ts from schema/v1/
npm run codegen:check    # fail if either is stale (CI drift guard)
npm run build            # tsc -> dist/
npm test                 # codegen check + build + conformance/metadata/serve tests
```

The runtime has **zero dependencies**; everything above is dev-only. See
[`specs/sdks.md`](../../specs/sdks.md) for the design of record.
