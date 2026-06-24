//! Mira — a Rust-first, code-first evaluation framework for agents and tools.
//!
//! Mira is a developer tool shaped like a test runner. You define evals in Rust
//! (or any language that speaks the [protocol]), and a generic host CLI runs
//! them across a **target** matrix, scores the results, and reports.
//!
//! # The model
//!
//! ```text
//! Eval = Dataset(Sample…) + Subject + [Scorer…]  ×  target matrix
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
//! * [`Target`] — one case of the matrix. Provider-agnostic;
//!   missing API keys mark a case unavailable so it is *skipped*, not failed.
//!
//! # Two ways to run
//!
//! * **In process** — build [`Eval`]s and drive them with a [`Runner`]. Best for
//!   unit-style evals that live next to the code under test.
//! * **Over the protocol** — your program is a [`Study`]: it bundles evals and
//!   calls [`serve`](Study::serve) to expose them. The `mira` host CLI ([`Host`])
//!   compiles/spawns it, plans the run, and owns selection, the matrix,
//!   run storage, and reporting. Provider keys never cross the wire — models are
//!   addressed by *label*. See [`protocol`].
//!
//! See the crate `examples/` (`greet`, `coding`, `cli_subject`) for runnable
//! studies.

// Boxed async-closure aliases (judge, subject factories) are the idiomatic way
// to express async callbacks behind trait objects here.
#![allow(clippy::type_complexity)]
#![forbid(unsafe_code)]

pub mod aggregate;
pub mod content;
pub mod dataset;
pub mod eval;
pub mod exec;
pub mod host;
pub mod protocol;
pub mod registry;
pub mod report;
pub mod run;
pub mod runner;
pub mod scorer;
pub mod study;
pub mod subject;
pub mod target;

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
///         .sample("hi", "say hi")
///         .subject(subject_fn(|_, _| async { Transcript::response("hi there") }))
///         .scorer(contains("hi"))
///         .build()
/// }
/// ```
#[cfg(feature = "macros")]
pub use mira_macros::eval;

pub use aggregate::{TrialAggregate, aggregate_trials};
pub use content::{Message, Part, Role, Source};
pub use dataset::{Dataset, Sample};
pub use eval::Eval;
pub use exec::{Concurrency, run_cases};
pub use host::{Host, HostHandle};
pub use target::Target;
// `register_eval!` is exported at the crate root via `#[macro_export]`.
pub use registry::registered_evals;
pub use run::{RunMeta, RunSummary, new_run_id, new_run_id_at, now_unix};
pub use runner::{CaseOutcome, RunReport, Runner};
pub use scorer::Scorer;
pub use study::Study;
pub use subject::{CliSubject, Subject, subject_fn};

/// Free-form, **open-ended** metadata attached to evals, samples, targets,
/// transcripts, and runs.
///
/// Keys are arbitrary; values are arbitrary JSON ([`serde_json::Value`]) — a
/// string, number, bool, or a nested object/array — so callers can attach
/// structured context (trace URLs, dashboard deep-links, commit SHAs, dataset
/// provenance, nested provider details) without the protocol modelling each
/// shape. Carried through the protocol untouched and surfaced in reports. Use
/// [`metrics`](Transcript::metrics) instead for values you want to *compare*
/// numerically.
pub type Metadata = BTreeMap<String, serde_json::Value>;

/// Matrix-axis values for one case: axis name → chosen value.
///
/// Unlike [`Metadata`], these are always plain strings — they form part of the
/// case key ([`case_key`]) and the selection grammar, so they stay scalar and
/// stable rather than open-ended.
pub type Params = BTreeMap<String, String>;

