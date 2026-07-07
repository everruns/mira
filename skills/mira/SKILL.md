---
name: mira
description: >-
  Author and run Mira evaluations — the Rust-first, code-first eval framework
  for agents and tools. Use when writing eval suites, adding scorers/subjects,
  running evals across a model matrix, wiring evals into CI, or driving the
  `mira` host CLI. Covers in-process (`subject_fn`), polyglot (`CliSubject`),
  and everruns runtime subjects.
---

# Mira evals

Mira is a developer tool shaped like a test runner.

```
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
```

- **Subject** — what's under test: `subject_fn` (in-process), `CliSubject`
  (any external binary, the polyglot path), or `mira_everruns::RuntimeSubject`.
- **Scorer** — grades a `Transcript`: built-ins (text, tools, budgets, files),
  combinators (`all_of`/`any_of`/`not`), closures, or `model_graded`.
- **Matrix** — `Target`s plus extra `.axis(name, values)`; missing API keys
  *skip*, so runs are green offline.

## Install

The framework is a library (`mira-eval`, imported as `mira`); the runner is a
binary (`mira-cli`, installed as `mira`).

**CLI (`mira` host)** — install the prebuilt binary; don't build from source:

```bash
brew install everruns/tap/mira      # prebuilt binary (recommended)
```

Prebuilt binaries for macOS (arm64/x86_64) and Linux (x86_64) are also attached
to each GitHub Release: <https://github.com/everruns/mira/releases>. If Homebrew
enforces tap trust checks, run `brew trust --tap everruns/tap` once first.
Building from source (`cargo install mira-cli --locked`) is the fallback only.

**Framework (Rust studies)** — add the library to your crate:

```bash
cargo add mira-eval                 # the eval framework, used as `mira::…`
cargo add mira-everruns             # + everruns runtime subject (optional)
```

