//! Mira `Subject` adapter for [`everruns-runtime`](https://crates.io/crates/everruns-runtime).
//!
//! [`RuntimeSubject`] drives a real `InProcessRuntime` session for each sample —
//! the in-process path to evaluating everruns-based agents (the canonical
//! migration target for everruns' `llm-tests` and the everruns coding CLI).
//!
//! Mira's core stays provider-agnostic: a [`Target`] carries only
//! `(provider, model)` labels. The **embedder** owns runtime/driver/tool wiring
//! via a [`RuntimeFactory`] closure that maps a `Target` onto a built runtime
//! and the session to drive. This crate just normalizes the runtime's
//! `TurnResult` and `Event` stream into a Mira [`Transcript`], so every Mira
//! scorer and report works unchanged.
//!
//! ```no_run
//! use std::sync::Arc;
//! use mira::{Eval, scorer::contains};
//! use mira_everruns::RuntimeSubject;
//! # use everruns_runtime::InProcessRuntime;
//! # use everruns_core::typed_id::SessionId;
//!
//! // The embedder builds a runtime for each matrix case (provider/model from
//! // the Target) and returns it with the session id to drive.
//! let subject = RuntimeSubject::new(|model| Box::pin(async move {
//!     // ... construct an InProcessRuntime registering a driver for
//!     // `target.provider` / `target.model`, then return (runtime, session_id).
//!     # let _ = model;
//!     # Err::<(InProcessRuntime, SessionId), String>("wire me up".into())
//! }));
//!
//! let _eval = Eval::new("greet")
//!     .sample("hi", "Say hi and tell me the answer.")
//!     .subject(subject)
//!     .scorer(contains("42"));
//! ```
//!
//! See `mira_everruns::target_to_resolved` for mapping a `Target` onto an
//! everruns `ResolvedModel` inside your factory.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use everruns_core::provider::DriverId;
use everruns_core::typed_id::SessionId;
use everruns_core::{
    ContentPart, Event, EventData, InputMessage, Message, ResolvedModel, TokenUsage,
};
use everruns_runtime::InProcessRuntime;

use mira::subject::summarize_events;
use mira::trajectory::{
    Agent, ObservationResult, Step, StepContent, StepSource, ToolCall as AtifToolCall, Trajectory,
};
use mira::{ErrorKind, RunCx, Sample, Subject, Target, Transcript};

/// The `everruns-runtime` version line this adapter is built against — stamped
/// as `agent.version` on folded trajectories. Kept in lockstep with the
/// workspace's `everruns-runtime` dependency pin (a test asserts this).
const EVERRUNS_RUNTIME_VERSION: &str = "0.15";

/// A built runtime plus the session to drive for one matrix case.
pub type RuntimeHandle = (InProcessRuntime, SessionId);

/// Builds a fresh runtime for a given matrix case. The embedder owns
/// platform/capability/tool/driver wiring here — Mira stays agnostic to it.
pub type RuntimeFactory = Box<
    dyn Fn(Target) -> Pin<Box<dyn Future<Output = Result<RuntimeHandle, String>> + Send>>
        + Send
        + Sync,
>;

/// Drives a real `everruns-runtime` session: sends each input turn, then
/// normalizes `TurnResult` + the event stream into a Mira [`Transcript`].
pub struct RuntimeSubject {
    factory: RuntimeFactory,
}

impl RuntimeSubject {
    /// Build from a factory closure. Each sample gets a fresh runtime (no state
    /// leaks across samples).
    pub fn new<F, Fut>(factory: F) -> Self
    where
        F: Fn(Target) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<RuntimeHandle, String>> + Send + 'static,
    {
        Self {
            factory: Box::new(move |m| Box::pin(factory(m))),
        }
    }
}

