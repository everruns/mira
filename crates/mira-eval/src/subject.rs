//! [`Subject`]: the thing under evaluation. Implementors turn a [`Sample`] into
//! a [`Transcript`]. Each run gets a fresh subject invocation (isolation), so
//! state from one sample cannot leak into another.
//!
//! Mira ships two general subjects:
//!
//! * [`subject_fn`] — wrap an async closure. The in-process path: ideal for
//!   evals that live next to the code under test, and for tests.
//! * [`CliSubject`] — run an external binary and read back its result. The
//!   **polyglot** path: any agent in any language becomes evaluable. Tool-using
//!   agents should write an ATIF trajectory file
//!   ([`TranscriptSource::AtifFile`] — the structured contract, see
//!   [`crate::trajectory`]); plain stdout and the JSONL `Event` heuristics
//!   remain for text-only agents and producer-shaped streams.
//!
//! Richer in-process adapters (e.g. driving a live runtime session) live in
//! integration crates such as `mira-everruns`.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;

use async_trait::async_trait;

use crate::{RunCx, Sample, Transcript, Usage};

/// The thing being evaluated.
#[async_trait]
pub trait Subject: Send + Sync {
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript;
}

// ----- closure subject ------------------------------------------------------

type SubjectFuture = Pin<Box<dyn Future<Output = Transcript> + Send>>;

struct FnSubject<F> {
    f: F,
}

#[async_trait]
impl<F> Subject for FnSubject<F>
where
    F: Fn(Sample, RunCx) -> SubjectFuture + Send + Sync,
{
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript {
        (self.f)(sample.clone(), cx.clone()).await
    }
}

/// Build a [`Subject`] from an async closure.
///
/// ```
/// use mira::{subject::subject_fn, Transcript};
/// let subject = subject_fn(|sample, _cx| async move {
///     Transcript::response(format!("echo: {}", sample.input.join(" ")))
/// });
/// ```
pub fn subject_fn<F, Fut>(f: F) -> impl Subject
where
    F: Fn(Sample, RunCx) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Transcript> + Send + 'static,
{
    FnSubject {
        f: move |s, c| Box::pin(f(s, c)) as SubjectFuture,
    }
}

// ----- CLI subject ----------------------------------------------------------

/// Where a [`CliSubject`] reads the run result from.
#[derive(Clone, Debug)]
pub enum TranscriptSource {
    /// The captured stdout is the final response (default).
    Stdout,
    /// Stdout is a JSONL `Event` stream; parse it into the transcript.
    EventsStdout,
    /// A file (relative to the workdir) holds the JSONL `Event` stream.
    EventsFile(String),
    /// A file (relative to the workdir) holds one ATIF trajectory JSON
    /// document (e.g. `"trajectory.json"`) — the **recommended** source for
    /// tool-using external agents. The parsed document becomes
    /// [`Transcript::trajectory`]; the flat fields (`final_response`,
    /// `tool_calls`, `usage`, `iterations`) are derived from it (see
    /// [`crate::trajectory`]). A file, not stdout: ATIF is a single JSON
    /// document (with an `images/` sidecar convention), not a JSONL stream,
    /// and harbor-ecosystem agents already write `trajectory.json`. The child
    /// process receives the absolute path as `MIRA_TRAJECTORY_PATH`.
    AtifFile(String),
}

/// Runs an external binary as the subject under evaluation — the polyglot path.
///
/// The prompt (the sample's input turns, joined by newlines) is supplied either
/// as a `{prompt}` placeholder in the arguments or on stdin. Seeded
/// `sample.files` are materialized into a fresh temp workdir, and `{workdir}` in
/// the arguments expands to its path. The result is read per
/// [`TranscriptSource`].
pub struct CliSubject {
    program: String,
    args: Vec<String>,
    stdin_prompt: bool,
    source: TranscriptSource,
    capture_files: bool,
    env: BTreeMap<String, String>,
}

impl CliSubject {
    /// A subject that runs `program`. Add arguments with [`arg`](Self::arg).
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            stdin_prompt: false,
            source: TranscriptSource::Stdout,
            capture_files: false,
            env: BTreeMap::new(),
        }
    }

    /// Add one argument. `{prompt}` and `{workdir}` placeholders are expanded
    /// per run.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Send the prompt on stdin instead of via a `{prompt}` placeholder.
    pub fn stdin_prompt(mut self) -> Self {
        self.stdin_prompt = true;
        self
    }

    /// Parse the result from a JSONL `Event` stream rather than treating stdout
    /// as the final response.
    pub fn transcript(mut self, source: TranscriptSource) -> Self {
        self.source = source;
        self
    }

    /// Read back the workdir's files into [`Transcript::files`] after the run,
    /// so file-based scorers can inspect what the subject produced.
    pub fn capture_files(mut self) -> Self {
        self.capture_files = true;
        self
    }

    /// Set an environment variable for the subprocess.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    fn expand(&self, raw: &str, prompt: &str, workdir: &str) -> String {
        raw.replace("{prompt}", prompt)
            .replace("{workdir}", workdir)
    }
}

