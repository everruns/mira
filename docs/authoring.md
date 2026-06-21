# Authoring evals

An `Eval` bundles a dataset, a subject, scorers, and a model matrix. The builder
is the primary surface; datasets can also be loaded from files.

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
```

## The builder

```rust
use mira::{Eval, ModelSpec, Sample};
use mira::subject::subject_fn;
use mira::scorer::{succeeded, contains};

let eval = Eval::new("greet")
    .describe("Greets the user")          // shown in `list`
    .meta("suite", "smoke")               // free-form metadata
    .case("hi", "say hi")                 // inline single-turn sample
    .sample(Sample::new("ho", "say ho").tag("smoke"))  // full sample
    .subject(subject_fn(|s, _| async move { mira::Transcript::response("hi") }))
    .scorer(succeeded())
    .scorer(contains("hi"))
    .models([ModelSpec::sim(), ModelSpec::anthropic("claude-opus-4-8")])
    .max_turns(8)
    .build();
```

Every method is optional except a subject. With no `models`, the matrix defaults
to `sim` (offline), so a fresh eval runs without credentials.

## Samples

A `Sample` is one dataset row.

```rust
use mira::Sample;

let s = Sample::new("fix-bug", "Fix the off-by-one in sum().")
    .target("expected answer")            // for matches_target / target-based scorers
    .file("lib.rs", "fn sum() {}")        // seed the subject's workspace
    .tag("regression")                    // selective runs: --tag regression
    .meta("difficulty", "hard");          // observability / provenance

let multi = Sample::turns("chat", ["hello", "and now in French"]);
```

## Datasets from files

Code-first inlining is the common case; for larger sets, load JSONL or JSON:

```rust
use mira::Dataset;

let eval = Eval::new("swe")
    .dataset(Dataset::jsonl("data/tasks.jsonl")?)   // one Sample per line
    // or Dataset::json("data/tasks.json")           // a top-level array
    .subject(/* … */)
    .build();
```

```jsonl
{"id":"a","input":["Add a greet function"],"files":{"lib.rs":"// todo\n"},"tags":["smoke"]}
{"id":"b","input":["Fix the bug"],"target":"42"}
```

Datasets are language-agnostic JSON, so the same file drives Rust, CLI, and
polyglot subjects.

## The model matrix

`models` is a first-class axis. The runner expands `evals × models × samples`
into independently-addressable cells (`greet/hi@sim`).

```rust
.models([
    ModelSpec::sim(),                            // offline, always available
    ModelSpec::anthropic("claude-opus-4-8"),     // gated on ANTHROPIC_API_KEY
    ModelSpec::openai("gpt-5.5"),                // gated on OPENAI_API_KEY
    ModelSpec::new("local", "ollama", "llama3")  // custom provider, always available
        .meta("endpoint", "http://localhost:11434"),
])
```

A cell whose model is **unavailable** (missing API key) is skipped, never failed
— so the default run is green offline and lights up as keys appear. The
`provider` and `model` fields are passed to the subject via `cx.model`; how
they're used is the subject's business.

## Extra matrix axes

Beyond the model, add arbitrary discrete axes with `.axis(name, values)`. The
runner takes the cross-product of every axis with the model matrix, and the
subject reads the chosen value per cell via `cx.param(name)`:

```rust
let eval = Eval::new("reasoning")
    .case("puzzle", "What is 17 * 23?")
    .axis("effort", ["low", "high"])             // a second axis
    .models([ModelSpec::sim(), ModelSpec::anthropic("claude-opus-4-8")])
    .subject(subject_fn(|_s, cx| async move {
        let effort = cx.param("effort").unwrap_or("low");
        // …vary behaviour by effort…
        mira::Transcript::response(format!("({effort}) 391"))
    }))
    .scorer(succeeded())
    .build();
```

This expands to `samples × models × effort` cells, each with a stable key like
`reasoning/puzzle@sim[effort=high]` that selection, checkpoints, and reports use.

## Metadata & observability

Metadata is free-form `string → string` on evals, samples, and models. It rides
through the protocol and surfaces in `list` and reports — the place to put trace
URLs, dashboard deep-links, commit SHAs, and dataset provenance.

```rust
Sample::new("hi", "…").meta("trace", "https://observe.example/run/abc123")
```

## Registration vs. explicit lists

Annotate factory functions with `#[eval]` and let the study collect them
(`#[eval]` is the ergonomic form of `register_eval!`):

```rust
use mira::{eval, Eval};

#[eval]
fn greet() -> Eval { /* … */ }

#[tokio::main]
async fn main() -> std::io::Result<()> { mira::Study::registered().serve().await }
```

Prefer no proc-macros? `register_eval!(greet);` is equivalent, and disabling the
default `macros` feature drops the `#[eval]` attribute entirely. Or build a
study from an explicit list:

```rust
#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::new().eval(greet()).eval(coding()).serve().await
}
```

## Running in-process

Sometimes you want evals as ordinary `#[tokio::test]`s, with no host process:

```rust
use mira::Runner;

#[tokio::test]
async fn greet_passes() {
    let report = Runner::new().add(greet()).run().await;
    assert!(report.all_passed());
}
```

`Runner` supports the same selection as the CLI: `.filter(…)`, `.tag(…)`,
`.models(…)`.
