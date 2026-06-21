//! Mira — a Rust-first, code-first evaluation framework for agents and tools.
//!
//! Mira is a developer tool shaped like a test runner. You define evals in Rust
//! (or any language that speaks the [protocol]), and a generic host CLI runs
//! them across a model matrix, scores the results, and reports.
//!
//! # The model
//!
//! ```text
//! Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  model matrix
//! ```
//!
//! * [`Sample`] — one dataset row: input turns, an optional `target`, seeded
//!   `files`, `tags`, and free-form `metadata`.
//! * [`Subject`] — the thing under evaluation. One adapter per
//!   *shape*: an in-process closure ([`subject_fn`]), an
//!   external binary ([`CliSubject`], the polyglot path),
//!   or a custom integration such as `mira-everruns`'s `RuntimeSubject`.
//! * [`Transcript`] — the normalized result every subject produces, so scorers
//!   and reporting are shared.
//! * [`Scorer`] — grades a [`Transcript`] into a [`Score`].
//!   Deterministic built-ins, an arbitrary-closure escape hatch, and
//!   LLM-as-judge ([`model_graded`](scorer::model_graded)) compose freely.
//! * [`ModelSpec`] — one cell of the matrix. Provider-agnostic;
//!   missing API keys mark a cell unavailable so it is *skipped*, not failed.
//!
//! # Two ways to run
//!
//! * **In process** — build [`Eval`]s and drive them with a [`Runner`]. Best for
//!   unit-style evals that live next to the code under test.
//! * **Over the protocol** — your program is a [`Study`]: it bundles evals and
//!   calls [`serve`](Study::serve) to expose them. The `mira` host CLI ([`Host`])
//!   compiles/spawns it, plans the run, and owns selection, the matrix,
//!   checkpoints, and reporting. Provider keys never cross the wire — models are
//!   addressed by *label*. See [`protocol`].
//!
//! See the crate `examples/` (`greet`, `coding`, `cli_subject`) for runnable
//! studies.

// Boxed async-closure aliases (judge, subject factories) are the idiomatic way
// to express async callbacks behind trait objects here.
#![allow(clippy::type_complexity)]
#![forbid(unsafe_code)]

pub mod dataset;
pub mod eval;
pub mod exec;
pub mod host;
pub mod model;
pub mod protocol;
pub mod registry;
pub mod report;
pub mod runner;
pub mod scorer;
pub mod session;
pub mod study;
pub mod subject;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// Re-exported so the `register_eval!` macro can reference `$crate::inventory`
// without users taking a direct dependency on it.
#[doc(hidden)]
pub use inventory;

/// The `#[eval]` attribute: registers a `fn() -> Eval` factory for
/// `cargo test`-style discovery (the ergonomic form of [`register_eval!`]).
///
/// ```
/// use mira::{eval, Eval, Transcript};
/// use mira::subject::subject_fn;
/// use mira::scorer::contains;
///
/// #[eval]
/// fn greet() -> Eval {
///     Eval::new("greet")
///         .case("hi", "say hi")
///         .subject(subject_fn(|_, _| async { Transcript::response("hi there") }))
///         .scorer(contains("hi"))
///         .build()
/// }
/// ```
#[cfg(feature = "macros")]
pub use mira_macros::eval;

pub use dataset::{Dataset, Sample};
pub use eval::{Case, Eval};
pub use exec::{Concurrency, run_cells};
pub use host::{Host, HostHandle};
pub use model::ModelSpec;
// `register_eval!` is exported at the crate root via `#[macro_export]`.
pub use registry::registered_evals;
pub use runner::{CaseOutcome, RunReport, Runner};
pub use scorer::Scorer;
pub use session::Session;
pub use study::Study;
pub use subject::{CliSubject, Subject, subject_fn};

/// Free-form key/value metadata attached to evals, samples, models, and runs.
///
/// This is where observability links (trace URLs, dashboard deep-links), commit
/// SHAs, dataset provenance, and any other context live. It is carried through
/// the protocol and surfaces in reports.
pub type Metadata = BTreeMap<String, String>;