Cross-language studies need no Rust framework at all — see [SDKs](#cross-language-studies-sdks).

## Authoring an eval study

A study is a program that defines evals and calls
`mira::Study::registered().serve()`; register factories with `#[eval]`. The
lightest form is a **single file** (`study.rs`) with cargo-script frontmatter for
its deps — run with `--script study.rs`, no `Cargo.toml`. The same code also
works as a crate `[[bin]]` (`--bin NAME`) or `examples/*.rs` (`--example NAME`).

```rust
#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2024"

[dependencies]
mira-eval = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
---
use mira::scorer::{file_contains, succeeded};
use mira::subject::subject_fn;
use mira::{eval, Eval, Sample, Target, Transcript};

#[eval]
fn coding() -> Eval {
    Eval::new("coding")
        .describe("Edits a file to satisfy an instruction")
        .add_sample(Sample::new("add-fn", "Add a greet function to lib.rs").file("lib.rs", "// here\n"))
        .subject(subject_fn(|sample, cx| async move {
            // Call the real agent/model (cx.target.provider / cx.target.model);
            // report the metrics the budget scorers grade.
            let mut t = Transcript::response("done");
            t.files.insert("lib.rs".into(), "fn greet() {}\n".into());
            t
        }))
        .scorer(succeeded())
        .scorer(file_contains("lib.rs", "fn greet"))
        .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> { mira::Study::registered().serve().await }
```

The host shims cargo-script onto **stable** (materializes a throwaway crate from
the frontmatter); `MIRA_SCRIPT_NATIVE=1` uses `cargo -Zscript` on nightly.

Full example (tools + budget scorers + main), the polyglot `CliSubject`, the
everruns runtime subject, in-process `Runner` tests, and custom scorers:
[`references/cookbook.md`](references/cookbook.md). A non-Rust study runs via
`--cmd "..."` — see [`examples/greet-python/`](https://github.com/everruns/mira/tree/main/examples/greet-python).

## Running

```bash
mira --script study.rs list                 # advertised evals/samples/scorers/targets
mira --script study.rs run                  # whole matrix
mira --script study.rs run add-fn           # substring filter on eval/sample@target
mira --script study.rs run --tag smoke
mira --script study.rs run --targets sim                      # restrict the target axis
mira --script study.rs run --axis effort=low                  # restrict any declared axis
mira --script study.rs run --preset smoke                     # saved selection from mira.toml
mira --script study.rs run --format junit --out results.xml   # CI artifact
mira --script study.rs run --format html  --out report.html   # transcript viewer
mira --script study.rs run                                    # saves a run folder by default
mira --script study.rs run --resume <run_id>                  # resume; run only the missing cases
mira report <run_id>                                          # re-render a saved run's reports
mira --bin NAME run                    # a crate study (workspace bin)
mira --cmd "python3 study.py" run      # a study written in another language
mira --bin coding doctor               # diagnose config/study/saved runs; --fix repairs
```

Exit code is non-zero if any case failed — drops straight into CI. Run
`mira help --full` for an overview, every flag, examples, and links.

When a setup misbehaves (typo'd `mira.toml` keys, a preset that selects
nothing, duplicate sample ids, unavailable targets, torn run folders), run
`mira doctor`: it lints the config, the study's advertised listing, and the
saved-run store, and `mira doctor --fix` applies the safe repairs (removing
leftover temp files, re-rendering missing reports). Errors exit non-zero.

## Scorers

A case passes only if every `.scorer(...)` passes. Families: **text/output**
(`succeeded`, `contains`, `regex`, `json_field_equals`…), **tools**
(`tool_called`, `tools_used_exactly`, `tool_called_before`…), **budgets**
(`tokens_within`, `cost_within`, `latency_within`…), **files** (`file_exists`,
`file_contains`), and **combinators / custom** (`all_of`, `any_of`, `not`,
`scorer(name, closure)`, `model_graded(rubric, judge)`).

Full catalog with semantics: [`references/scorers.md`](references/scorers.md).

## Subjects

What's under test — pick one per eval:

- `subject_fn(...)` — in-process Rust (see Authoring above).
- `CliSubject` — evaluate **any external binary** (the polyglot path).
- `mira_everruns::RuntimeSubject` — a real everruns runtime session.

Recipes for all three (+ in-process `Runner` tests):
[`references/cookbook.md`](references/cookbook.md).

## Cross-language studies (SDKs)

Any language that speaks the protocol is a first-class study: the host owns
selection, the model matrix, concurrency, saved runs, and reporting; the study
owns subjects and scoring. The SDKs are native (not FFI bindings) and generated
from the canonical schema, so they never drift from the wire format.

- Python SDK — <https://github.com/everruns/mira/blob/main/sdks/python/README.md>
- Wire protocol (write your own, any language) — <https://github.com/everruns/mira/blob/main/docs/protocol.md>
- Worked example — <https://github.com/everruns/mira/tree/main/examples/greet-python>
- Run it: `mira --cmd "python3 study.py" run`

## Examples (runnable, offline)

All run against the `sim` model with no API keys, so they stay green in CI and
cost nothing. Browse: <https://github.com/everruns/mira/tree/main/examples>

- `greet` — smallest eval, single-file (`--script`): `#[eval]`, a closure subject, text + LLM-judge scorers — <https://github.com/everruns/mira/blob/main/examples/greet.rs>
- `coding` — single-file (`--script`): seeded files, a model matrix, structural + file scorers — <https://github.com/everruns/mira/blob/main/examples/coding.rs>
- `cli_subject` — crate (`--bin`): the polyglot path, driving an external program — <https://github.com/everruns/mira/tree/main/examples/cli_subject>
- `matrix` — crate (`--bin`): a multi-axis matrix (targets × a custom `effort` axis) — <https://github.com/everruns/mira/tree/main/examples/matrix>
- `greet-python` — a whole study in Python via the SDK — <https://github.com/everruns/mira/tree/main/examples/greet-python>

```bash
cargo run -p mira-cli -- --script examples/greet.rs run                   # a single-file Rust example
cargo run -p mira-cli -- --bin matrix run                                 # a crate example
cargo run -p mira-cli -- --cmd "python3 examples/greet-python/study.py" run  # polyglot
```

## Learn more (read on demand)

Progressive disclosure: this skill is the overview. Bundled references ship with
the skill (offline) — read them first:

- [`references/cookbook.md`](references/cookbook.md) — recipes for every subject
  kind, in-process tests, and custom scorers.
- [`references/scorers.md`](references/scorers.md) — the full scorer catalog.

Canonical prose lives in the repo docs — open one only when the task needs that
depth:

| Doc | When to read |
|-----|--------------|
| [getting-started](https://github.com/everruns/mira/blob/main/docs/getting-started.md) | First study, end-to-end. |
| [authoring](https://github.com/everruns/mira/blob/main/docs/authoring.md) | Evals, samples, targets, axes, presets. |
| [scorers](https://github.com/everruns/mira/blob/main/docs/scorers.md) | Every built-in scorer + LLM-as-judge. |
| [subjects](https://github.com/everruns/mira/blob/main/docs/subjects.md) | `subject_fn`, `CliSubject`, runtime. |
| [metrics](https://github.com/everruns/mira/blob/main/docs/metrics.md) | Usage/timing/tools the budget scorers grade. |
| [extensibility](https://github.com/everruns/mira/blob/main/docs/extensibility.md) | Custom scorers/subjects. |
| [how-it-works](https://github.com/everruns/mira/blob/main/docs/how-it-works.md) | Core model + vocabulary. |
| [protocol](https://github.com/everruns/mira/blob/main/docs/protocol.md) | Wire format for non-Rust studies. |
| [specs/architecture](https://github.com/everruns/mira/blob/main/specs/architecture.md) | Design of record (the *why*). |

Or run `mira help --full` for the self-orienting CLI guide (overview, every flag,
examples, links).
