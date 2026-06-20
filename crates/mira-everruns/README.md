# mira-everruns

A [Mira](https://github.com/everruns/mira) `Subject` adapter for the published
[`everruns-runtime`](https://crates.io/crates/everruns-runtime). `RuntimeSubject`
drives a real `InProcessRuntime` session per sample — the in-process path to
evaluating everruns-based agents.

```rust
use mira_everruns::{RuntimeSubject, model_to_resolved};

let subject = RuntimeSubject::new(|model| Box::pin(async move {
    let resolved = model_to_resolved(&model); // ModelSpec → everruns ResolvedModel
    // …build an InProcessRuntime + session for `resolved`, return (runtime, session_id)…
    # Err::<(everruns_runtime::InProcessRuntime, everruns_core::typed_id::SessionId), String>("wire me".into())
}));
```

Mira's core stays provider-agnostic; this crate maps a `ModelSpec` onto the
everruns runtime and normalizes its `TurnResult` + `Event` stream into a Mira
`Transcript`. Licensed under MIT.
