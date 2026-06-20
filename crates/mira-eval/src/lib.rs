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
//! * **Over the protocol** — your program calls [`serve`] to expose its evals;
//!   the `mira` host CLI ([`Host`]) compiles/spawns it, plans the run, and owns
//!   selection, the matrix, checkpoints, and reporting. Provider keys never
//!   cross the wire — models are addressed by *label*. See [`protocol`].
//!
//! See the crate `examples/` (`greet`, `coding`, `cli_subject`) for runnable
//! eval servers.

// Boxed async-closure aliases (judge, subject factories) are the idiomatic way
// to express async callbacks behind trait objects here.
#![allow(clippy::type_complexity)]
#![forbid(unsafe_code)]

pub mod dataset;
pub mod eval;
pub mod host;
pub mod model;
pub mod protocol;
pub mod registry;
pub mod report;
pub mod runner;
pub mod scorer;
pub mod server;
pub mod subject;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// Re-exported so the `register_eval!` macro can reference `$crate::inventory`
// without users taking a direct dependency on it.
#[doc(hidden)]
pub use inventory;

pub use dataset::{Dataset, Sample};
pub use eval::{Case, Eval};
pub use host::Host;
pub use model::ModelSpec;
// `register_eval!` is exported at the crate root via `#[macro_export]`.
pub use registry::registered_evals;
pub use runner::{CaseOutcome, RunReport, Runner};
pub use scorer::Scorer;
pub use server::{serve, serve_registered};
pub use subject::{CliSubject, Subject, subject_fn};

/// Free-form key/value metadata attached to evals, samples, models, and runs.
///
/// This is where observability links (trace URLs, dashboard deep-links), commit
/// SHAs, dataset provenance, and any other context live. It is carried through
/// the protocol and surfaces in reports.
pub type Metadata = BTreeMap<String, String>;

/// Token / cost accounting, summed across all turns of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
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
        self.cost_usd += other.cost_usd;
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
        };
        a.add(&Usage {
            input_tokens: 1,
            output_tokens: 2,
            cost_usd: 0.01,
        });
        assert_eq!(a.input_tokens, 11);
        assert_eq!(a.total_tokens(), 18);
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
}
