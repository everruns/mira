# Subjects

A `Subject` is the thing under evaluation. It turns a `Sample` into a normalized
`Transcript`, so scorers and reporting never depend on a subject's internals.

```rust
#[async_trait]
pub trait Subject: Send + Sync {
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript;
}
```

`RunCx` carries the matrix cell's `Target` (`cx.target`) and the run limit
(`cx.max_turns`). Each run gets a fresh subject invocation, so state from one
sample cannot leak into another.

A `Transcript` is the shared currency:

```rust
pub struct Transcript {
    pub final_response: String,
    pub iterations: usize,
    pub tool_calls_count: usize,
    pub usage: Usage,                     // typed metrics: tokens + cost
    pub timing: Timing,                   // typed metrics: duration, TTFT
    pub tool_calls: Vec<String>,          // tool names, in order
    pub files: BTreeMap<String, String>,  // workspace after the run
    pub events: Vec<serde_json::Value>,   // raw transcript (e.g. JSONL Events)
    pub metrics: BTreeMap<String, f64>,   // open metrics: any numeric you measure
    pub metadata: Metadata,
    pub error: Option<String>,
    pub error_kind: ErrorKind,            // Subject (default) | Infra (→ N/A, retried)
}
```

Mira ships two general subjects, plus a runtime adapter in `mira-everruns`.

## In-process: `subject_fn`

Wrap an async closure. Ideal for evals that live next to the code under test,
and for fakes in unit tests.

```rust
use mira::{Transcript, subject::subject_fn};

let subject = subject_fn(|sample, cx| async move {
    let reply = my_agent::answer(&sample.input.join("\n"), &cx.target.model).await;
    Transcript::response(reply)
});
```

Populate as much of the transcript as you can measure — `iterations`,
`tool_calls`, `usage`, `files` — so structural and cost scorers have signal.

## Polyglot: `CliSubject`

Run an external binary. This is how **non-Rust agents become first-class**: any
program in any language is evaluable. The prompt (the sample's input turns,
joined by newlines) is passed via a `{prompt}` placeholder or on stdin; seeded
files are materialized into a fresh temp workdir, and `{workdir}` expands to it.

```rust
use mira::subject::{CliSubject, TranscriptSource};

// Pass the prompt as an argument; stdout is the final response.
let s = CliSubject::new("my-agent").arg("--task").arg("{prompt}");

// Or send the prompt on stdin.
let s = CliSubject::new("my-agent").stdin_prompt();

// Read structured results from a canonical JSONL Event stream instead of stdout.
let s = CliSubject::new("coding-cli")
    .arg("--prompt").arg("{prompt}")
    .arg("--transcript").arg("{workdir}/events.jsonl")
    .transcript(TranscriptSource::EventsFile("events.jsonl".into()))
    .capture_files();   // read the workdir back into Transcript.files
```

When reading a JSONL transcript, Mira extracts tool-call names and token/cost
usage structurally — any producer emitting `{input_tokens, output_tokens, cost}`
usage blocks and `{name, input}` tool-call objects is understood, including
everruns coding CLIs. A line with a `final_response` / `response` / `text` field
sets the final response.

The subprocess also receives `MIRA_TARGET` and `MIRA_PROVIDER` env vars so it can
route on the matrix cell.

## Runtime sessions: `mira-everruns`

`mira_everruns::RuntimeSubject` drives a real `everruns-runtime`
`InProcessRuntime` session — the in-process path to evaluating everruns agents.
The embedder supplies a factory that builds a runtime for each matrix cell; Mira
normalizes the `TurnResult` and `Event` stream into a `Transcript`.

```rust
use mira_everruns::{RuntimeSubject, target_to_resolved};

let subject = RuntimeSubject::new(|model| Box::pin(async move {
    let resolved = target_to_resolved(&model);   // Target → everruns ResolvedModel
    // …build an InProcessRuntime registering a driver for `resolved`,
    // create a session, and return (runtime, session_id)…
    Ok((runtime, session_id))
}));
```

