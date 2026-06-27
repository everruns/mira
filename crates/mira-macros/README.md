# mira-macros

Procedural macros for [Mira](https://github.com/everruns/mira), the Rust-first,
code-first evaluation framework for agents and tools.

[![crates.io](https://img.shields.io/crates/v/mira-macros.svg)](https://crates.io/crates/mira-macros)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

This crate provides the **`#[eval]` attribute**, which registers an eval factory
for `cargo test`-style discovery — annotate a function returning `Eval` and the
host finds it automatically, no manual registration list to maintain.

## You don't depend on this directly

`mira-macros` is re-exported by [`mira-eval`](https://crates.io/crates/mira-eval)
as `mira::eval` (enabled by the default `macros` feature). Depend on `mira-eval`
and import the attribute from there:

```rust
use mira::{eval, Eval, Transcript};
use mira::subject::subject_fn;
use mira::scorer::contains;

#[eval]
fn greet() -> Eval {
    Eval::new("greet")
        .sample("hi", "say hi")
        .subject(subject_fn(|_, _| async { Transcript::response("hi there") }))
        .scorer(contains("hi"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // `Study::registered()` collects every `#[eval]`-annotated factory.
    mira::Study::registered().serve().await
}
```

## How discovery works

`#[eval]` leaves the function untouched and additionally submits it to a global
registry, collected at link time via
[`inventory`](https://crates.io/crates/inventory).
`mira::Study::registered()` collects them all, so adding an eval is just adding a
function — the same ergonomics as `#[test]`. It is the declarative alternative to
calling `register_eval!(greet)` by hand.

See the [Mira docs](https://docs.rs/mira-eval) and the
[authoring guide](https://github.com/everruns/mira/blob/main/docs/authoring.md)
for the full guide.

## License

MIT — see [LICENSE](../../LICENSE).