#[async_trait]
impl Subject for RuntimeSubject {
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript {
        let started = std::time::Instant::now();
        let (runtime, session_id) = match (self.factory)(cx.target.clone()).await {
            // Building the runtime failed before the model ran a turn — that's
            // scaffolding (config/transport), so attribute it to infrastructure.
            Err(e) => return Transcript::infra_error(format!("runtime build failed: {e}")),
            Ok(handle) => handle,
        };

        let mut transcript = Transcript::default();
        for prompt in &sample.input {
            match runtime
                .run_turn(session_id, InputMessage::user(prompt.clone()))
                .await
            {
                Ok(result) => {
                    transcript.final_response = result.response;
                    transcript.iterations += result.iterations;
                    if !result.success {
                        // A failed turn may be the model's doing or the
                        // provider's; classify by the error text.
                        let msg = result.error.unwrap_or_else(|| "turn failed".into());
                        transcript.error_kind = classify_runtime_error(&msg);
                        transcript.error = Some(msg);
                        break;
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    transcript.error_kind = classify_runtime_error(&msg);
                    transcript.error = Some(msg);
                    break;
                }
            }
        }

        if let Ok(events) = runtime.events().await {
            // The primary structured contract: fold the typed event stream
            // into an ATIF trajectory. The raw `events` channel stays as-is
            // for now (debugging; its retirement is a later decision).
            let mut trajectory = atif_from_events(&events);
            if trajectory.agent.model_name.is_none() {
                trajectory.agent.model_name = Some(cx.target.model.clone());
            }
            transcript.trajectory = Some(trajectory);
            for event in &events {
                if let Ok(value) = serde_json::to_value(event) {
                    transcript.events.push(value);
                }
            }
        }
        // `summarize_events` totals usage from a generic JSON walk, but its
        // tool-call detection only matches `{ name, input }` objects, which the
        // everruns stream never emits — tool calls arrive as `tool.completed`
        // events keyed by `data.tool_name`. Pull names from that event shape
        // here (the adapter owns it) so tool-selection scorers see real calls.
        // See EVE-676.
        let (usage, _) = summarize_events(&transcript.events);
        transcript.usage = usage;
        transcript.tool_calls = extract_tool_calls(&transcript.events);
        transcript.tool_calls_count = transcript.tool_calls.len();
        transcript.timing.duration_ms = started.elapsed().as_millis() as u64;
        // Fill any still-default flat fields from the trajectory (the explicit
        // values set above always win — see `Trajectory::project_into`).
        transcript.project_trajectory();
        transcript
    }
}

// ----- events → ATIF fold ---------------------------------------------------

/// All text parts of an everruns message, joined by newlines (non-text parts
/// are skipped — ATIF images are referenced files, not inline bytes).
fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|p| p.as_text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map an everruns [`TokenUsage`] block onto ATIF step metrics:
/// `input → prompt`, `output → completion`, `cache_read → cached`, and the
/// effective (actual-else-estimated) cost. `cache_creation_tokens` has no ATIF
/// slot, so it rides in `extra` — the same convention as `reasoning_tokens`.
fn merge_usage(step: &mut Step, usage: &TokenUsage) {
    let m = step.metrics.get_or_insert_with(Default::default);
    m.prompt_tokens = Some(u64::from(usage.input_tokens));
    m.completion_tokens = Some(u64::from(usage.output_tokens));
    m.cached_tokens = usage.cache_read_tokens.map(u64::from);
    m.cost_usd = usage.effective_cost_usd();
    if let Some(creation) = usage.cache_creation_tokens {
        m.extra.insert(
            "cache_creation_tokens".into(),
            serde_json::json!(u64::from(creation)),
        );
    }
}

/// Fold state: the agent step currently being assembled, flushed into
/// `steps` when the next step begins (or the stream ends).
struct Fold {
    steps: Vec<Step>,
    current: Option<Step>,
}

impl Fold {
    fn flush(&mut self) {
        if let Some(mut step) = self.current.take() {
            step.step_id = self.steps.len() as u64 + 1;
            self.steps.push(step);
        }
    }

    fn push(&mut self, mut step: Step) {
        self.flush();
        step.step_id = self.steps.len() as u64 + 1;
        self.steps.push(step);
    }

