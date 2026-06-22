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
//! // The embedder builds a runtime for each matrix cell (provider/model from
//! // the Target) and returns it with the session id to drive.
//! let subject = RuntimeSubject::new(|model| Box::pin(async move {
//!     // ... construct an InProcessRuntime registering a driver for
//!     // `target.provider` / `target.model`, then return (runtime, session_id).
//!     # let _ = model;
//!     # Err::<(InProcessRuntime, SessionId), String>("wire me up".into())
//! }));
//!
//! let _eval = Eval::new("greet")
//!     .case("hi", "Say hi and tell me the answer.")
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
use everruns_core::{InputMessage, ResolvedModel};
use everruns_runtime::InProcessRuntime;

use mira::subject::summarize_events;
use mira::{ErrorKind, RunCx, Sample, Subject, Target, Transcript};

/// A built runtime plus the session to drive for one matrix cell.
pub type RuntimeHandle = (InProcessRuntime, SessionId);

/// Builds a fresh runtime for a given matrix cell. The embedder owns
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
                    transcript.tool_calls_count += result.tool_calls_count;
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
            for event in &events {
                if let Ok(value) = serde_json::to_value(event) {
                    transcript.events.push(value);
                }
            }
        }
        let (usage, tools) = summarize_events(&transcript.events);
        transcript.usage = usage;
        transcript.tool_calls = tools;
        transcript.tool_calls_count = transcript.tool_calls.len();
        transcript.timing.duration_ms = started.elapsed().as_millis() as u64;
        transcript
    }
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
