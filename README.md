<div align="center">

# Mira

**A Rust-first, code-first evaluation framework for agents and tools.**

[![CI](https://github.com/everruns/mira/actions/workflows/ci.yml/badge.svg)](https://github.com/everruns/mira/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mira-eval.svg)](https://crates.io/crates/mira-eval)
[![docs.rs](https://img.shields.io/docsrs/mira-eval)](https://docs.rs/mira-eval)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Part of the [Everruns](https://github.com/everruns) ecosystem.

</div>

Mira is a developer tool shaped like a test runner. You define evals in Rust (or
any language that speaks the [protocol](docs/protocol.md)), and a generic host
CLI runs them across a model matrix, scores the results, and reports — with
selective runs, resumable checkpoints, and CI-native output.

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
```

- **`Subject`** — the thing under evaluation. One adapter per *shape*: an
  in-process closure, an external binary (`CliSubject`, the **polyglot** path),
  or a live runtime session (`mira-everruns`).
- **`Scorer`** — deterministic built-ins (`contains`, `regex`, `tool_called`,
  `file_contains`, `cost_within`, …), an arbitrary-closure escape hatch, and
  LLM-as-judge (`model_graded`) — one open vocabulary, freely composed.
- **Model matrix** — a first-class axis. Missing API keys **skip** rather than
  fail, so a fresh run is green offline.
- **Two processes, one protocol** — your eval program (the *server*) owns
  subjects and scoring; the `mira` CLI (the *host*) owns selection, the matrix,
  checkpoints, and reporting. Provider keys never cross the wire.

## Quick start

Add the framework and write an eval server:

```toml
# Cargo.toml
[dependencies]
mira-eval = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
// examples/my_evals.rs
use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Transcript, register_eval};

fn greet() -> Eval {
    Eval::new("greet")
        .case("hi", "Say hi and tell me the answer to life.")
        .subject(subject_fn(|sample, _cx| async move {
            // A real subject calls a model; this one fakes a good answer.
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

Install the host CLI and run it:

```bash
cargo install mira-cli            # installs the `mira` binary

mira --example my_evals list      # what the server advertises
mira --example my_evals run       # run the whole matrix
mira --example my_evals run greet # selective (substring), like cargo test
mira --example my_evals run --tag smoke
mira --example my_evals run --models sim --format junit --out results.xml
mira --example my_evals run --checkpoint ck.json   # resumable long runs
```

See [`docs/getting-started.md`](docs/getting-started.md) for a full walkthrough.

## Why Mira

Teams run agents and tools against datasets in incompatible ways — a Python
SWE-bench harness here, a bespoke Rust string-check bench there, an rstest matrix
somewhere else. Mira is the one framework they can converge on:

- **Code-first authoring** with `cargo test`-style discovery and selection.
- **Polyglot by design** — the `CliSubject` evaluates any binary in any language
  that emits the canonical JSONL transcript, so non-Rust agents are first-class.
- **Composable scoring** that generalizes string checks and LLM-judge into one
  trait.
- **Built for CI** — JSON, JUnit, and Markdown output; checkpoints for resume.

## Workspace layout

| Path | Crate | What |
|------|-------|------|
| [`crates/mira-eval`](crates/mira-eval) | `mira-eval` (lib `mira`) | The framework: types, traits, scorers, subjects, protocol, server, host. |
| [`crates/mira-cli`](crates/mira-cli) | `mira-cli` (bin `mira`) | The host CLI that drives eval servers. |
| [`crates/mira-everruns`](crates/mira-everruns) | `mira-everruns` | `RuntimeSubject` over the published `everruns-runtime`. |
| [`crates/mira-eval/examples`](crates/mira-eval/examples) | — | Runnable eval servers: `greet`, `coding`, `cli_subject`. |
| [`docs/`](docs) | — | Public docs incl. the [protocol reference](docs/protocol.md). |
| [`specs/`](specs) | — | Design specs and the [release process](specs/release-process.md). |

## Documentation

- [Getting started](docs/getting-started.md)
- [Authoring evals](docs/authoring.md)
- [Scorers](docs/scorers.md)
- [Subjects](docs/subjects.md)
- [The eval protocol](docs/protocol.md) — the wire format, ACP-style reference
- [Design spec](specs/SPEC.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md). Run
`just check` before opening a PR.

## License

MIT — see [LICENSE](LICENSE).
