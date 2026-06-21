<div align="center">

# Mira

**A Rust-first, code-first evaluation framework for agents and tools.**

[![CI](https://github.com/everruns/mira/actions/workflows/ci.yml/badge.svg)](https://github.com/everruns/mira/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mira-eval.svg)](https://crates.io/crates/mira-eval)
[![docs.rs](https://img.shields.io/docsrs/mira-eval)](https://docs.rs/mira-eval)
[![Crates.io downloads](https://img.shields.io/crates/d/mira-eval.svg)](https://crates.io/crates/mira-eval)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](rust-toolchain.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Part of the [Everruns](https://everruns.com) ecosystem. Originated as the
`proposals/mira` PoC in [everruns/everruns#2345](https://github.com/everruns/everruns/pull/2345).

</div>

Mira is a developer tool shaped like a test runner. You define evals in Rust (or
any language that speaks the [protocol](docs/protocol.md)), and a generic host
CLI runs them across a model matrix, scores the results, and reports — with
selective runs, resumable checkpoints, operational-metric budgets, and CI-native
output (including a self-contained HTML report).

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix × axes
```

- **`Subject`** — the thing under evaluation. One adapter per *shape*: an
  in-process closure, an external binary (`CliSubject`, the **polyglot** path),
  or a live runtime session (`mira-everruns`).
- **`Scorer`** — deterministic built-ins (`contains`, `regex`, `tool_called`,
  `file_contains`, …), operational budgets (`tokens_within`, `cost_within`,
  `latency_within`, `ttft_within`, `tools_used_exactly`, …), combinators
  (`all_of`/`any_of`/`not`), an arbitrary-closure escape hatch, and LLM-as-judge
  (`model_graded`) — one open vocabulary, freely composed.
- **Matrix & axes** — models are a first-class axis; add arbitrary axes
  (`.axis("effort", ["low","high"])`) and the runner takes the cross-product.
  Missing API keys **skip** rather than fail, so a fresh run is green offline.
- **Two processes, one protocol** — your eval program (the *study*) owns
  subjects and scoring; the `mira` CLI (the *host*) owns selection, the matrix,
  checkpoints, and reporting. Provider keys never cross the wire. The
  [protocol](docs/protocol.md) is versioned and forward-compatible.

## Install

The `mira` host CLI, via Homebrew (recommended):

```bash
brew install everruns/tap/mira
```

Works on macOS (arm64/x86_64) and Linux (x86_64). If your Homebrew enforces tap
trust checks, trust the tap once first with `brew trust --tap everruns/tap`.
Building from source instead? `cargo install mira-cli --locked`.

## Quick start

Add the framework and write an eval study:

```toml
# Cargo.toml
[dependencies]
mira-eval = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
// examples/my_evals.rs
use mira::scorer::{contains, succeeded, latency_within};
use mira::subject::subject_fn;
use mira::{eval, Eval, Transcript};

#[eval]
fn greet() -> Eval {
    Eval::new("greet")
        .case("hi", "Say hi and tell me the answer to life.")
        .subject(subject_fn(|_sample, _cx| async move {
            // A real subject calls a model; this one fakes a good answer.
            Transcript::response("Hi! The answer is 42.")
        }))
        .scorer(succeeded())
        .scorer(contains("42"))
        .scorer(latency_within(2_000))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
```

Run it with the host CLI:

```bash
mira --bin my_evals list                 # what the study advertises
mira --bin my_evals run                  # run the whole matrix
mira --bin my_evals run greet            # selective (substring), like cargo test
mira --bin my_evals run --tag smoke
mira --bin my_evals run --format html --out report.html   # self-contained viewer
mira --bin my_evals run --checkpoint ck.json              # resumable long runs
```

See [`docs/getting-started.md`](docs/getting-started.md) for a full walkthrough,
and [`examples/`](examples) for runnable servers (`greet`, `coding`,
`cli_subject`, `metrics`, `matrix`, `swe_bench`, `llmsim`, plus the non-Rust
`greet-python`):

```bash
cargo run -p mira-cli -- --bin metrics run
```

## Why Mira

Teams run agents and tools against datasets in incompatible ways — a Python
SWE-bench harness here, a bespoke Rust string-check bench there, an rstest matrix
somewhere else. Mira is the one framework they can converge on:

- **Code-first authoring** with `cargo test`-style discovery (`#[eval]`) and
  selection.
- **Polyglot by design** — the `CliSubject` evaluates any binary in any language
  that emits the canonical JSONL transcript, so non-Rust agents are first-class.
- **Composable scoring** that generalizes string checks, operational budgets, and
  LLM-judge into one trait.
- **Operational metrics first-class** — tokens (incl. cache/reasoning), cost,
  wall-clock latency, time-to-first-token, and exact tool usage are scorable
  fields, surfaced per-cell in the JSON/HTML reports.
- **Built for CI** — JSON, JUnit, Markdown, and a self-contained HTML report;
  checkpoints for resume; non-zero exit on failure.

## Workspace layout

| Path | Crate | What |
|------|-------|------|
| [`crates/mira-eval`](crates/mira-eval) | `mira-eval` (lib `mira`) | The framework: types, traits, scorers, subjects, protocol, study, host. |
| [`crates/mira-cli`](crates/mira-cli) | `mira-cli` (bin `mira`) | The host CLI that drives eval studies. |
| [`crates/mira-macros`](crates/mira-macros) | `mira-macros` | The `#[eval]` attribute macro (re-exported as `mira::eval`). |
| [`crates/mira-everruns`](crates/mira-everruns) | `mira-everruns` | `RuntimeSubject` over the published `everruns-runtime`. |
| [`examples/`](examples) | per-example crates | Runnable, offline example studies (one self-contained folder each; Rust + a Python study). |
| [`docs/`](docs) | — | Public docs: [getting started](docs/getting-started.md), [extensibility](docs/extensibility.md), and the [protocol reference](docs/protocol.md). |
| [`specs/`](specs) | — | [Architecture](specs/architecture.md) and the [release process](specs/release-process.md). |
| [`Formula/`](Formula) | — | The Homebrew formula (mirrored to the tap on release). |

## Documentation

- [Getting started](docs/getting-started.md)
- [Authoring evals](docs/authoring.md)
- [Scorers](docs/scorers.md)
- [Subjects](docs/subjects.md)
- [The eval protocol](docs/protocol.md) — the wire format, ACP-style reference
- [Architecture](specs/architecture.md)

## Ecosystem

Mira is part of [Everruns](https://everruns.com) — a platform for building,
running, and evaluating agents:

- [everruns.com](https://everruns.com) — the platform.
- [`everruns-runtime`](https://crates.io/crates/everruns-runtime) — the embeddable
  in-process agent runtime that `mira-everruns` drives.
- [github.com/everruns](https://github.com/everruns) — the rest of the ecosystem.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md). Run
`just check` before opening a PR.

## License

MIT — see [LICENSE](LICENSE).
