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

## Extending

- [Extensibility](extensibility.md) — the map of every seam: subjects, scorers,
  metrics, events, and protocol-level extension.

## Reference

- [The eval protocol](protocol.md) — the normative wire format and its
  forward-compatible versioning.

## Diagrams

Conceptual diagrams are committed SVGs under [`assets/`](assets). See the
[docs spec](../specs/docs.md#3-diagrams) for the convention.
