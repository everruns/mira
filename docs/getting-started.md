# Getting started

This walks you from zero to a passing eval run.

## 1. Install

The framework is a library (`mira-eval`, imported as `mira`); the runner is a
binary (`mira-cli`, installed as `mira`).

```bash
cargo add mira-eval
brew install everruns/tap/mira   # or: cargo install mira-cli
```

## 2. Write an eval server

An eval **server** is just a program that defines evals and calls
`mira::serve_registered()`. Put it anywhere `cargo run` can reach it — a binary,
or (handy for libraries) an example:

```rust
// examples/my_evals.rs
use mira::scorer::{contains, succeeded, tool_called};
use mira::subject::subject_fn;
use mira::{eval, Eval, ModelSpec, Sample, Transcript};

#[eval]
fn capital() -> Eval {
    Eval::new("capital")
        .describe("Knows world capitals")
        .sample(Sample::new("france", "What is the capital of France?").target("Paris"))
        .sample(Sample::new("japan", "What is the capital of Japan?").target("Tokyo"))
        .subject(subject_fn(|sample, cx| async move {
            // Replace this with a real model call keyed on `cx.model`.
            let answer = match sample.id.as_str() {
                "france" => "The capital of France is Paris.",
                _ => "The capital of Japan is Tokyo.",
            };
            let _ = cx; // model is available as cx.model
            Transcript::response(answer)
        }))
        .scorer(succeeded())
        .scorer(mira::scorer::matches_target()) // compares to Sample.target
        .models([ModelSpec::sim(), ModelSpec::anthropic("claude-opus-4-8")])
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::serve_registered().await
}
```

## 3. Run it

```bash
mira --example my_evals list
```

```text
capital — Knows world capitals  (max_turns=12)
  samples: france, japan
  scorers: succeeded, matches_target
  models:  sim, anthropic/claude-opus-4-8 (unavailable)
```

The cloud cell is **unavailable** because `ANTHROPIC_API_KEY` isn't set — it will
be skipped, not failed. Run the matrix:

```bash
mira --example my_evals run
```

```text
── matrix (passed/ran) ──
  eval         sim  anthropic/claude-opus-4-8
  capital      2/2                          —

2 passed / 2 ran (0 failed, 2 skipped)
```

Set `ANTHROPIC_API_KEY` and the cloud column lights up too.

## 4. Select, report, resume

```bash
mira --example my_evals run france                 # substring filter on the case key
mira --example my_evals run --tag smoke            # by sample tag
mira --example my_evals run --models sim           # restrict the matrix
mira --example my_evals run --format junit --out results.xml   # CI artifact
mira --example my_evals run --format html  --out report.html   # self-contained viewer
mira --example my_evals run --checkpoint ck.json   # resumable; re-run skips done cells
```

The exit code is non-zero if any cell failed, so `mira ... run` drops straight
into CI. The HTML report is a single dependency-free file (summary, matrix, and
per-case scores/usage/timing) you can open straight from a CI artifact.

## Next steps

- [Authoring evals](authoring.md) — datasets, the matrix, extra axes, metadata.
- [Scorers](scorers.md) — the built-ins (incl. metric budgets) and writing your own.
- [Subjects](subjects.md) — in-process, CLI/polyglot, and runtime sessions.
- [The protocol](protocol.md) — what flows over the wire, and its versioning.
