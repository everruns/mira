# Mira cookbook

Copy-paste recipes for the three subject kinds and for testing. The overview and
when-to-invoke live in [`../SKILL.md`](../SKILL.md); canonical prose is in
[`docs/`](https://github.com/everruns/mira/tree/main/docs).

## Authoring a Rust study

A study is a program that defines evals and calls
`mira::Study::registered().serve()`. Register factories with `#[eval]` (or
`register_eval!`). A study is just a `[[bin]]`; the host resolves it with
`--bin NAME`.

```rust
use mira::scorer::{file_contains, latency_within, succeeded, tool_called, tokens_within};
use mira::subject::subject_fn;
use mira::{eval, Eval, Target, Sample, Transcript};

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
            // Call the real agent/model (cx.target.provider / cx.target.model).
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
        .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> { mira::Study::registered().serve().await }
```

More: <https://github.com/everruns/mira/blob/main/docs/authoring.md>.

## Polyglot subject (evaluate any binary)

```rust
use mira::subject::{CliSubject, TranscriptSource};
let s = CliSubject::new("my-agent")
    .arg("--prompt").arg("{prompt}")             // or .stdin_prompt()
    .transcript(TranscriptSource::EventsFile("events.jsonl".into()))  // JSONL Events
    .capture_files();                            // read workdir into Transcript.files
```

`{prompt}` and `{workdir}` expand per run; seeded `sample.files` are written into
a fresh temp workdir; `MIRA_TARGET` / `MIRA_PROVIDER` env vars are set. Example:
<https://github.com/everruns/mira/tree/main/examples/cli_subject>.

## everruns runtime subject

`mira_everruns::RuntimeSubject` drives a real `everruns-runtime` session (add
`cargo add mira-everruns`). Offline, point it at the `LlmSim` driver — example:
<https://github.com/everruns/mira/tree/main/examples/llmsim>.

## In-process testing

Drive a study from a `#[tokio::test]` with `Runner` — no host binary, no network.

```rust
use mira::Runner;
#[tokio::test]
async fn passes() {
    let report = Runner::new().add(coding()).run().await;
    assert!(report.all_passed());
}
```

## Custom scorer (closure)

```rust
use mira::{Score, scorer::scorer};
let s = scorer("nonempty", |_, t| {
    if t.final_response.trim().is_empty() { Score::fail("nonempty", "empty") }
    else { Score::pass("nonempty", "ok") }
});
```

Full scorer catalog: [`scorers.md`](scorers.md). Custom subjects/scorers as
crates: <https://github.com/everruns/mira/blob/main/docs/extensibility.md>.
