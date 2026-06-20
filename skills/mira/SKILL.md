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
- **Matrix** — `ModelSpec`s plus extra `.axis(name, values)`; missing API keys
  *skip*, so runs are green offline.

## Authoring an eval server

A server is a program that defines evals and calls `mira::serve_registered()`.
Register factories with `#[eval]` (or `register_eval!`).

```rust
use mira::scorer::{file_contains, latency_within, succeeded, tool_called, tokens_within};
use mira::subject::subject_fn;
use mira::{eval, Eval, ModelSpec, Sample, Transcript};

#[eval]
fn coding() -> Eval {
    Eval::new("coding")
        .describe("Edits a file to satisfy an instruction")
        .sample(
            Sample::new("add-fn", "Add a greet function to lib.rs")
                .file("lib.rs", "// here\n")
                .tag("smoke"),
        )
        .subject(subject_fn(|sample, cx| async move {
            // Call the real agent/model (cx.model.provider / cx.model.model).
            // Report metrics the budget scorers grade: usage, timing, tools.
            let mut t = Transcript::response("done");
            t.tool_calls = vec!["edit_file".into()];
            t.tool_calls_count = 1;
            t.usage.output_tokens = 80;
            t.timing.duration_ms = 400;
            t.files.insert("lib.rs".into(), "fn greet() {}\n".into());
            t
        }))
        .scorer(succeeded())
        .scorer(tool_called("edit_file"))
        .scorer(file_contains("lib.rs", "fn greet"))
        .scorer(tokens_within(4_000))
        .scorer(latency_within(5_000))
        .models([ModelSpec::sim(), ModelSpec::anthropic("claude-opus-4-8")])
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> { mira::serve_registered().await }
```

Put servers in `examples/*.rs` (run via `--example NAME`) or as `[[bin]]`s.

## Running

```bash
mira --example coding list                 # advertised evals/samples/scorers/models
mira --example coding run                  # whole matrix
mira --example coding run add-fn           # substring filter on eval/sample@model
mira --example coding run --tag smoke
mira --example coding run --models sim
mira --example coding run --format junit --out results.xml   # CI artifact
mira --example coding run --format html  --out report.html   # transcript viewer
mira --example coding run --checkpoint ck.json               # resumable
```

Exit code is non-zero if any cell failed — drops straight into CI.

## Scorers (cheat sheet)

- **Text/output**: `succeeded` · `non_empty` · `contains` · `not_contains` ·
  `equals` · `regex` · `matches_target` · `json_valid` · `json_field_equals`
- **Tools**: `tool_called` · `tool_not_called` · `tool_calls_within` ·
  `tools_used_exactly` · `tool_called_before`
- **Budgets**: `tokens_within` · `output_tokens_within` · `cost_within` ·
  `turns_within` · `latency_within` · `ttft_within`
- **Files**: `file_exists` · `file_contains`
- **Combinators / custom**: `all_of` · `any_of` · `not` · `scorer(name, closure)`
  · `model_graded(rubric, judge)`

Closure escape hatch:

```rust
use mira::{Score, scorer::scorer};
let s = scorer("nonempty", |_, t| {
    if t.final_response.trim().is_empty() { Score::fail("nonempty", "empty") }
    else { Score::pass("nonempty", "ok") }
});
```

## Polyglot subject (evaluate any binary)

```rust
use mira::subject::{CliSubject, TranscriptSource};
let s = CliSubject::new("my-agent")
    .arg("--prompt").arg("{prompt}")             // or .stdin_prompt()
    .transcript(TranscriptSource::EventsFile("events.jsonl".into()))  // JSONL Events
    .capture_files();                            // read workdir into Transcript.files
```

`{prompt}` and `{workdir}` expand per run; seeded `sample.files` are written into
a fresh temp workdir; `MIRA_MODEL` / `MIRA_PROVIDER` env vars are set.

## In-process testing

```rust
use mira::Runner;
#[tokio::test]
async fn passes() {
    let report = Runner::new().add(coding()).run().await;
    assert!(report.all_passed());
}
```

## References

- `docs/getting-started.md`, `docs/authoring.md`, `docs/scorers.md`,
  `docs/subjects.md`
- `docs/protocol.md` — the wire protocol (for non-Rust servers)
- `specs/architecture.md` — the design of record