/// One trial's reproducibility context: which repetition this case run is
/// (`index` of `count`) and the seed handed to the subject, if any.
///
/// **Trials are repetitions of the *same* logical case** — unlike an [axis], they
/// don't form new cases, they're re-runs grouped back together so the host can
/// compute pass@k, pass-rate, and score variance (see [`crate::aggregate`]).
/// A `seed` makes a trial reproducible: a subject seeds its RNG / sampling
/// temperature from it so the same `(case, seed)` replays identically.
///
/// The single, unrepeated run is [`Trial::single`] (`count == 1`); it carries no
/// trial dimension, so it adds no `#index` suffix to the case key.
///
/// [axis]: crate::eval::Axis
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trial {
    /// 0-based repetition index within this case's trials.
    pub index: usize,
    /// Total repetitions planned for this case. `1` means no trial dimension.
    pub count: usize,
    /// Per-trial seed for reproducibility, when the run set one.
    pub seed: Option<u64>,
}

impl Default for Trial {
    fn default() -> Self {
        Self::single()
    }
}

impl Trial {
    /// The single, unrepeated run: index 0 of 1, no seed.
    pub fn single() -> Self {
        Self {
            index: 0,
            count: 1,
            seed: None,
        }
    }

    /// True when this case runs more than once (the trial dimension is active).
    pub fn is_repeated(&self) -> bool {
        self.count > 1
    }

    /// The `#index` suffix this trial contributes to a case key, or empty when
    /// the case isn't repeated — so single-trial runs keep their plain keys.
    pub fn key_suffix(&self) -> String {
        trial_suffix(self.index, self.count)
    }
}

/// The `#index` key suffix for a `(trial, trials)` pair: present only when the
/// case is repeated (`trials > 1`), so a single-trial case keeps the plain
/// `eval/sample@target[…]` key. Host and study compute it identically.
pub fn trial_suffix(trial: usize, trials: usize) -> String {
    if trials > 1 {
        format!("#{trial}")
    } else {
        String::new()
    }
}

/// Render an open-ended [`Metadata`] value for display (reports, CLI): a JSON
/// string yields its raw contents (no surrounding quotes); anything else yields
/// its compact JSON form (`3`, `true`, `{"k":"v"}`).
pub fn metadata_display(value: &serde_json::Value) -> String {
    match value.as_str() {
        Some(s) => s.to_string(),
        None => value.to_string(),
    }
}

/// Token / cost accounting, summed across all turns of a run.
///
/// Beyond raw input/output tokens, `cache_read_tokens` and `reasoning_tokens`
/// capture the breakdowns modern providers report; they default to zero for
/// subjects that don't surface them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Extensible **numeric** metrics a subject measured that the core doesn't
    /// model as a typed field (recall@k, energy_joules, p95 latency, …).
    ///
    /// Design: `Usage`/`Timing` stay typed because shared budget scorers depend
    /// on their exact shape; everything else is an *open vocabulary* keyed by
    /// name so a subject can report a *new metric key* and grade it with
    /// [`metric_within`]/[`metric_at_least`] without a new protocol version (the
    /// `metrics` map itself is a versioned, additive part of the wire). Use this
    /// (not `metadata`) for anything you want to compare numerically — values
    /// stay `f64`, surface in the JSON/HTML reports, and feed generic scorers.
    ///
    /// [`metric_within`]: crate::scorer::metric_within
    /// [`metric_at_least`]: crate::scorer::metric_at_least
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, f64>,
    /// Multimodal output — the response as an ordered list of typed [`Part`]s
    /// (text, image, audio, file, structured JSON) for subjects whose result
    /// isn't plain text. `final_response` stays the canonical *text* projection
    /// (a text-only scorer keeps working); `output` carries the modalities text
    /// can't. Empty for the common text-only case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<Part>,
    /// Free-form metadata: observability links, run ids, etc.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: Metadata,
    /// Set when the subject failed to complete the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Classifies `error`: a [`Subject`](ErrorKind::Subject) failure (the model
    /// under test got it wrong — scored as a failure) vs. an
    /// [`Infra`](ErrorKind::Infra) failure (budget, rate limit, provider outage,
    /// timeout — not the model's fault). Defaulted/omitted for the common subject
    /// case; meaningless when `error` is `None`.
    #[serde(default, skip_serializing_if = "ErrorKind::is_subject")]
    pub error_kind: ErrorKind,
}