#[async_trait]
impl Subject for CliSubject {
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript {
        // Fresh isolated workdir; seeded files written in, optionally read back.
        let workdir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(e) => return Transcript::failed(format!("workdir: {e}")),
        };
        let workdir_path = workdir.path().to_path_buf();
        if let Err(e) = seed_files(&workdir_path, &sample.files).await {
            return Transcript::failed(format!("seed files: {e}"));
        }

        let prompt = sample.input.join("\n");
        let workdir_str = workdir_path.to_string_lossy().to_string();
        let started = std::time::Instant::now();

        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.current_dir(&workdir_path);
        for arg in &self.args {
            cmd.arg(self.expand(arg, &prompt, &workdir_str));
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.env("MIRA_TARGET", &cx.target.label);
        cmd.env("MIRA_PROVIDER", &cx.target.provider);
        // Hint where the agent should write its ATIF document (absolute, so
        // the agent needs no workdir knowledge of its own).
        if let TranscriptSource::AtifFile(rel) = &self.source {
            cmd.env("MIRA_TRAJECTORY_PATH", workdir_path.join(rel));
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if self.stdin_prompt {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Transcript::failed(format!("spawn {}: {e}", self.program)),
        };

        if self.stdin_prompt
            && let Some(mut stdin) = child.stdin.take()
        {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(prompt.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let output = match child.wait_with_output().await {
            Ok(o) => o,
            Err(e) => return Transcript::failed(format!("wait: {e}")),
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let mut transcript = Transcript::default();
        if !output.status.success() {
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into());
            transcript.error = Some(format!("exit {code}: {}", stderr.trim()));
        }

        match &self.source {
            TranscriptSource::Stdout => {
                transcript.final_response = stdout.trim().to_string();
            }
            TranscriptSource::EventsStdout => apply_events(&mut transcript, &stdout),
            TranscriptSource::EventsFile(rel) => {
                match tokio::fs::read_to_string(workdir_path.join(rel)).await {
                    Ok(text) => apply_events(&mut transcript, &text),
                    Err(e) => {
                        transcript.error.get_or_insert(format!("read {rel}: {e}"));
                    }
                }
            }
            TranscriptSource::AtifFile(rel) => {
                // Read/parse failures are subject errors (like EventsFile):
                // the agent was asked to produce a trajectory and didn't.
                match tokio::fs::read_to_string(workdir_path.join(rel)).await {
                    Ok(text) => match crate::trajectory::Trajectory::from_json(&text) {
                        Ok(trajectory) => {
                            transcript.trajectory = Some(trajectory);
                            // Canonical projection — never hand-rolled here.
                            transcript.project_trajectory();
                        }
                        Err(e) => {
                            transcript.error.get_or_insert(format!("{rel}: {e}"));
                        }
                    },
                    Err(e) => {
                        transcript.error.get_or_insert(format!("read {rel}: {e}"));
                    }
                }
            }
        }

        if self.capture_files {
            transcript.files = read_files(&workdir_path).await;
        }
        transcript.timing.duration_ms = duration_ms;

        transcript
    }
}

async fn seed_files(root: &Path, files: &BTreeMap<String, String>) -> std::io::Result<()> {
    for (rel, contents) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, contents).await?;
    }
    Ok(())
}

async fn read_files(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = tokio::fs::read_to_string(&path).await
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.insert(rel.to_string_lossy().to_string(), text);
            }
        }
    }
    out
}

/// Parse a JSONL `Event` stream into a transcript: each non-blank line is a JSON
/// value; tool names and token usage are extracted structurally. A line with a
/// `final_response`/`response`/`text` field updates the final response.
fn apply_events(transcript: &mut Transcript, jsonl: &str) {
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(text) = value
            .get("final_response")
            .or_else(|| value.get("response"))
            .or_else(|| value.get("text"))
            .and_then(|v| v.as_str())
        {
            transcript.final_response = text.to_string();
        }
        transcript.events.push(value);
    }
    let (usage, tools) = summarize_events(&transcript.events);
    transcript.usage = usage;
    transcript.tool_calls_count = tools.len();
    transcript.tool_calls = tools;
}

