# mira-macros

Procedural macros for [Mira](https://github.com/everruns/mira), the Rust-first,
code-first evaluation framework for agents and tools.

This crate provides the `#[eval]` attribute, which registers an eval factory for
`cargo test`-style discovery. You do not depend on it directly — it is
re-exported by [`mira-eval`](https://crates.io/crates/mira-eval) as `mira::eval`
(enabled by the default `macros` feature):

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
```

See the [Mira docs](https://docs.rs/mira-eval) for the full guide.

## License

MIT — see [LICENSE](../../LICENSE).