/// Why a run failed, when it did — set alongside [`Transcript::error`].
///
/// A [`Subject`](ErrorKind::Subject) error is the model/agent *under test*
/// getting it wrong: it ran but crashed on the input, produced garbage, or blew
/// its turn budget — a real failure the eval should catch. An
/// [`Infra`](ErrorKind::Infra) error is the scaffolding *around* the run breaking
/// (out of budget/quota, rate-limited, a provider 5xx/outage, a network/timeout
/// fault): not the model's fault. Infra failures are surfaced as **N/A**
/// ([`Score::na`]) so they are excluded from the case verdict and aggregate
/// (neither pass nor fail, like [`Score::na`] for a single scorer), and the host
/// retries them up to `--max-retries`. See [`Transcript::infra_error`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The subject/model under test errored: a real, scoreable failure.
    #[default]
    Subject,
    /// The infrastructure around the run errored: not the model's fault, scored
    /// N/A (not failed), and retryable.
    Infra,
}

impl ErrorKind {
    /// True for the default ([`Subject`](ErrorKind::Subject)); lets serde skip
    /// the field on the wire for the common case.
    pub fn is_subject(&self) -> bool {
        matches!(self, ErrorKind::Subject)
    }
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

    /// A failed transcript carrying an error message, attributed to the subject
    /// under test ([`ErrorKind::Subject`]) — a real, scoreable failure. For an
    /// *infrastructure* failure that should not be scored against the model, use
    /// [`Transcript::infra_error`].
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// A transcript that failed for an *infrastructure* reason ([`ErrorKind::Infra`]):
    /// budget/quota, rate limit, provider outage, network/timeout — not the
    /// model's fault. Scoring short-circuits to **N/A** so the case is excluded
    /// from pass/fail, and the host retries it.
    pub fn infra_error(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            error_kind: ErrorKind::Infra,
            ..Default::default()
        }
    }

    /// True when no error was recorded.
    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }

    /// True when this run hit an infrastructure error (see [`ErrorKind::Infra`]).
    pub fn errored_infra(&self) -> bool {
        self.error.is_some() && self.error_kind == ErrorKind::Infra
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

    /// Record a custom numeric metric. Returns `self` for builder-style use:
    /// `Transcript::response(text).with_metric("recall@5", 0.8)`.
    ///
    /// Non-finite values (`NaN`/`±inf`) are dropped rather than stored: JSON
    /// can't represent them, so storing one would break report serialization.
    /// The metric stays *unreported*, and a budget over it fails accordingly.
    pub fn with_metric(mut self, name: impl Into<String>, value: f64) -> Self {
        self.record_metric(name, value);
        self
    }

    /// Record a custom numeric metric in place (for subjects that build the
    /// transcript mutably). Non-finite values are dropped — see [`with_metric`].
    ///
    /// [`with_metric`]: Transcript::with_metric
    pub fn record_metric(&mut self, name: impl Into<String>, value: f64) {
        if value.is_finite() {
            self.metrics.insert(name.into(), value);
        }
    }

    /// Look up a custom numeric metric by name.
    pub fn metric(&self, name: &str) -> Option<f64> {
        self.metrics.get(name).copied()
    }

    /// Attach multimodal output parts, keeping `final_response` as the canonical
    /// text projection. Builder-style: `Transcript::response(text).with_output(parts)`.
    /// See [`Transcript::output`].
    pub fn with_output(mut self, parts: impl IntoIterator<Item = Part>) -> Self {
        self.output = parts.into_iter().collect();
        self
    }

    /// The distinct output modalities present (`text`, `image`, …), in first-seen
    /// order. Empty when no multimodal `output` was recorded.
    /// See [`Transcript::output`].
    pub fn output_modalities(&self) -> Vec<&'static str> {
        content::modalities(&self.output)
    }
}