    /// The agent step under assembly. Normally opened by `reason.started`;
    /// a tool/output event arriving without one (partial stream) opens a
    /// synthetic agent step so its data is never dropped.
    fn agent_step(&mut self, event: &Event) -> &mut Step {
        self.current.get_or_insert_with(|| {
            let mut step = Step::new(0, StepSource::Agent, "");
            step.timestamp = Some(event.ts.to_rfc3339());
            step
        })
    }
}

/// Fold a typed everruns `Event` stream into an ATIF [`Trajectory`] — one
/// agent step per reasoning iteration (`reason.started` opens a step; thinking,
/// the iteration's tool calls, their observations, and the final assistant
/// message attach to it).
///
/// Mapping, on the typed event structs (never JSON key-grubbing):
/// * `input.message` → a `user` step.
/// * `reason.thinking.completed` → the step's `reasoning_content`.
/// * `reason.completed` → per-step metrics from its usage block
///   (`llm_call_count = 1`); the preview text seeds the step message until the
///   full `output.message.completed` text replaces it.
/// * `reason.item` token counts → `metrics.extra.reasoning_tokens` (ATIF has
///   no first-class slot; the RFC's convention, which the usage projection
///   reads back).
/// * `tool.started` / `tool.call_requested` → structured
///   `ToolCall { tool_call_id, function_name, arguments }` (deduped by id).
/// * `tool.completed` → a correlated `ObservationResult` — failures included,
///   same policy as [`extract_tool_calls`] (EVE-676): the model chose to call
///   the tool. A completion whose call was never announced still synthesizes
///   the call (arguments unknown → `{}`), so completed-only streams count.
/// * `output.message.completed` → the step's full message text (and its usage
///   block, only when the iteration's `reason.completed` didn't carry one —
///   they describe the same LLM call, so counting both would double-bill).
///
/// `agent` is `everruns-runtime`/[`EVERRUNS_RUNTIME_VERSION`]; `model_name`
/// comes from the first model the stream mentions ([`RuntimeSubject`] falls
/// back to the target's model when the stream never names one).
pub fn atif_from_events(events: &[Event]) -> Trajectory {
    let mut trajectory = Trajectory::new(Agent::new("everruns-runtime", EVERRUNS_RUNTIME_VERSION));
    trajectory.session_id = events.first().map(|e| e.session_id.to_string());

    let mut fold = Fold {
        steps: Vec::new(),
        current: None,
    };
    for event in events {
        match &event.data {
            EventData::InputMessage(d) => {
                let mut step = Step::new(0, StepSource::User, message_text(&d.message));
                step.timestamp = Some(event.ts.to_rfc3339());
                fold.push(step);
            }
            EventData::ReasonStarted(d) => {
                fold.flush();
                let step = fold.agent_step(event);
                step.llm_call_count = Some(1);
                if let Some(meta) = &d.metadata {
                    step.model_name = Some(meta.model.clone());
                    if trajectory.agent.model_name.is_none() {
                        trajectory.agent.model_name = Some(meta.model.clone());
                    }
                }
            }
            EventData::ReasonThinkingCompleted(d) => {
                let step = fold.agent_step(event);
                match &mut step.reasoning_content {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(&d.thinking);
                    }
                    none => *none = Some(d.thinking.clone()),
                }
            }
            EventData::ReasonItem(d) => {
                if let Some(count) = d.token_count {
                    let m = fold
                        .agent_step(event)
                        .metrics
                        .get_or_insert_with(Default::default);
                    let prev = m
                        .extra
                        .get("reasoning_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    m.extra.insert(
                        "reasoning_tokens".into(),
                        serde_json::json!(prev + u64::from(count)),
                    );
                }
            }
            EventData::ReasonCompleted(d) => {
                let step = fold.agent_step(event);
                if let Some(usage) = &d.usage {
                    merge_usage(step, usage);
                }
                if step.message.is_empty()
                    && let Some(preview) = &d.text_preview
                {
                    step.message = preview.as_str().into();
                }
            }
            EventData::ToolStarted(d) => {
                push_tool_call(fold.agent_step(event), &d.tool_call);
            }
            EventData::ToolCallRequested(d) => {
                let step = fold.agent_step(event);
                for call in &d.tool_calls {
                    push_tool_call(step, call);
                }
            }
            EventData::ToolCompleted(d) => {
                let step = fold.agent_step(event);
                if !step
                    .tool_calls
                    .iter()
                    .any(|c| c.tool_call_id == d.tool_call_id)
                {
                    step.tool_calls.push(AtifToolCall::new(
                        d.tool_call_id.clone(),
                        d.tool_name.clone(),
                        serde_json::json!({}),
                    ));
                }
                let content: StepContent = match (&d.error, &d.result) {
                    (Some(error), _) => error.as_str().into(),
                    (None, Some(parts)) => parts
                        .iter()
                        .filter_map(ContentPart::as_text)
                        .collect::<Vec<_>>()
                        .join("\n")
                        .into(),
                    (None, None) => StepContent::default(),
                };
                let mut result = ObservationResult {
                    source_call_id: Some(d.tool_call_id.clone()),
                    content: Some(content),
                    ..Default::default()
                };
                if !d.success {
                    result
                        .extra
                        .insert("status".into(), serde_json::json!(d.status));
                }
                step.observation
                    .get_or_insert_with(Default::default)
                    .results
                    .push(result);
            }
            EventData::OutputMessageCompleted(d) => {
                let step = fold.agent_step(event);
                let text = message_text(&d.message);
                if !text.is_empty() {
                    step.message = text.into();
                }
                // Same LLM call as this iteration's reason.completed — only
                // use its usage when that event didn't carry one.
                if step.metrics.is_none()
                    && let Some(usage) = &d.usage
                {
                    merge_usage(step, usage);
                }
                if trajectory.agent.model_name.is_none()
                    && let Some(meta) = &d.metadata
                {
                    trajectory.agent.model_name = Some(meta.model.clone());
                }
            }
            EventData::LlmGeneration(d) if trajectory.agent.model_name.is_none() => {
                trajectory.agent.model_name = Some(d.metadata.model.clone());
            }
            _ => {}
        }
    }
    fold.flush();
    trajectory.steps = fold.steps;
    trajectory
}