/// Walk a serialized event stream and total up token/cost usage and tool-call
/// names. Walking the JSON keeps this robust to a producer's internal struct
/// shape — anything that emits `{input_tokens, output_tokens, cost}` usage
/// blocks and `{name, input}` tool-call objects is understood.
pub fn summarize_events(events: &[serde_json::Value]) -> (Usage, Vec<String>) {
    let mut usage = Usage::default();
    let mut tools = Vec::new();
    for event in events {
        walk(event, &mut usage, &mut tools);
    }
    (usage, tools)
}

fn walk(value: &serde_json::Value, usage: &mut Usage, tools: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(input) = map.get("input_tokens").and_then(|v| v.as_u64()) {
                usage.input_tokens += input;
                usage.output_tokens += map
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                usage.cache_read_tokens += map
                    .get("cache_read_tokens")
                    .or_else(|| map.get("cached_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                usage.reasoning_tokens += map
                    .get("reasoning_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if let Some(cost) = map
                    .get("cost")
                    .or_else(|| map.get("cost_usd"))
                    .and_then(|v| v.as_f64())
                {
                    usage.cost_usd += cost;
                }
            }
            if map.contains_key("input")
                && let Some(name) = map.get("name").and_then(|v| v.as_str())
            {
                tools.push(name.to_string());
            }
            for child in map.values() {
                walk(child, usage, tools);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk(item, usage, tools);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Target;

    fn cx() -> RunCx {
        RunCx {
            target: Target::sim(),
            max_turns: 8,
            params: Default::default(),
            trial: crate::Trial::single(),
            conversation: Vec::new(),
        }
    }

    #[tokio::test]
    async fn fn_subject_runs() {
        let s = subject_fn(|sample, _| async move { Transcript::response(sample.input.join(",")) });
        let t = s.run(&Sample::turns("a", ["x", "y"]), &cx()).await;
        assert_eq!(t.final_response, "x,y");
    }

    #[tokio::test]
    async fn cli_subject_stdout() {
        // `printf` echoes the expanded prompt to stdout.
        let s = CliSubject::new("printf").arg("%s").arg("{prompt}");
        let t = s.run(&Sample::new("a", "hello world"), &cx()).await;
        assert_eq!(t.final_response, "hello world");
        assert!(t.succeeded());
    }

    #[tokio::test]
    async fn cli_subject_stdin() {
        let s = CliSubject::new("cat").stdin_prompt();
        let t = s.run(&Sample::new("a", "from stdin"), &cx()).await;
        assert_eq!(t.final_response, "from stdin");
    }

    #[tokio::test]
    async fn cli_subject_seeds_and_captures_files() {
        // Append a line to the seeded file, then capture it back.
        let s = CliSubject::new("sh")
            .arg("-c")
            .arg("echo added >> note.txt; printf done")
            .capture_files();
        let sample = Sample::new("a", "ignored").file("note.txt", "seed\n");
        let t = s.run(&sample, &cx()).await;
        assert_eq!(t.final_response, "done");
        assert_eq!(t.files.get("note.txt").unwrap(), "seed\nadded\n");
    }

    #[tokio::test]
    async fn cli_subject_nonzero_exit_is_error() {
        let s = CliSubject::new("sh").arg("-c").arg("echo boom >&2; exit 3");
        let t = s.run(&Sample::new("a", "x"), &cx()).await;
        assert!(!t.succeeded());
        assert!(t.error.as_ref().unwrap().contains("exit 3"));
        assert!(t.error.as_ref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn cli_subject_parses_event_stream() {
        let jsonl = r#"{"name":"read_file","input":{"path":"a"}}
{"usage":{"input_tokens":10,"output_tokens":4,"cost":0.02}}
{"final_response":"all done"}"#;
        let s = CliSubject::new("printf")
            .arg("%s")
            .arg("{prompt}")
            .transcript(TranscriptSource::EventsStdout);
        let t = s.run(&Sample::new("a", jsonl), &cx()).await;
        assert_eq!(t.final_response, "all done");
        assert_eq!(t.tool_calls, vec!["read_file"]);
        assert_eq!(t.usage.input_tokens, 10);
        assert_eq!(t.usage.output_tokens, 4);
    }

    /// A minimal-but-real ATIF document: two tool calls with arguments, a
    /// correlated observation, per-step metrics, and a final agent message.
    const ATIF_DOC: &str = r#"{
        "schema_version": "ATIF-v1.7",
        "agent": {"name": "fake-cli-agent", "version": "1.0"},
        "steps": [
            {"step_id": 1, "source": "user", "message": "look up the price"},
            {"step_id": 2, "source": "agent", "message": "",
             "tool_calls": [
                {"tool_call_id": "c1", "function_name": "financial_search",
                 "arguments": {"ticker": "GOOGL"}},
                {"tool_call_id": "c2", "function_name": "fetch_page",
                 "arguments": {"url": "https://example"}}
             ],
             "observation": {"results": [
                {"source_call_id": "c1", "content": "$185.35"}
             ]},
             "metrics": {"prompt_tokens": 100, "completion_tokens": 20, "cost_usd": 0.001}},
            {"step_id": 3, "source": "agent", "message": "GOOGL trades at $185.35."}
        ]
    }"#;

    #[tokio::test]
    async fn cli_subject_reads_atif_trajectory_file() {
        // The child writes its ATIF document to the hinted path — proving both
        // the MIRA_TRAJECTORY_PATH env export and the AtifFile read-back.
        let s = CliSubject::new("sh")
            .arg("-c")
            .arg(format!(
                "printf '%s' '{}' > \"$MIRA_TRAJECTORY_PATH\"",
                ATIF_DOC.replace('\n', " ")
            ))
            .transcript(TranscriptSource::AtifFile("trajectory.json".into()));
        let t = s.run(&Sample::new("a", "look up the price"), &cx()).await;

        assert!(t.succeeded(), "error: {:?}", t.error);
        // Flat fields are projections of the trajectory — nothing hand-rolled.
        assert_eq!(t.final_response, "GOOGL trades at $185.35.");
        assert_eq!(t.tool_calls, vec!["financial_search", "fetch_page"]);
        assert_eq!(t.tool_calls_count, 2);
        assert_eq!(t.iterations, 2);
        assert_eq!(t.usage.input_tokens, 100);
        assert!((t.usage.cost_usd - 0.001).abs() < 1e-12);
        // The structured layer is present: arguments + correlated observation.
        let trajectory = t.trajectory.as_ref().expect("trajectory attached");
        assert_eq!(trajectory.agent.name, "fake-cli-agent");
        let calls = t.tool_invocations();
        assert_eq!(calls[0].arguments.unwrap()["ticker"], "GOOGL");
        assert_eq!(calls[0].result.unwrap().text(), "$185.35");
    }

    #[tokio::test]
    async fn cli_subject_atif_names_feed_name_based_scorers() {
        // End-to-end: an external agent writes ATIF, and the existing
        // name-based scorer grades the projected tool names.
        let s = CliSubject::new("sh")
            .arg("-c")
            .arg(format!(
                "printf '%s' '{}' > trajectory.json",
                ATIF_DOC.replace('\n', " ")
            ))
            .transcript(TranscriptSource::AtifFile("trajectory.json".into()));
        let sample = Sample::new("a", "look up the price");
        let t = s.run(&sample, &cx()).await;

        let pass = crate::scorer::tool_called("financial_search")
            .score(&sample, &t)
            .await;
        assert!(pass.pass, "reason: {}", pass.reason);
        let fail = crate::scorer::tool_called("write_file")
            .score(&sample, &t)
            .await;
        assert!(!fail.pass);
    }

    #[tokio::test]
    async fn cli_subject_atif_parse_failure_is_subject_error() {
        // Invalid ATIF (and a missing file) degrade to a subject-kind error on
        // the transcript — same policy as EventsFile read errors, never a panic.
        let s = CliSubject::new("sh")
            .arg("-c")
            .arg("printf '{not json' > trajectory.json")
            .transcript(TranscriptSource::AtifFile("trajectory.json".into()));
        let t = s.run(&Sample::new("a", "x"), &cx()).await;
        assert!(!t.succeeded());
        assert!(!t.errored_infra());
        let err = t.error.as_ref().unwrap();
        assert!(err.contains("trajectory.json"), "got: {err}");
        assert!(err.contains("invalid ATIF trajectory"), "got: {err}");
        assert!(t.trajectory.is_none());

        // Missing file: the read error is reported, attributed to the file.
        let s = CliSubject::new("true").transcript(TranscriptSource::AtifFile("t.json".into()));
        let t = s.run(&Sample::new("a", "x"), &cx()).await;
        assert!(!t.succeeded());
        assert!(t.error.as_ref().unwrap().contains("read t.json"));
    }

    #[test]
    fn summarize_walks_nested() {
        let events = vec![serde_json::json!({
            "turn": {"usage": {"input_tokens": 3, "output_tokens": 1, "cost": 0.5}},
            "calls": [{"name": "grep", "input": {}}]
        })];
        let (usage, tools) = summarize_events(&events);
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(tools, vec!["grep"]);
    }
}