/// Outcome of a single [`Scorer`] on a [`Transcript`].
///
/// `value` is a continuous score in `0.0..=1.0`; `pass` is the boolean verdict
/// (often `value >= threshold`). Keeping both lets a scorer report a graded
/// signal while still contributing a pass/fail to the matrix.
///
/// A third state — **N/A** ([`na`](Score::na)) — lets a scorer say "I couldn't
/// evaluate this" (an unreachable judge, a missing API key, any infra hiccup)
/// rather than crashing the run or lying with a `fail`. An N/A score is excluded
/// from the case verdict and the aggregate: it neither passes nor fails.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Score {
    pub scorer: String,
    pub value: f64,
    pub pass: bool,
    /// True when the scorer did not apply / could not run (infra issue, missing
    /// credentials, …). Excluded from the case verdict and aggregate.
    #[serde(default, skip_serializing_if = "is_false")]
    pub na: bool,
    pub reason: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Score {
    /// A passing score (`value = 1.0`).
    pub fn pass(scorer: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            scorer: scorer.into(),
            value: 1.0,
            pass: true,
            na: false,
            reason: reason.into(),
        }
    }

    /// A failing score (`value = 0.0`).
    pub fn fail(scorer: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            scorer: scorer.into(),
            value: 0.0,
            pass: false,
            na: false,
            reason: reason.into(),
        }
    }

    /// A not-applicable score: the scorer could not be evaluated (e.g. the judge
    /// model was unreachable or unconfigured). Counts as neither pass nor fail —
    /// the case verdict and aggregate ignore it. This is the sanctioned way to
    /// handle infra failures: return N/A instead of crashing or failing.
    pub fn na(scorer: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            scorer: scorer.into(),
            value: 0.0,
            pass: false,
            na: true,
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
            na: false,
            reason: reason.into(),
        }
    }

    /// True when this score did not apply (see [`Score::na`]).
    pub fn is_na(&self) -> bool {
        self.na
    }
}

/// Per-run context handed to a [`Subject`]: which target to use
/// for this matrix case, and the run limits.
#[derive(Clone, Debug)]
pub struct RunCx {
    /// The matrix case's target (the model or harness under evaluation).
    pub target: Target,
    /// Maximum reasoning iterations a subject should take.
    pub max_turns: usize,
    /// Values for any extra matrix axes this case varies (axis name → value),
    /// e.g. `{"effort": "high"}`. Empty for a target-only matrix. A subject reads
    /// these to vary its behaviour per case.
    pub params: Params,
    /// This run's trial within its case: which repetition (`index` of `count`)
    /// and the optional seed. A stochastic subject seeds its RNG / sampling from
    /// [`Trial::seed`] so the run is reproducible. [`Trial::single`] for an
    /// unrepeated case.
    pub trial: Trial,
    /// The conversation so far, for an **interactive** (multi-turn) eval: the
    /// alternating `User`/`Assistant` [`Message`]s leading up to this call, with
    /// the latest `User` turn last. Empty on the first call and for single-shot
    /// evals (the subject reads the [`Sample`] directly then). A multi-turn-aware
    /// subject reconstructs its context from this each call (it is invoked once
    /// per turn). Populated by the interactive driver; see [`Eval::responder`].
    ///
    /// [`Eval::responder`]: crate::eval::EvalBuilder::responder
    pub conversation: Vec<Message>,
}

impl RunCx {
    /// A context for `target` with default limits, no extra axis params, a single
    /// (unrepeated, unseeded) trial, and an empty conversation.
    pub fn new(target: Target) -> Self {
        Self {
            target,
            max_turns: 12,
            params: Params::new(),
            trial: Trial::single(),
            conversation: Vec::new(),
        }
    }

    /// The value of an extra matrix axis for this case, if set.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }

    /// This run's seed, if the host set one (a convenience for
    /// `self.trial.seed`). Seed a subject's RNG / sampling from this for
    /// reproducible trials.
    pub fn seed(&self) -> Option<u64> {
        self.trial.seed
    }
}