/// Record a structured tool call on the step, deduped by id (`tool.started`
/// and `tool.call_requested` may both announce the same call).
fn push_tool_call(step: &mut Step, call: &everruns_core::ToolCall) {
    if step.tool_calls.iter().any(|c| c.tool_call_id == call.id) {
        return;
    }
    step.tool_calls.push(AtifToolCall::new(
        call.id.clone(),
        call.name.clone(),
        call.arguments.clone(),
    ));
}

/// Tool names from the everruns event stream, in order, one per completed call.
///
/// The runtime serializes each finished tool call as a `tool.completed` event
/// (`everruns_core::TOOL_COMPLETED`) whose `data` is a `ToolCompletedData` with
/// a `tool_name` field. This matches that shape directly rather than relying on
/// Mira's generic `{ name, input }` walk, which the everruns stream never hits.
/// Failed calls still emit `tool.completed` (with `success: false`), so they
/// count as invocations — a tool the model chose to call, which is what
/// tool-selection scorers ask about.
fn extract_tool_calls(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some(everruns_core::TOOL_COMPLETED))
        .filter_map(|e| {
            e.get("data")
                .and_then(|d| d.get("tool_name"))
                .and_then(|n| n.as_str())
                .map(String::from)
        })
        .collect()
}

/// Classify a runtime/provider error string as infrastructure vs. subject.
///
/// The runtime surfaces a string, not a typed error, so this is a deliberately
/// conservative keyword heuristic: rate limits (via [`mira::is_rate_limited`])
/// plus the unambiguous "not the model's fault" signals — quota/budget,
/// provider 5xx, and network/timeout faults — map to [`ErrorKind::Infra`]
/// (scored N/A and retried). Everything else stays [`ErrorKind::Subject`] (a
/// real, scoreable failure), so a genuine model error is never silently excused.
pub fn classify_runtime_error(message: &str) -> ErrorKind {
    if mira::is_rate_limited(message) {
        return ErrorKind::Infra;
    }
    let m = message.to_ascii_lowercase();
    const INFRA_SIGNALS: &[&str] = &[
        "budget",
        "billing",
        "out of credit",
        "insufficient funds",
        "service unavailable",
        "503",
        "502",
        "500",
        "bad gateway",
        "gateway timeout",
        "timed out",
        "timeout",
        "connection refused",
        "connection reset",
        "connection closed",
        "broken pipe",
        "network unreachable",
        "network error",
        "dns error",
        "tls handshake",
        "econnreset",
        "temporarily unavailable",
    ];
    if INFRA_SIGNALS.iter().any(|s| m.contains(s)) {
        ErrorKind::Infra
    } else {
        ErrorKind::Subject
    }
}