/// Token / cost accounting, summed across all turns of a run.
///
/// Beyond raw input/output tokens, `cache_read_tokens` and `reasoning_tokens`
/// capture the breakdowns modern providers report; they default to zero for
/// subjects that don't surface them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Prompt tokens served from cache (a subset of `input_tokens` for providers
    /// that bill them separately). Zero when not reported.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cache_read_tokens: u64,
    /// Reasoning / thinking tokens (a subset of `output_tokens`). Zero when not
    /// reported.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl Usage {
    /// Total tokens (input + output).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Accumulate another usage record into this one.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cost_usd += other.cost_usd;
    }
}

/// Wall-clock timing for a run. Subjects that can measure it populate these;
/// the rest leave them at their defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    /// Total wall-clock duration of the run, in milliseconds.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub duration_ms: u64,
    /// Time from run start to the first streamed token/event, in milliseconds,
    /// when the subject can measure it (latency a user perceives first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u64>,
}

impl Timing {
    /// True when no timing was recorded (all fields at their defaults).
    pub fn is_default(&self) -> bool {
        *self == Timing::default()
    }
}

/// Normalized result of running a [`Subject`] on one
/// [`Sample`].
///
/// Every subject — in-process, CLI, or a custom integration — produces this same
/// shape, so scorers and reporting never depend on a subject's internals. The
/// `events` field carries the raw transcript (e.g. everruns' canonical JSONL
/// `Event`s) for structural scorers to search.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Transcript {
    /// The subject's final response text.
    pub final_response: String,
    /// Reasoning iterations / turns taken.
    pub iterations: usize,
    /// Number of tool calls made.
    pub tool_calls_count: usize,
    /// Token / cost usage.
    pub usage: Usage,
    /// Wall-clock timing (duration, time-to-first-token).
    #[serde(default, skip_serializing_if = "Timing::is_default")]
    pub timing: Timing,
    /// Best-effort list of tool names invoked, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<String>,
    /// Files present in the subject's workspace after the run (path → contents).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
    /// Raw serialized events (e.g. the everruns `Event` JSONL transcript).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<serde_json::Value>,
    /// Free-form metadata: observability links, run ids, etc.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: Metadata,
    /// Set when the subject failed to complete the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Transcript {
    /// A transcript whose only content is a final response. Convenience for
    /// simple subjects and tests.
    pub fn response(text: impl Into<String>) -> Self {
        Self {
            final_response: text.into(),
            ..Default::default()
        }
    }

    /// A failed transcript carrying an error message.
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// True when no error was recorded.
    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }

    /// Distinct tool names invoked, in first-seen order. `tool_calls` keeps every
    /// invocation (with repeats); this collapses to the unique set used.
    pub fn tools_used(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for name in &self.tool_calls {
            if !seen.contains(name) {
                seen.push(name.clone());
            }
        }
        seen
    }

    /// Record wall-clock duration. Returns `self` for builder-style use in
    /// subjects and tests.
    pub fn with_duration_ms(mut self, ms: u64) -> Self {
        self.timing.duration_ms = ms;
        self
    }
}

/// Outcome of a single [`Scorer`] on a [`Transcript`].
///
/// `value` is a continuous score in `0.0..=1.0`; `pass` is the boolean verdict
/// (often `value >= threshold`). Keeping both lets a scorer report a graded
/// signal while still contributing a pass/fail to the matrix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub scorer: String,
    pub value: f64,
    pub pass: bool,
    pub reason: String,
}

