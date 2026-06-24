# Mira documentation

User-facing guides and the wire-protocol reference. The crate-level API docs are
rustdoc on the crates (`cargo doc --no-deps --open`); the design of record lives
in [`specs/`](../specs). Conventions for this directory are in
[`specs/docs.md`](../specs/docs.md).

## Start here

- [How it works](how-it-works.md) — the core model and moving parts, end to end.
- [Getting started](getting-started.md) — zero to a passing run.

## Authoring

- [Authoring evals](authoring.md) — datasets, the model matrix, extra axes,
  metadata, infra-errors-vs-failures.
- [Scorers](scorers.md) — built-ins, budgets, combinators, closures, LLM-judge.
- [Metrics](metrics.md) — tokens/cost/latency and custom numeric metrics.
- [Subjects](subjects.md) — in-process, CLI/polyglot, and runtime sessions.
- [Python SDK](../sdks/python) — author a study in Python (native library,
  protocol over stdio, no Rust dependency).
- [TypeScript SDK](../sdks/typescript) — author a study in TypeScript/Node
  (native, zero-dependency library, protocol over stdio, no Rust dependency).

## Extending

- [Extensibility](extensibility.md) — the map of every seam: subjects, scorers,
  metrics, events, and protocol-level extension.

## Reference

- [The eval protocol](protocol.md) — the normative wire format and its
  forward-compatible versioning.

## Diagrams

Conceptual diagrams are committed SVGs under [`assets/`](assets). See the
[docs spec](../specs/docs.md#3-diagrams) for the convention.

- [`mira-overview.svg`](assets/mira-overview.svg) — host ▸ study ▸ subject, at a
  glance (embedded in the repo `README.md`).
- [`mira-workflow.svg`](assets/mira-workflow.svg) — the end-to-end pipeline from
  authoring an eval to a CI-ready report (in [`getting-started.md`](getting-started.md)).
- [`mira-entities.svg`](assets/mira-entities.svg) — the entity hierarchy: study,
  eval, and the cases/trials/scores the matrix expands into (in
  [`authoring.md`](authoring.md#the-entity-hierarchy)).
- [`mira-run-lifecycle.svg`](assets/mira-run-lifecycle.svg) — the host ⇄ study
  protocol sequence for one run (in [`how-it-works.md`](how-it-works.md#two-processes-one-protocol)).
- [`mira-subjects.svg`](assets/mira-subjects.svg) — the three subject shapes all
  normalising into one `Transcript` (in [`subjects.md`](subjects.md)).
- [`mira-scoring.svg`](assets/mira-scoring.svg) — how scorers read a transcript's
  surfaces and combine into a case verdict (in [`scorers.md`](scorers.md)).