See the [`mira-everruns` crate docs](https://docs.rs/mira-everruns) for the full
factory contract.

## Multimodal inputs

A `Sample` carries text turns in `input`; attach non-text input (images, audio,
files, structured JSON) with `attachments`. A subject reads the fused prompt via
`Sample::prompt_parts()` — the text turns followed by the attachments, as one
ordered `Part` list:

```rust
use mira::{Part, Sample};

let sample = Sample::new("vqa", "What format is this image?")
    .image("image/png", "https://example/cat.png")        // or a `data:` URI
    .attach(Part::audio_uri("audio/wav", "https://example/clip.wav"));

// In the subject: send the whole multimodal prompt to the model.
let parts = sample.prompt_parts();          // [Text, Image, Audio]
let kinds = sample.modalities();            // ["text", "image", "audio"]
```

Media is *referenced* (`media_type` + a `uri` or inline base64 `data`), never raw
bytes, so a sample stays plain JSON in a JSONL dataset. This is study-side only —
no protocol change. Runnable example: `examples/multimodal/`. Multimodal *output*
(`Transcript::output`) is staged behind the `protocol-unstable` feature; see
[architecture §14](../specs/architecture.md).

## Writing your own

Reach for a `subject_fn` closure first. Implement `Subject` directly when you
want a **reusable adapter that holds state** — a connection pool, an HTTP client,
an auth token shared across every cell.

The contract:

- **Isolated per call.** One invocation per `(sample, model, axis)` cell; never
  let state from one sample leak into the next.
- **Fill what you can measure.** Set `iterations`, `tool_calls` (names, in order)
  and `tool_calls_count`, `usage`, `timing`, `files` so structural and budget
  scorers have signal. Anything you can't measure stays at its default.
- **Record failures, don't panic.** Put the error in `Transcript.error` (or use
  `Transcript::failed(msg)`); a panicking subject takes down the whole run.
- **Separate infra errors from failures.** When the fault is the scaffolding —
  budget/quota, rate limit, provider outage, network/timeout — use
  `Transcript::infra_error(msg)` instead. Scoring short-circuits to a single
  **N/A** score, so the cell is excluded from the pass-rate (neither pass nor
  fail, like a scorer's `Score::na`) and the host retries it. See
  [authoring](authoring.md#infrastructure-errors-vs-failures).

```rust
use async_trait::async_trait;
use mira::{RunCx, Sample, Transcript, Usage, subject::Subject};

/// Posts the prompt to an HTTP agent and normalizes the reply. The `reqwest`
/// client is built once and shared across every matrix cell.
struct HttpAgent {
    client: reqwest::Client,
    base_url: String,
}

#[async_trait]
impl Subject for HttpAgent {
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript {
        let started = std::time::Instant::now();

        // Route on the matrix cell's model (cx.target.provider / cx.target.model).
        let req = self.client
            .post(format!("{}/run", self.base_url))
            .json(&serde_json::json!({
                "prompt": sample.input.join("\n"),
                "model":  cx.target.model,
                "max_turns": cx.max_turns,
            }));

        // A transport/outage fault is infrastructure, not the model's fault:
        // scored N/A (excluded from pass/fail) and retried by the host.
        let resp = match req.send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(e) => return Transcript::infra_error(format!("agent request failed: {e}")),
        };
        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(e) => return Transcript::failed(format!("bad agent response: {e}")),
        };

        let mut t = Transcript::response(
            body["answer"].as_str().unwrap_or_default().to_string(),
        );
        t.usage = Usage {
            input_tokens:  body["usage"]["in"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["out"].as_u64().unwrap_or(0),
            ..Default::default()
        };
        t.timing.duration_ms = started.elapsed().as_millis() as u64;
        t
    }
}
```

Attach it with `.subject(...)`, or share one instance across several evals with
`.subject_arc(std::sync::Arc::new(agent))`.
