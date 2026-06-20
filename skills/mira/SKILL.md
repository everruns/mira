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
- **Scorer** — grades a `Transcript`: built-ins, closures, or `model_graded`.
- **Matrix** — `ModelSpec`s; missing API keys *skip*, so runs are green offline.

## Authoring an eval server

A server is a program that defines evals and calls `mira::serve_registered()`.

```rust
use mira::scorer::{contains, succeeded, tool_called, file_contains};
use mira::subject::subject_fn;
use mira::{Eval, ModelSpec, Sample, Transcript, register_eval};

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
            let mut t = Transcript::response("done");
            t.tool_calls = vec!["edit_file".into()];
            t.tool_calls_count = 1;
            t.files.insert("lib.rs".into(), "fn greet() {}\n".into());
            t
        }))
        .scorer(succeeded())
        .scorer(tool_called("edit_file"))
        .scorer(file_contains("lib.rs", "fn greet"))
        .models([ModelSpec::sim(), ModelSpec::anthropic("claude-opus-4-8")])
        .build()
}
register_eval!(coding);

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
mira --example coding run --checkpoint ck.json               # resumable
```

Exit code is non-zero if any cell failed — drops straight into CI.

## Scorers (cheat sheet)

`succeeded` · `contains` · `not_contains` · `equals` · `regex` ·
`matches_target` · `tool_called` · `tool_calls_within` · `turns_within` ·
`cost_within` · `file_exists` · `file_contains` · `scorer(name, closure)` ·
`model_graded(rubric, judge)`.

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
- `specs/SPEC.md` — the design of record
