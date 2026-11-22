# mira-everruns

A [Mira](https://github.com/everruns/mira) `Subject` adapter for the published
[Everruns Framework](https://crates.io/crates/everruns) (`everruns`).
`RuntimeSubject` drives a real everruns `Session` per sample, the in-process path
to evaluating everruns-based agents.

[![crates.io](https://img.shields.io/crates/v/mira-everruns.svg)](https://crates.io/crates/mira-everruns)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

## Why a separate crate

Mira's core ([`mira-eval`](https://crates.io/crates/mira-eval)) is deliberately
**provider-agnostic**: a `Target` carries `(provider, model)` labels and no SDK
types, and the everruns framework is a large dependency tree. Keeping the
integration here means the core builds in seconds and never pulls in a provider
SDK. Each provider integration is its own crate; this is the everruns one.

## What it does

`RuntimeSubject` is a Mira `Subject`. For each sample the host runs, it:

1. asks your factory closure to build a `Session` for the matrix `Target`
   (`target_to_model` maps a provider target onto a live everruns `Model`),
2. plays the sample's turns through the session, and
3. normalizes each `Turn` + the session's canonical event stream into a Mira
   `Transcript` (final response, tool calls, token/cost usage, events), the same
   shape every other subject produces, so scoring and reporting are shared.

## Usage

```rust
use everruns::{Agent, Engine};
use mira_everruns::{RuntimeSubject, target_to_model};

let subject = RuntimeSubject::new(|target| async move {
    let agent = Agent::builder()
        .instructions("Answer concisely.")
        .model(target_to_model(&target)?) // Target → live everruns Model
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Engine::new().create(agent)) // a fresh Session per sample
});
```

Drop the `subject` into an `Eval` like any other and run it with the
[`mira-cli`](https://crates.io/crates/mira-cli) host. The crate also exposes
`classify_runtime_error` to bucket runtime errors into Mira's `ErrorKind` so
infrastructure failures are reported distinctly from genuine eval failures.

See the [Mira docs](https://github.com/everruns/mira/tree/main/docs), in
particular [subjects](https://github.com/everruns/mira/blob/main/docs/subjects.md).

Licensed under MIT, see [LICENSE](../../LICENSE).
