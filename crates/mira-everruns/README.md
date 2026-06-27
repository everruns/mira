# mira-everruns

A [Mira](https://github.com/everruns/mira) `Subject` adapter for the published
[`everruns-runtime`](https://crates.io/crates/everruns-runtime). `RuntimeSubject`
drives a real `InProcessRuntime` session per sample — the in-process path to
evaluating everruns-based agents.

[![crates.io](https://img.shields.io/crates/v/mira-everruns.svg)](https://crates.io/crates/mira-everruns)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

## Why a separate crate

Mira's core ([`mira-eval`](https://crates.io/crates/mira-eval)) is deliberately
**provider-agnostic**: a `Target` carries `(provider, model)` labels and no SDK
types, and the everruns runtime is a large dependency tree. Keeping the
integration here means the core builds in seconds and never pulls in a provider
SDK. Each provider integration is its own crate; this is the everruns one.

## What it does

`RuntimeSubject` is a Mira `Subject`. For each sample the host runs, it:

1. maps the matrix `Target` onto an everruns `ResolvedModel` (`target_to_resolved`),
2. asks your factory closure to build an `InProcessRuntime` + session for it,
3. plays the sample's turns through the session, and
4. normalizes the runtime's `TurnResult` + `Event` stream into a Mira
   `Transcript` (final response, tool calls, token/cost usage, events) — the same
   shape every other subject produces, so scoring and reporting are shared.

## Usage

```rust
use mira_everruns::{RuntimeSubject, target_to_resolved};

let subject = RuntimeSubject::new(|model| Box::pin(async move {
    let resolved = target_to_resolved(&model); // Target → everruns ResolvedModel
    // …build an InProcessRuntime + session for `resolved`, return (runtime, session_id)…
    # Err::<(everruns_runtime::InProcessRuntime, everruns_core::typed_id::SessionId), String>("wire me".into())
}));
```

Drop the `subject` into an `Eval` like any other and run it with the
[`mira-cli`](https://crates.io/crates/mira-cli) host. The crate also exposes
`classify_runtime_error` to bucket runtime errors into Mira's `ErrorKind` so
infrastructure failures are reported distinctly from genuine eval failures.

See the [Mira docs](https://github.com/everruns/mira/tree/main/docs) — in
particular [subjects](https://github.com/everruns/mira/blob/main/docs/subjects.md).

Licensed under MIT — see [LICENSE](../../LICENSE).