impl Score {
    /// A passing score (`value = 1.0`).
    pub fn pass(scorer: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            scorer: scorer.into(),
            value: 1.0,
            pass: true,
            reason: reason.into(),
        }
    }

    /// A failing score (`value = 0.0`).
    pub fn fail(scorer: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            scorer: scorer.into(),
            value: 0.0,
            pass: false,
            reason: reason.into(),
        }
    }

    /// A graded score in `0.0..=1.0`; `pass` is `value >= threshold`.
    pub fn graded(
        scorer: impl Into<String>,
        value: f64,
        threshold: f64,
        reason: impl Into<String>,
    ) -> Self {
        let value = value.clamp(0.0, 1.0);
        Self {
            scorer: scorer.into(),
            value,
            pass: value >= threshold,
            reason: reason.into(),
        }
    }
}

/// Per-run context handed to a [`Subject`]: which model to use
/// for this matrix cell, and the run limits.
#[derive(Clone, Debug)]
pub struct RunCx {
    /// The matrix cell's model.
    pub model: ModelSpec,
    /// Maximum reasoning iterations a subject should take.
    pub max_turns: usize,
    /// Values for any extra matrix axes this cell varies (axis name → value),
    /// e.g. `{"effort": "high"}`. Empty for a model-only matrix. A subject reads
    /// these to vary its behaviour per cell.
    pub params: Metadata,
}

impl RunCx {
    /// A context for `model` with default limits and no extra axis params.
    pub fn new(model: ModelSpec) -> Self {
        Self {
            model,
            max_turns: 12,
            params: Metadata::new(),
        }
    }

    /// The value of an extra matrix axis for this cell, if set.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }
}

/// The canonical, stable identity of one matrix cell: `eval/sample@model`,
/// suffixed with `[k=v,…]` (axis params sorted by key) when extra axes vary.
/// Used for selection, dedupe, checkpoint resume, and reporting — host and
/// study compute it identically.
pub fn cell_key(eval: &str, sample: &str, model: &str, params: &Metadata) -> String {
    let base = format!("{eval}/{sample}@{model}");
    if params.is_empty() {
        return base;
    }
    // BTreeMap iterates sorted by key, so the suffix is deterministic.
    let suffix = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{base}[{suffix}]")
}

/// Heuristic: does this error message look like a provider rate-limit / quota /
/// overload signal? The core is provider-agnostic, so detection is a substring
/// match over the common phrasings (HTTP 429, "rate limit", "overloaded",
/// "quota", …). The host's adaptive scheduler uses this to back off and retry a
/// cell instead of failing it (see [`exec`]).
pub fn is_rate_limited(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("429")
        || m.contains("rate limit")
        || m.contains("rate-limit")
        || m.contains("ratelimit")
        || m.contains("too many requests")
        || m.contains("overloaded")
        || m.contains("quota")
        || m.contains("try again later")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_accumulates() {
        let mut a = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: 0.1,
            ..Default::default()
        };
        a.add(&Usage {
            input_tokens: 1,
            output_tokens: 2,
            reasoning_tokens: 4,
            cost_usd: 0.01,
            ..Default::default()
        });
        assert_eq!(a.input_tokens, 11);
        assert_eq!(a.total_tokens(), 18);
        assert_eq!(a.reasoning_tokens, 4);
        assert!((a.cost_usd - 0.11).abs() < 1e-9);
    }

    #[test]
    fn score_graded_respects_threshold() {
        let s = Score::graded("s", 0.8, 0.7, "ok");
        assert!(s.pass);
        let s = Score::graded("s", 0.6, 0.7, "low");
        assert!(!s.pass);
        // Out-of-range values clamp.
        assert_eq!(Score::graded("s", 2.0, 0.7, "").value, 1.0);
    }

    #[test]
    fn transcript_helpers() {
        assert!(Transcript::response("hi").succeeded());
        assert!(!Transcript::failed("boom").succeeded());
    }

    #[test]
    fn detects_rate_limit_signals() {
        assert!(is_rate_limited("HTTP 429 Too Many Requests"));
        assert!(is_rate_limited("anthropic: overloaded_error"));
        assert!(is_rate_limited("Rate limit exceeded, try again later"));
        assert!(is_rate_limited("insufficient_quota"));
        assert!(!is_rate_limited("invalid api key"));
        assert!(!is_rate_limited("connection refused"));
    }
}