/// The canonical, stable identity of one matrix case: `eval/sample@target`,
/// suffixed with `[k=v,…]` (axis params sorted by key) when extra axes vary.
/// Used for selection, dedupe, checkpoint resume, and reporting — host and
/// study compute it identically. `target` is the target label.
pub fn case_key(eval: &str, sample: &str, target: &str, params: &Params) -> String {
    let base = format!("{eval}/{sample}@{target}");
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
/// case instead of failing it (see [`exec`]).
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
    fn na_score_is_neither_pass_nor_fail() {
        let s = Score::na("judge", "model unreachable");
        assert!(s.is_na());
        assert!(!s.pass);
        // N/A is carried through serialization so consumers can distinguish it.
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"na\":true"));
        // A normal score omits the flag.
        let p = Score::pass("s", "ok");
        assert!(!serde_json::to_string(&p).unwrap().contains("na"));
    }

    #[test]
    fn transcript_helpers() {
        assert!(Transcript::response("hi").succeeded());
        assert!(!Transcript::failed("boom").succeeded());
    }

    #[test]
    fn infra_error_is_distinct_from_subject_error() {
        let infra = Transcript::infra_error("budget exhausted");
        assert!(!infra.succeeded());
        assert!(infra.errored_infra());
        assert_eq!(infra.error_kind, ErrorKind::Infra);

        let subject = Transcript::failed("wrong answer");
        assert!(!subject.succeeded());
        assert!(!subject.errored_infra()); // a real failure, not infra
        assert_eq!(subject.error_kind, ErrorKind::Subject);

        assert!(!Transcript::response("ok").errored_infra());

        // Subject (default) kind is omitted on the wire; Infra is serialized.
        let subj = serde_json::to_string(&Transcript::failed("x")).unwrap();
        assert!(!subj.contains("error_kind"));
        let inf = serde_json::to_string(&Transcript::infra_error("x")).unwrap();
        assert!(inf.contains("\"error_kind\":\"infra\""));
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

    #[test]
    fn custom_metrics_round_trip_and_reject_non_finite() {
        let t = Transcript::response("ok")
            .with_metric("recall@5", 0.8)
            .with_metric("nan", f64::NAN)
            .with_metric("inf", f64::INFINITY);
        // Finite values stored; non-finite dropped (so they stay "unreported").
        assert_eq!(t.metric("recall@5"), Some(0.8));
        assert_eq!(t.metric("nan"), None);
        assert_eq!(t.metric("inf"), None);
        // What we kept must serialize as JSON (non-finite floats would error).
        serde_json::to_string(&t).expect("transcript with metrics serializes");
    }

    #[test]
    fn multimodal_output_rides_alongside_text() {
        let t = Transcript::response("a cat on a mat").with_output([
            Part::text("a cat on a mat"),
            Part::image_uri("image/png", "https://x/cat.png"),
        ]);
        // final_response stays the canonical text; output carries the modalities.
        assert_eq!(t.final_response, "a cat on a mat");
        assert_eq!(t.output_modalities(), vec!["text", "image"]);
        // Round-trips on the committed wire.
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""kind":"image""#));
        let back: Transcript = serde_json::from_str(&json).unwrap();
        assert_eq!(back.output, t.output);
    }

    #[test]
    fn metadata_is_open_ended_and_round_trips() {
        let mut t = Transcript::response("ok");
        // Open-ended values: a string, a number, and a nested object.
        t.metadata.insert("trace".into(), "https://obs/123".into());
        t.metadata.insert("attempt".into(), 3.into());
        t.metadata.insert(
            "ctx".into(),
            serde_json::json!({ "shard": 2, "warm": true }),
        );

        let json = serde_json::to_string(&t).unwrap();
        let back: Transcript = serde_json::from_str(&json).unwrap();
        assert_eq!(back.metadata["attempt"], serde_json::json!(3));
        assert_eq!(back.metadata["ctx"]["shard"], serde_json::json!(2));

        // Display: strings render bare; structured values render as compact JSON.
        assert_eq!(metadata_display(&back.metadata["trace"]), "https://obs/123");
        assert_eq!(metadata_display(&back.metadata["attempt"]), "3");
        assert_eq!(
            metadata_display(&back.metadata["ctx"]),
            r#"{"shard":2,"warm":true}"#
        );
    }
}