/// Map a Mira [`Target`] onto an everruns [`ResolvedModel`]. A convenience
/// for factory authors; reads the provider's API key from the conventional env
/// var. Unknown providers default to [`DriverId::LlmSim`] (offline).
pub fn target_to_resolved(target: &Target) -> ResolvedModel {
    let (provider_type, api_key) = match target.provider.as_str() {
        "anthropic" => (DriverId::Anthropic, std::env::var("ANTHROPIC_API_KEY").ok()),
        "openai" => (DriverId::OpenAI, std::env::var("OPENAI_API_KEY").ok()),
        "gemini" => (DriverId::Gemini, std::env::var("GEMINI_API_KEY").ok()),
        "openrouter" => (
            DriverId::OpenRouter,
            std::env::var("OPENROUTER_API_KEY").ok(),
        ),
        _ => (DriverId::LlmSim, Some("sim".to_string())),
    };
    ResolvedModel {
        model: target.model.clone(),
        provider_type,
        api_key,
        base_url: None,
        provider_metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use everruns_core::typed_id::{HarnessId, TurnId};
    use everruns_core::{
        Event, EventContext, InputMessageData, ModelMetadata, OutputMessageCompletedData,
        ReasonCompletedData, ReasonStartedData, ReasonThinkingCompletedData, ToolCall,
        ToolCompletedData, ToolStartedData,
    };
    use serde_json::json;

    /// Serialize a real everruns `tool.completed` event the way the runtime
    /// does, so the assertion pins the on-the-wire shape: if `tool.completed`
    /// or `data.tool_name` ever drifts, this reconstructs the *new* shape and
    /// the extractor (which hardcodes the old keys) returns empty — a loud
    /// failure instead of the silent zero-tool-call scoring of EVE-676.
    fn tool_completed_event(tool_name: &str) -> serde_json::Value {
        let data =
            ToolCompletedData::success("call_1".into(), tool_name.into(), Vec::new(), Some(3));
        let event = Event::new(SessionId::new(), EventContext::empty(), data);
        serde_json::to_value(&event).expect("event serializes")
    }

    #[test]
    fn extracts_tool_names_from_completed_events() {
        let events = vec![
            tool_completed_event("read_file"),
            tool_completed_event("write_file"),
        ];
        assert_eq!(extract_tool_calls(&events), vec!["read_file", "write_file"]);
    }

    #[test]
    fn extract_ignores_non_tool_events_and_generic_shapes() {
        // A `{ name, input }` object (Mira's generic tool shape) and an
        // unrelated event must not be counted — only `tool.completed` is.
        let events = vec![
            serde_json::json!({ "type": "output.message.completed", "data": { "text": "hi" } }),
            serde_json::json!({ "name": "read_file", "input": { "path": "x" } }),
        ];
        assert!(extract_tool_calls(&events).is_empty());
    }

    #[test]
    fn extract_includes_failed_tool_calls() {
        // A failed tool call still emits `tool.completed`; the model chose to
        // call the tool, so tool-selection scorers should see it.
        let data = ToolCompletedData::failure(
            "call_2".into(),
            "search".into(),
            "error".into(),
            "boom".into(),
            None,
        );
        let event = Event::new(SessionId::new(), EventContext::empty(), data);
        let value = serde_json::to_value(&event).expect("event serializes");
        assert_eq!(extract_tool_calls(&[value]), vec!["search"]);
    }

    /// A typed event exactly as the runtime constructs it. Building through
    /// `Event::new` + the real data constructors pins the shape the fold
    /// consumes: if an event struct drifts upstream, this stops compiling —
    /// a loud failure in the adapter that owns the mapping (the EVE-676
    /// lesson, now enforced by the type system instead of JSON keys).
    fn event(data: impl Into<everruns_core::EventData>) -> Event {
        Event::new(SessionId::new(), EventContext::empty(), data)
    }

    /// The stream a real two-iteration tool-calling turn produces:
    /// input → reason (thinking, usage, tool call) → tool started/completed →
    /// reason → final output message.
    fn two_iteration_stream() -> Vec<Event> {
        let meta = ModelMetadata {
            model: "llmsim-1".into(),
            model_id: None,
            provider_id: None,
        };
        vec![
            event(InputMessageData::new(everruns_core::Message::user(
                "what does GOOGL trade at?",
            ))),
            event(ReasonStartedData {
                harness_id: HarnessId::new(),
                agent_id: None,
                metadata: Some(meta.clone()),
            }),
            event(ReasonThinkingCompletedData {
                turn_id: TurnId::new(),
                thinking: "I should look the price up.".into(),
            }),
            event(ReasonCompletedData::success(
                "",
                true,
                1,
                Some(12),
                Some(TokenUsage::new(100, 20)),
            )),
            event(ToolStartedData {
                tool_call: ToolCall {
                    id: "call_1".into(),
                    name: "financial_search".into(),
                    arguments: json!({"ticker": "GOOGL"}),
                },
                tool_call_fingerprint: None,
                display_name: None,
                narration: None,
            }),
            event(ToolCompletedData::success(
                "call_1".into(),
                "financial_search".into(),
                vec![ContentPart::text("$185.35")],
                Some(3),
            )),
            event(ReasonStartedData {
                harness_id: HarnessId::new(),
                agent_id: None,
                metadata: Some(meta.clone()),
            }),
            event(ReasonCompletedData::success(
                "GOOGL trades at $185.35.",
                false,
                0,
                Some(9),
                Some(TokenUsage::new(200, 30)),
            )),
            event(
                OutputMessageCompletedData::new(everruns_core::Message::assistant(
                    "GOOGL trades at $185.35.",
                ))
                .with_metadata(meta)
                .with_usage(TokenUsage::new(200, 30)),
            ),
        ]
    }

    #[test]
    fn atif_fold_builds_one_agent_step_per_reasoning_iteration() {
        let events = two_iteration_stream();
        let t = atif_from_events(&events);

        assert_eq!(t.agent.name, "everruns-runtime");
        assert_eq!(t.agent.version, EVERRUNS_RUNTIME_VERSION);
        assert_eq!(t.agent.model_name.as_deref(), Some("llmsim-1"));
        assert!(t.session_id.is_some());

        // user + two agent iterations, 1-based ids in stream order.
        assert_eq!(t.steps.len(), 3);
        assert_eq!(
            t.steps.iter().map(|s| s.step_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(t.steps[0].source, StepSource::User);
        assert_eq!(t.steps[0].message.text(), "what does GOOGL trade at?");

        // Iteration 1: reasoning, the structured call, its observation, metrics.
        let s = &t.steps[1];
        assert_eq!(s.source, StepSource::Agent);
        assert_eq!(s.llm_call_count, Some(1));
        assert_eq!(
            s.reasoning_content.as_deref(),
            Some("I should look the price up.")
        );
        assert_eq!(s.tool_calls.len(), 1);
        assert_eq!(s.tool_calls[0].tool_call_id, "call_1");
        assert_eq!(s.tool_calls[0].function_name, "financial_search");
        assert_eq!(s.tool_calls[0].arguments, json!({"ticker": "GOOGL"}));
        let obs = &s.observation.as_ref().unwrap().results[0];
        assert_eq!(obs.source_call_id.as_deref(), Some("call_1"));
        assert_eq!(obs.content.as_ref().unwrap().text(), "$185.35");
        let m = s.metrics.as_ref().unwrap();
        assert_eq!(m.prompt_tokens, Some(100));
        assert_eq!(m.completion_tokens, Some(20));

        // Iteration 2: the full output text (not the 200-char preview) and no
        // double-billed usage from output.message.completed.
        let s = &t.steps[2];
        assert_eq!(s.message.text(), "GOOGL trades at $185.35.");
        assert_eq!(s.metrics.as_ref().unwrap().prompt_tokens, Some(200));
        assert_eq!(t.usage().input_tokens, 300);
        assert_eq!(t.usage().output_tokens, 50);

        // The projection feeds the existing name-based scorers.
        let transcript = Transcript::from_trajectory(t);
        assert_eq!(transcript.final_response, "GOOGL trades at $185.35.");
        assert_eq!(transcript.tool_calls, vec!["financial_search"]);
        assert_eq!(transcript.iterations, 2);
    }

    #[test]
    fn atif_fold_matches_extract_tool_calls_on_completed_only_streams() {
        // EVE-676 parity: a stream carrying only tool.completed events (one
        // success, one failure) still counts every call the model made, with
        // the failure's error text as the observation content.
        let events = vec![
            event(ToolCompletedData::success(
                "c1".into(),
                "read_file".into(),
                vec![ContentPart::text("contents")],
                None,
            )),
            event(ToolCompletedData::failure(
                "c2".into(),
                "search".into(),
                "error".into(),
                "boom".into(),
                None,
            )),
        ];
        let t = atif_from_events(&events);
        assert_eq!(t.tool_call_names(), vec!["read_file", "search"]);

        let json_events: Vec<serde_json::Value> = events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect();
        assert_eq!(t.tool_call_names(), extract_tool_calls(&json_events));

        // The synthesized call has unknown arguments; the failed observation
        // carries the error text and status.
        let step = &t.steps[0];
        assert_eq!(step.tool_calls[1].arguments, json!({}));
        let failed = &step.observation.as_ref().unwrap().results[1];
        assert_eq!(failed.content.as_ref().unwrap().text(), "boom");
        assert_eq!(failed.extra["status"], json!("error"));
    }

    #[test]
    fn everruns_runtime_version_stays_in_lockstep_with_the_dependency_pin() {
        // The fold stamps agent.version from this constant; make a dependency
        // bump impossible to ship without updating it.
        let workspace_manifest = include_str!("../../../Cargo.toml");
        let pin = format!("everruns-runtime = \"{EVERRUNS_RUNTIME_VERSION}\"");
        assert!(
            workspace_manifest.contains(&pin),
            "EVERRUNS_RUNTIME_VERSION ({EVERRUNS_RUNTIME_VERSION}) does not match the \
             workspace everruns-runtime pin — update the constant alongside the dependency"
        );
    }

    #[test]
    fn maps_known_providers() {
        let r = target_to_resolved(&Target::new("a", "anthropic", "claude-opus-4-8"));
        assert_eq!(r.provider_type, DriverId::Anthropic);
        assert_eq!(r.model, "claude-opus-4-8");
    }

    #[test]
    fn unknown_provider_falls_back_to_sim() {
        let r = target_to_resolved(&Target::sim());
        assert_eq!(r.provider_type, DriverId::LlmSim);
    }

    #[tokio::test]
    async fn factory_error_yields_infra_errored_transcript() {
        // A runtime that can't even be built is infrastructure, not a model
        // failure: it must be scored N/A and retried, not penalized.
        let subject = RuntimeSubject::new(|_| async { Err("nope".to_string()) });
        let cx = RunCx::new(Target::sim());
        let t = subject.run(&Sample::new("a", "hi"), &cx).await;
        assert!(!t.succeeded());
        assert!(t.errored_infra());
        assert!(t.error.unwrap().contains("nope"));
    }

    #[test]
    fn classify_runtime_error_flags_transient_faults() {
        for msg in [
            "HTTP 429 Too Many Requests",
            "rate limit exceeded",
            "insufficient_quota: you exceeded your budget",
            "anthropic: overloaded_error",
            "503 Service Unavailable",
            "connection reset by peer",
            "request timed out",
        ] {
            assert_eq!(
                classify_runtime_error(msg),
                ErrorKind::Infra,
                "expected infra for: {msg}"
            );
        }
        for msg in [
            "the assistant produced invalid JSON",
            "tool call failed: file not found",
            "max turns exceeded",
        ] {
            assert_eq!(
                classify_runtime_error(msg),
                ErrorKind::Subject,
                "expected subject for: {msg}"
            );
        }
    }
}
