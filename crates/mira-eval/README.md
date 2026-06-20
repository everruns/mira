# mira-eval

The core of [Mira](https://github.com/everruns/mira) — a Rust-first, code-first
evaluation framework for agents and tools. The library is imported as `mira`.

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
```

```rust
use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Transcript, register_eval};

fn greet() -> Eval {
    Eval::new("greet")
        .case("hi", "Say hi and tell me the answer to life.")
        .subject(subject_fn(|_s, _cx| async move {
            Transcript::response("Hi! The answer is 42.")
        }))
        .scorer(succeeded())
        .scorer(contains("42"))
        .build()
}
register_eval!(greet);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::serve_registered().await
}
```

Run it with the [`mira-cli`](https://crates.io/crates/mira-cli) host:

```bash
mira --example greet run
```

See the [docs](https://github.com/everruns/mira/tree/main/docs) for the full
guide and the [protocol reference](https://github.com/everruns/mira/blob/main/docs/protocol.md).

Licensed under MIT.
