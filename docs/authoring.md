# Authoring evals

An `Eval` bundles a dataset, a subject, scorers, and a model matrix. The builder
is the primary surface; datasets can also be loaded from files.

```text
Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
```

## The builder

```rust
use mira::{Eval, Target, Sample};
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
    .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
    .max_turns(8)
    .build();
```

Every method is optional except a subject. With no `targets`, the matrix defaults
to `sim` (offline), so a fresh eval runs without credentials.

## Samples

A `Sample` is one dataset row.

```rust
use mira::Sample;

let s = Sample::new("fix-bug", "Fix the off-by-one in sum().")
    .expected("expected answer")          // for matches_expected / answer-comparison scorers
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
{"id":"b","input":["Fix the bug"],"expected":"42"}
```

Datasets are language-agnostic JSON, so the same file drives Rust, CLI, and
polyglot subjects.

## The model matrix

The **target** is the first-class axis. The runner expands `evals × targets ×
samples` into independently-addressable cells (`greet/hi@sim`).

```rust
.targets([
    Target::sim(),                            // offline, always available
    Target::anthropic("claude-opus-4-8"),     // gated on ANTHROPIC_API_KEY
    Target::openai("gpt-5.5"),                // gated on OPENAI_API_KEY
    Target::new("local", "ollama", "llama3")  // custom provider, always available
        .meta("endpoint", "http://localhost:11434"),
])
```

A cell whose model is **unavailable** (missing API key) is skipped, never failed
— so the default run is green offline and lights up as keys appear. The
`provider` and `model` fields are passed to the subject via `cx.target`; how
they're used is the subject's business.

## Infrastructure errors vs. failures

A run can go wrong two ways, and Mira keeps them apart so you measure the model,
not the weather:

- A **failure** is the model/agent under test getting it wrong — a scorer
  doesn't pass. It counts against the pass-rate; that's the eval's job.
- An **infrastructure error** is the scaffolding around the run breaking: out of
  budget/quota, rate-limited, a provider 5xx/outage, a network/timeout fault.
  *Not the model's fault.*

A subject signals the latter with `Transcript::infra_error(..)` instead of the
subject-attributed `Transcript::failed(..)`:

```rust
.subject(subject_fn(|sample, _cx| async move {
    match call_provider(&sample.input).await {
        Ok(text) => Transcript::response(text),
        // The model answered, but wrongly — a real, scored failure.
        Err(ApiError::BadOutput(e)) => Transcript::failed(e.to_string()),
        // The provider was down / out of budget — not the model's fault.
        Err(ApiError::RateLimited | ApiError::Outage) =>
            Transcript::infra_error("provider unavailable"),
    }
}))
```

An infra error **short-circuits scoring to a single N/A score** — the cell-level
dual of a scorer returning [`Score::na`](scorers.md). The cell is then excluded
from the pass-rate (neither pass nor fail, like a skip), and is **retry-eligible**:
the host's concurrent executor re-queues it (alongside rate-limited cells) up to
`--max-retries`. A cell that stays broken is reported **N/A**, never counted
against the model, so an outage can't turn a green suite red.

The `mira-everruns` adapter does this for you: `classify_runtime_error`
recognises rate-limit, quota, 5xx, timeout, and network phrases as infra, leaving
ambiguous errors attributed to the subject.

## Extra matrix axes

Beyond the model, add arbitrary discrete axes with `.axis(name, values)`. The
runner takes the cross-product of every axis with the model matrix, and the
subject reads the chosen value per cell via `cx.param(name)`:

```rust
let eval = Eval::new("reasoning")
    .case("puzzle", "What is 17 * 23?")
    .axis("effort", ["low", "high"])             // a second axis
    .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
    .subject(subject_fn(|_s, cx| async move {
        let effort = cx.param("effort").unwrap_or("low");
        // …vary behaviour by effort…
        mira::Transcript::response(format!("({effort}) 391"))
    }))
    .scorer(succeeded())
    .build();
```

This expands to `samples × targets × effort` cells, each with a stable key like
`reasoning/puzzle@sim[effort=high]` that selection, checkpoints, and reports use.

## Interactive (multi-turn) evals

By default a subject runs once per case. To evaluate a *conversation*, add a
`.responder(..)` — a simulated user. The runner then drives a turn exchange:
it invokes the subject once per turn with the running conversation in
`cx.conversation`, appends the responder's reply, and repeats until the responder
returns `None` or `max_turns` is reached. The whole dialog is folded into one
transcript the scorers grade — so scoring is unchanged.

```rust
use mira::{Eval, Message, Part, Role, Transcript, subject::subject_fn};
use mira::scorer::{contains, succeeded};

let eval = Eval::new("clarify")
    .case("weather", "What's the weather?")
    .max_turns(4)
    // The subject answers from the conversation so far.
    .subject(subject_fn(|_s, cx| async move {
        let answered = cx.conversation.iter()
            .filter(|m| m.role == Role::User).nth(1).map(Message::text);
        match answered {
            Some(city) => Transcript::response(format!("It's sunny in {city}.")),
            None => Transcript::response("Which city?"),
        }
    }))
    // The simulated user answers the clarifying question once, then stops.
    .responder(|convo: &[Message]| {
        let last = convo.last()?;
        (last.role == Role::Assistant && last.text().contains("Which city"))
            .then(|| vec![Part::text("Paris")])
    })
    .scorer(succeeded())
    .scorer(contains("sunny"))
    .build();
```

This is in-process and needs no protocol feature — the study owns the loop. A
model-graded responder (an LLM playing the user) is just a closure that calls a
judge. Runnable example: `examples/interactive/`.

## Metadata & observability

Metadata is free-form, open-ended `string → JSON` on evals, samples, and
targets — values may be a string, number, bool, or a nested object/array. It rides
through the protocol and surfaces in `list` and reports — the place to put trace
URLs, dashboard deep-links, commit SHAs, and dataset provenance.

```rust
// Per-sample provenance (repo, difficulty, dataset split, …):
Sample::new("hi", "…")
    .meta("trace", "https://observe.example/run/abc123")
    .meta("difficulty", "hard");

// Per-model config riding the model column (agent, effort, price, sandbox, …):
Target::anthropic("claude-opus-4-8")
    .meta("agent", "swe-agent")
    .meta("effort", "high");
```

### Grouping reports by metadata

The host can break resolve-rate down by any metadata (or axis) key with
`--group-by`:

```bash
mira --bin swe_bench run --group-by difficulty   # one resolve-rate row per difficulty
mira --bin swe_bench run --group-by agent         # …or per model-level config key
```

Each cell's group value is resolved in order: axis `params`, then sample
metadata, then model metadata, then transcript metadata. The breakdown prints to
the terminal and is folded into the JSON, Markdown, and HTML reports (a `groups`
block in the JSON record).

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
`.targets(…)`.
