//! [`Scorer`]s grade a [`Transcript`]. They compose freely: an [`Eval`] holds a
//! `Vec<Box<dyn Scorer>>` and every scorer runs against every transcript. This
//! is one open, code-first vocabulary — deterministic built-ins, an
//! arbitrary-closure escape hatch, and LLM-as-judge — rather than a closed enum.
//!
//! # Source of truth for cross-SDK parity
//!
//! These deterministic built-ins are **canonical**. Each non-Rust SDK
//! (`sdks/python/mira/scorers.py`, `sdks/typescript/src/scorers.ts`) carries a
//! hand-written mirror — scoring runs study-side, so the logic can't be shared,
//! only kept in lock-step. Behaviour is pinned by the shared vectors in
//! `schema/v1/conformance/scorers.json` (this crate's `tests/scorer_parity.rs`
//! is the oracle that proves the vectors match Rust; each SDK runs the same
//! vectors against its mirror). Changing or adding a deterministic scorer here
//! means: update the vectors, mirror it in every SDK, and extend the `KINDS`
//! list in `tests/scorer_parity.rs` (which fails until a vector exists). See
//! `specs/sdks.md` for the full maintenance rule. The closure [`scorer`] and
//! the LLM-judge [`model_graded`] are intentionally *not* mirrored — neither is
//! a deterministic, language-portable spec.
//!
//! [`Eval`]: crate::eval::Eval

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::{Sample, Score, Transcript};

/// Grades a [`Transcript`] for one [`Sample`] into a [`Score`].
#[async_trait]
pub trait Scorer: Send + Sync {
    /// A stable, human-readable name (shown in reports).
    fn name(&self) -> String;
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score;
}

// ----- closure scorer -------------------------------------------------------

/// Wraps a plain closure as a scorer — the escape hatch for one-off checks. The
/// bar for bespoke logic is a closure, not a new type.
pub struct FnScorer {
    name: String,
    f: Box<dyn Fn(&Sample, &Transcript) -> Score + Send + Sync>,
}

#[async_trait]
impl Scorer for FnScorer {
    fn name(&self) -> String {
        self.name.clone()
    }
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score {
        (self.f)(sample, transcript)
    }
}

/// Build a scorer from a closure: `scorer("nonempty", |_, t| ...)`.
pub fn scorer(
    name: impl Into<String>,
    f: impl Fn(&Sample, &Transcript) -> Score + Send + Sync + 'static,
) -> Box<dyn Scorer> {
    Box::new(FnScorer {
        name: name.into(),
        f: Box::new(f),
    })
}

// ----- deterministic built-ins ----------------------------------------------

/// Passes if the final response contains `needle`.
pub fn contains(needle: impl Into<String>) -> Box<dyn Scorer> {
    let needle = needle.into();
    scorer(format!("contains({needle:?})"), move |_, t| {
        if t.final_response.contains(&needle) {
            Score::pass("contains", format!("found {needle:?}"))
        } else {
            Score::fail("contains", format!("missing {needle:?}"))
        }
    })
}

/// Passes if the final response does NOT contain `needle`.
pub fn not_contains(needle: impl Into<String>) -> Box<dyn Scorer> {
    let needle = needle.into();
    scorer(format!("not_contains({needle:?})"), move |_, t| {
        if t.final_response.contains(&needle) {
            Score::fail("not_contains", format!("unexpectedly found {needle:?}"))
        } else {
            Score::pass("not_contains", format!("absent {needle:?}"))
        }
    })
}

/// Passes if the final response (case-insensitively) equals `expected`.
pub fn equals(expected: impl Into<String>) -> Box<dyn Scorer> {
    let expected = expected.into();
    scorer(format!("equals({expected:?})"), move |_, t| {
        if t.final_response
            .trim()
            .eq_ignore_ascii_case(expected.trim())
        {
            Score::pass("equals", "exact match")
        } else {
            Score::fail(
                "equals",
                format!("expected {expected:?}, got {:?}", t.final_response),
            )
        }
    })
}

/// Passes if the final response matches the regex `pattern`.
pub fn regex(pattern: impl Into<String>) -> Box<dyn Scorer> {
    let pattern = pattern.into();
    let compiled = regex::Regex::new(&pattern);
    scorer(format!("regex({pattern:?})"), move |_, t| match &compiled {
        Ok(re) if re.is_match(&t.final_response) => {
            Score::pass("regex", format!("matched {pattern:?}"))
        }
        Ok(_) => Score::fail("regex", format!("no match for {pattern:?}")),
        Err(e) => Score::fail("regex", format!("bad pattern: {e}")),
    })
}

/// Passes if the final response matches the sample's string `expected` answer.
/// Samples without a string expected value fail (the eval is misconfigured).
pub fn matches_expected() -> Box<dyn Scorer> {
    scorer("matches_expected", move |s, t| match s.expected_str() {
        Some(expected) if t.final_response.trim() == expected.trim() => {
            Score::pass("matches_expected", "matched expected")
        }
        Some(expected) => Score::fail("matches_expected", format!("expected {expected:?}")),
        None => Score::fail("matches_expected", "sample has no string expected answer"),
    })
}

/// Passes if a file at `path` exists in the subject's captured workspace, and
/// (optionally) contains `needle`.
pub fn file_contains(path: impl Into<String>, needle: impl Into<String>) -> Box<dyn Scorer> {
    let path = path.into();
    let needle = needle.into();
    scorer(
        format!("file_contains({path}, {needle:?})"),
        move |_, t| match t.files.get(&path) {
            Some(contents) if contents.contains(&needle) => {
                Score::pass("file_contains", format!("{path} contains {needle:?}"))
            }
            Some(_) => Score::fail("file_contains", format!("{path} missing {needle:?}")),
            None => Score::fail("file_contains", format!("no such file: {path}")),
        },
    )
}

/// Passes if a file at `path` exists in the subject's captured workspace.
pub fn file_exists(path: impl Into<String>) -> Box<dyn Scorer> {
    let path = path.into();
    scorer(format!("file_exists({path})"), move |_, t| {
        if t.files.contains_key(&path) {
            Score::pass("file_exists", format!("{path} exists"))
        } else {
            Score::fail("file_exists", format!("no such file: {path}"))
        }
    })
}

/// Passes if a tool named `tool` was invoked at least once.
pub fn tool_called(tool: impl Into<String>) -> Box<dyn Scorer> {
    let tool = tool.into();
    scorer(format!("tool_called({tool})"), move |_, t| {
        if t.tool_calls.iter().any(|n| n == &tool) {
            Score::pass("tool_called", format!("{tool} was called"))
        } else {
            Score::fail("tool_called", format!("{tool} never called"))
        }
    })
}

/// Passes if a tool named `tool` was never invoked.
pub fn tool_not_called(tool: impl Into<String>) -> Box<dyn Scorer> {
    let tool = tool.into();
    scorer(format!("tool_not_called({tool})"), move |_, t| {
        if t.tool_calls.iter().any(|n| n == &tool) {
            Score::fail("tool_not_called", format!("{tool} was called"))
        } else {
            Score::pass("tool_not_called", format!("{tool} never called"))
        }
    })
}

/// Passes if the run used no more than `max` tool calls.
pub fn tool_calls_within(max: usize) -> Box<dyn Scorer> {
    scorer(format!("tool_calls_within({max})"), move |_, t| {
        if t.tool_calls_count <= max {
            Score::pass(
                "tool_calls_within",
                format!("{} <= {max}", t.tool_calls_count),
            )
        } else {
            Score::fail(
                "tool_calls_within",
                format!("{} > {max}", t.tool_calls_count),
            )
        }
    })
}

/// Passes if the run took no more than `max` reasoning iterations.
pub fn turns_within(max: usize) -> Box<dyn Scorer> {
    scorer(format!("turns_within({max})"), move |_, t| {
        if t.iterations <= max {
            Score::pass("turns_within", format!("{} <= {max}", t.iterations))
        } else {
            Score::fail("turns_within", format!("{} > {max}", t.iterations))
        }
    })
}

/// Passes if the run stayed at or under `max_usd` total cost.
pub fn cost_within(max_usd: f64) -> Box<dyn Scorer> {
    scorer(format!("cost_within(${max_usd})"), move |_, t| {
        if t.usage.cost_usd <= max_usd {
            Score::pass(
                "cost_within",
                format!("${:.4} <= ${max_usd}", t.usage.cost_usd),
            )
        } else {
            Score::fail(
                "cost_within",
                format!("${:.4} > ${max_usd}", t.usage.cost_usd),
            )
        }
    })
}

/// Passes if the subject completed without an error.
pub fn succeeded() -> Box<dyn Scorer> {
    scorer("succeeded", move |_, t| match &t.error {
        None => Score::pass("succeeded", "no error"),
        Some(e) => Score::fail("succeeded", format!("error: {e}")),
    })
}

/// Passes if the final response is non-empty after trimming.
pub fn non_empty() -> Box<dyn Scorer> {
    scorer("non_empty", move |_, t| {
        if t.final_response.trim().is_empty() {
            Score::fail("non_empty", "empty response")
        } else {
            Score::pass("non_empty", "non-empty response")
        }
    })
}

// ----- token & cost budgets -------------------------------------------------

/// Passes if total tokens (input + output) stayed at or under `max`.
pub fn tokens_within(max: u64) -> Box<dyn Scorer> {
    scorer(format!("tokens_within({max})"), move |_, t| {
        let total = t.usage.total_tokens();
        if total <= max {
            Score::pass("tokens_within", format!("{total} <= {max} tokens"))
        } else {
            Score::fail("tokens_within", format!("{total} > {max} tokens"))
        }
    })
}

/// Passes if output (completion) tokens stayed at or under `max`.
pub fn output_tokens_within(max: u64) -> Box<dyn Scorer> {
    scorer(format!("output_tokens_within({max})"), move |_, t| {
        let out = t.usage.output_tokens;
        if out <= max {
            Score::pass("output_tokens_within", format!("{out} <= {max}"))
        } else {
            Score::fail("output_tokens_within", format!("{out} > {max}"))
        }
    })
}

// ----- latency budgets ------------------------------------------------------

/// Passes if the run's wall-clock duration stayed at or under `max_ms`.
pub fn latency_within(max_ms: u64) -> Box<dyn Scorer> {
    scorer(format!("latency_within({max_ms}ms)"), move |_, t| {
        let ms = t.timing.duration_ms;
        if ms <= max_ms {
            Score::pass("latency_within", format!("{ms}ms <= {max_ms}ms"))
        } else {
            Score::fail("latency_within", format!("{ms}ms > {max_ms}ms"))
        }
    })
}

/// Passes if time-to-first-token stayed at or under `max_ms`. Samples whose
/// subject did not measure TTFT fail (the budget can't be verified).
pub fn ttft_within(max_ms: u64) -> Box<dyn Scorer> {
    scorer(format!("ttft_within({max_ms}ms)"), move |_, t| {
        match t.timing.time_to_first_token_ms {
            Some(ms) if ms <= max_ms => {
                Score::pass("ttft_within", format!("ttft {ms}ms <= {max_ms}ms"))
            }
            Some(ms) => Score::fail("ttft_within", format!("ttft {ms}ms > {max_ms}ms")),
            None => Score::fail("ttft_within", "subject did not report TTFT"),
        }
    })
}

// ----- custom (open-vocabulary) metrics -------------------------------------

/// Passes if the subject's custom metric `name` is at or below `max`. The
/// generic budget check for [`Transcript::metrics`] — the same shape as
/// `tokens_within`/`cost_within`, but for any metric a subject reports. A
/// transcript that never recorded `name` fails (the budget can't be verified).
///
/// [`Transcript::metrics`]: crate::Transcript::metrics
pub fn metric_within(name: impl Into<String>, max: f64) -> Box<dyn Scorer> {
    let name = name.into();
    scorer(
        format!("metric_within({name} <= {max})"),
        move |_, t| match t.metric(&name) {
            Some(v) if v <= max => Score::pass("metric_within", format!("{name}={v} <= {max}")),
            Some(v) => Score::fail("metric_within", format!("{name}={v} > {max}")),
            None => Score::fail("metric_within", format!("subject did not report {name}")),
        },
    )
}

/// Passes if the subject's custom metric `name` is at or above `min` — for
/// metrics where higher is better (recall, coverage, …). A transcript that
/// never recorded `name` fails.
///
/// [`Transcript::metrics`]: crate::Transcript::metrics
pub fn metric_at_least(name: impl Into<String>, min: f64) -> Box<dyn Scorer> {
    let name = name.into();
    scorer(
        format!("metric_at_least({name} >= {min})"),
        move |_, t| match t.metric(&name) {
            Some(v) if v >= min => Score::pass("metric_at_least", format!("{name}={v} >= {min}")),
            Some(v) => Score::fail("metric_at_least", format!("{name}={v} < {min}")),
            None => Score::fail("metric_at_least", format!("subject did not report {name}")),
        },
    )
}

// ----- richer tool checks ---------------------------------------------------

/// Passes if exactly the given set of tools was used (order-independent, repeats
/// ignored) — neither missing nor extra tools.
pub fn tools_used_exactly(tools: impl IntoIterator<Item = impl Into<String>>) -> Box<dyn Scorer> {
    let mut expected: Vec<String> = tools.into_iter().map(Into::into).collect();
    expected.sort();
    expected.dedup();
    let label = expected.join(",");
    scorer(format!("tools_used_exactly([{label}])"), move |_, t| {
        let mut used = t.tools_used();
        used.sort();
        if used == expected {
            Score::pass("tools_used_exactly", format!("used exactly [{label}]"))
        } else {
            Score::fail(
                "tools_used_exactly",
                format!("expected [{label}], used [{}]", used.join(",")),
            )
        }
    })
}

/// Passes if tool `first` was invoked before tool `second` (both must appear).
pub fn tool_called_before(first: impl Into<String>, second: impl Into<String>) -> Box<dyn Scorer> {
    let first = first.into();
    let second = second.into();
    scorer(
        format!("tool_called_before({first}, {second})"),
        move |_, t| {
            let fi = t.tool_calls.iter().position(|n| n == &first);
            let si = t.tool_calls.iter().position(|n| n == &second);
            match (fi, si) {
                (Some(f), Some(s)) if f < s => {
                    Score::pass("tool_called_before", format!("{first} before {second}"))
                }
                (Some(_), Some(_)) => {
                    Score::fail("tool_called_before", format!("{first} not before {second}"))
                }
                _ => Score::fail(
                    "tool_called_before",
                    format!("both {first} and {second} must be called"),
                ),
            }
        },
    )
}

// ----- trajectory (ATIF) scorers ---------------------------------------------
// These grade the structure of the ATIF trajectory — the protocol's primary
// trajectory contract ([`crate::trajectory`]) — via
// [`Transcript::tool_invocations`], which is how scorers see tool arguments and
// correlated observations. A transcript without a trajectory FAILS with
// "subject reported no trajectory" (the `ttft_within` precedent: an
// unverifiable check fails, it isn't N/A — N/A is reserved for infra).

/// Passes if some invocation of `tool` has arguments whose value at `pointer`
/// (an RFC 6901 JSON Pointer, e.g. `"/ticker"`) equals `expected` (JSON value
/// equality). Requires the ATIF trajectory
/// ([`Transcript::trajectory`](crate::Transcript::trajectory)) — arguments
/// only exist there; a transcript without one fails (the check can't be
/// verified).
pub fn tool_called_with(
    tool: impl Into<String>,
    pointer: impl Into<String>,
    expected: serde_json::Value,
) -> Box<dyn Scorer> {
    let tool = tool.into();
    let pointer = pointer.into();
    scorer(
        format!("tool_called_with({tool}, {pointer}, {expected})"),
        move |_, t| {
            if t.trajectory.is_none() {
                return Score::fail("tool_called_with", "subject reported no trajectory");
            }
            let hit = t.tool_invocations().iter().any(|c| {
                c.name == tool && c.arguments.and_then(|a| a.pointer(&pointer)) == Some(&expected)
            });
            if hit {
                Score::pass(
                    "tool_called_with",
                    format!("{tool} called with {pointer} == {expected}"),
                )
            } else {
                Score::fail(
                    "tool_called_with",
                    format!("no {tool} call with {pointer} == {expected}"),
                )
            }
        },
    )
}

/// Passes if some invocation of `tool` has a **string** argument at `pointer`
/// (an RFC 6901 JSON Pointer) matching the regex `pattern` — the regex variant
/// of [`tool_called_with`]. A non-string value at the pointer fails with a
/// reason (grade non-strings with `tool_called_with` instead). Requires the
/// ATIF trajectory; a transcript without one fails.
pub fn tool_arg_matches(
    tool: impl Into<String>,
    pointer: impl Into<String>,
    pattern: impl Into<String>,
) -> Box<dyn Scorer> {
    let tool = tool.into();
    let pointer = pointer.into();
    let pattern = pattern.into();
    let compiled = regex::Regex::new(&pattern);
    scorer(
        format!("tool_arg_matches({tool}, {pointer}, {pattern:?})"),
        move |_, t| {
            let re = match &compiled {
                Ok(re) => re,
                Err(e) => return Score::fail("tool_arg_matches", format!("bad pattern: {e}")),
            };
            if t.trajectory.is_none() {
                return Score::fail("tool_arg_matches", "subject reported no trajectory");
            }
            let mut non_string = false;
            for c in t.tool_invocations() {
                if c.name != tool {
                    continue;
                }
                match c.arguments.and_then(|a| a.pointer(&pointer)) {
                    Some(serde_json::Value::String(s)) if re.is_match(s) => {
                        return Score::pass(
                            "tool_arg_matches",
                            format!("{tool} called with {pointer} matching {pattern:?}"),
                        );
                    }
                    Some(serde_json::Value::String(_)) | None => {}
                    Some(_) => non_string = true,
                }
            }
            if non_string {
                Score::fail(
                    "tool_arg_matches",
                    format!("non-string value at {pointer} in {tool} arguments"),
                )
            } else {
                Score::fail(
                    "tool_arg_matches",
                    format!("no {tool} call with {pointer} matching {pattern:?}"),
                )
            }
        },
    )
}

/// Passes if the observation content correlated to some invocation of `tool`
/// (joined via `source_call_id`) contains `needle` as a substring. Multimodal
/// observation content is graded on its text projection
/// ([`StepContent::text`](crate::trajectory::StepContent::text)). Requires the
/// ATIF trajectory — observations only exist there; a transcript without one
/// fails.
pub fn observation_contains(tool: impl Into<String>, needle: impl Into<String>) -> Box<dyn Scorer> {
    let tool = tool.into();
    let needle = needle.into();
    scorer(
        format!("observation_contains({tool}, {needle:?})"),
        move |_, t| {
            if t.trajectory.is_none() {
                return Score::fail("observation_contains", "subject reported no trajectory");
            }
            let hit = t
                .tool_invocations()
                .iter()
                .any(|c| c.name == tool && c.result.is_some_and(|r| r.text().contains(&needle)));
            if hit {
                Score::pass(
                    "observation_contains",
                    format!("{tool} observation contains {needle:?}"),
                )
            } else {
                Score::fail(
                    "observation_contains",
                    format!("no {tool} observation contains {needle:?}"),
                )
            }
        },
    )
}

/// Passes if the trajectory has at most `max` steps — the ATIF step-count
/// budget (distinct from [`turns_within`], which counts subject-reported
/// iterations). Requires the ATIF trajectory; a transcript without one fails.
pub fn steps_within(max: usize) -> Box<dyn Scorer> {
    scorer(format!("steps_within({max})"), move |_, t| {
        match &t.trajectory {
            Some(tr) if tr.steps.len() <= max => {
                Score::pass("steps_within", format!("{} <= {max}", tr.steps.len()))
            }
            Some(tr) => Score::fail("steps_within", format!("{} > {max}", tr.steps.len())),
            None => Score::fail("steps_within", "subject reported no trajectory"),
        }
    })
}

// ----- JSON output checks ---------------------------------------------------

/// Passes if the final response parses as a JSON value.
pub fn json_valid() -> Box<dyn Scorer> {
    scorer("json_valid", move |_, t| {
        match serde_json::from_str::<serde_json::Value>(t.final_response.trim()) {
            Ok(_) => Score::pass("json_valid", "valid JSON"),
            Err(e) => Score::fail("json_valid", format!("invalid JSON: {e}")),
        }
    })
}

/// Passes if the final response is a JSON object whose top-level `key` equals the
/// (string) `value`.
pub fn json_field_equals(key: impl Into<String>, value: impl Into<String>) -> Box<dyn Scorer> {
    let key = key.into();
    let value = value.into();
    scorer(
        format!("json_field_equals({key}={value:?})"),
        move |_, t| {
            let parsed: Result<serde_json::Value, _> =
                serde_json::from_str(t.final_response.trim());
            match parsed.ok().and_then(|v| v.get(&key).cloned()) {
                Some(serde_json::Value::String(s)) if s == value => {
                    Score::pass("json_field_equals", format!("{key} == {value:?}"))
                }
                Some(other) => Score::fail(
                    "json_field_equals",
                    format!("{key} is {other}, expected {value:?}"),
                ),
                None => Score::fail("json_field_equals", format!("no JSON field {key}")),
            }
        },
    )
}

// ----- multimodal output ----------------------------------------------------

/// Passes if the subject produced an output [`Part`](crate::Part) of the given
/// modality (`"text"` / `"image"` / `"audio"` / `"file"` / `"json"`). Grades
/// [`Transcript::output`](crate::Transcript::output).
pub fn produced_modality(kind: impl Into<String>) -> Box<dyn Scorer> {
    let kind = kind.into();
    scorer(format!("produced_modality({kind})"), move |_, t| {
        if t.output.iter().any(|p| p.kind() == kind) {
            Score::pass("produced_modality", format!("produced a {kind} part"))
        } else {
            Score::fail("produced_modality", format!("no {kind} part in output"))
        }
    })
}

// ----- combinators ----------------------------------------------------------

/// Passes only if **all** inner scorers pass. The aggregate value is their mean.
/// Composes the built-ins into higher-level rubrics without a new trait.
pub fn all_of(name: impl Into<String>, scorers: Vec<Box<dyn Scorer>>) -> Box<dyn Scorer> {
    Box::new(Combinator {
        name: name.into(),
        scorers,
        require_all: true,
    })
}

/// Passes if **any** inner scorer passes. The aggregate value is the max.
pub fn any_of(name: impl Into<String>, scorers: Vec<Box<dyn Scorer>>) -> Box<dyn Scorer> {
    Box::new(Combinator {
        name: name.into(),
        scorers,
        require_all: false,
    })
}

/// Inverts a scorer: passes iff the inner scorer fails.
pub fn not(inner: Box<dyn Scorer>) -> Box<dyn Scorer> {
    Box::new(Not(inner))
}

struct Combinator {
    name: String,
    scorers: Vec<Box<dyn Scorer>>,
    require_all: bool,
}

#[async_trait]
impl Scorer for Combinator {
    fn name(&self) -> String {
        self.name.clone()
    }
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score {
        // N/A inner scores are ignored; if every inner scorer is N/A, the whole
        // combinator is N/A (nothing could be evaluated).
        let mut values = Vec::with_capacity(self.scorers.len());
        let mut reasons = Vec::new();
        for s in &self.scorers {
            let sc = s.score(sample, transcript).await;
            let glyph = if sc.na {
                "–"
            } else if sc.pass {
                "✓"
            } else {
                "✗"
            };
            reasons.push(format!("{glyph}{}", sc.scorer));
            if !sc.na {
                values.push((sc.value, sc.pass));
            }
        }
        let reason = reasons.join(", ");
        if values.is_empty() {
            return Score::na(self.name.clone(), reason);
        }
        let (pass, value) = if self.require_all {
            (
                values.iter().all(|(_, p)| *p),
                values.iter().map(|(v, _)| v).sum::<f64>() / values.len() as f64,
            )
        } else {
            (
                values.iter().any(|(_, p)| *p),
                values.iter().map(|(v, _)| *v).fold(0.0_f64, f64::max),
            )
        };
        Score {
            scorer: self.name.clone(),
            value,
            pass,
            na: false,
            reason,
        }
    }
}

struct Not(Box<dyn Scorer>);

#[async_trait]
impl Scorer for Not {
    fn name(&self) -> String {
        format!("not({})", self.0.name())
    }
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score {
        let inner = self.0.score(sample, transcript).await;
        // You can't invert "couldn't evaluate" — N/A passes straight through.
        if inner.na {
            return Score::na(
                format!("not({})", inner.scorer),
                format!("inner N/A: {}", inner.reason),
            );
        }
        Score {
            scorer: format!("not({})", inner.scorer),
            value: 1.0 - inner.value,
            pass: !inner.pass,
            na: false,
            reason: format!("inverted: {}", inner.reason),
        }
    }
}

// ----- model-graded scorer --------------------------------------------------

/// An async judge: given a rubric and the transcript, return a [`Score`].
/// Backed by a separate model — keeping the judge independent of the model
/// under test is the standard guidance, though the framework does not enforce
/// it.
pub type JudgeFn =
    Box<dyn Fn(String, Transcript) -> Pin<Box<dyn Future<Output = Score> + Send>> + Send + Sync>;

pub struct ModelGraded {
    rubric: String,
    judge: JudgeFn,
}

#[async_trait]
impl Scorer for ModelGraded {
    fn name(&self) -> String {
        format!("model_graded({:?})", self.rubric)
    }
    async fn score(&self, _sample: &Sample, transcript: &Transcript) -> Score {
        (self.judge)(self.rubric.clone(), transcript.clone()).await
    }
}

/// Grade a transcript against a natural-language `rubric` using a `judge`
/// (typically a cheaper model). LLM-as-judge is just another [`Scorer`], not a
/// special case in the engine.
pub fn model_graded(rubric: impl Into<String>, judge: JudgeFn) -> Box<dyn Scorer> {
    Box::new(ModelGraded {
        rubric: rubric.into(),
        judge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn transcript() -> Transcript {
        Transcript {
            final_response: "The answer is 42.".into(),
            iterations: 2,
            tool_calls_count: 2,
            tool_calls: vec!["read".into(), "calc".into()],
            usage: crate::Usage {
                input_tokens: 5,
                output_tokens: 3,
                cost_usd: 0.01,
                ..Default::default()
            },
            timing: crate::Timing {
                duration_ms: 120,
                time_to_first_token_ms: Some(40),
            },
            files: BTreeMap::from([("out.txt".into(), "hello world".into())]),
            metrics: BTreeMap::from([("recall@5".into(), 0.8), ("p95_ms".into(), 250.0)]),
            ..Default::default()
        }
    }

    async fn run(scorer: Box<dyn Scorer>, sample: &Sample) -> Score {
        scorer.score(sample, &transcript()).await
    }

    #[tokio::test]
    async fn text_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(contains("42"), &s).await.pass);
        assert!(!run(contains("99"), &s).await.pass);
        assert!(run(not_contains("99"), &s).await.pass);
        assert!(run(regex(r"answer is \d+"), &s).await.pass);
        assert!(!run(regex(r"^\d+$"), &s).await.pass);
        assert!(run(equals("the answer is 42."), &s).await.pass); // case-insensitive
    }

    #[tokio::test]
    async fn expected_scorer() {
        let s = Sample::new("a", "q").expected("The answer is 42.");
        assert!(run(matches_expected(), &s).await.pass);
        let s2 = Sample::new("b", "q"); // no expected answer
        assert!(!run(matches_expected(), &s2).await.pass);
    }

    #[tokio::test]
    async fn structural_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(tool_called("calc"), &s).await.pass);
        assert!(!run(tool_called("grep"), &s).await.pass);
        assert!(run(tool_not_called("grep"), &s).await.pass);
        assert!(!run(tool_not_called("calc"), &s).await.pass);
        assert!(run(tool_calls_within(2), &s).await.pass);
        assert!(!run(tool_calls_within(1), &s).await.pass);
        assert!(run(turns_within(2), &s).await.pass);
        assert!(run(cost_within(0.05), &s).await.pass);
        assert!(!run(cost_within(0.001), &s).await.pass);
        assert!(run(succeeded(), &s).await.pass);
        assert!(run(non_empty(), &s).await.pass);
    }

    #[tokio::test]
    async fn budget_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(tokens_within(8), &s).await.pass);
        assert!(!run(tokens_within(7), &s).await.pass);
        assert!(run(output_tokens_within(3), &s).await.pass);
        assert!(!run(output_tokens_within(2), &s).await.pass);
        assert!(run(latency_within(200), &s).await.pass);
        assert!(!run(latency_within(100), &s).await.pass);
        assert!(run(ttft_within(50), &s).await.pass);
        assert!(!run(ttft_within(30), &s).await.pass);
    }

    #[tokio::test]
    async fn custom_metric_scorers() {
        let s = Sample::new("a", "q");
        // higher-is-better
        assert!(run(metric_at_least("recall@5", 0.75), &s).await.pass);
        assert!(!run(metric_at_least("recall@5", 0.9), &s).await.pass);
        // lower-is-better
        assert!(run(metric_within("p95_ms", 300.0), &s).await.pass);
        assert!(!run(metric_within("p95_ms", 200.0), &s).await.pass);
        // unreported metric can't be verified → fail
        assert!(!run(metric_within("absent", 1.0), &s).await.pass);
        assert!(!run(metric_at_least("absent", 0.0), &s).await.pass);
    }

    #[tokio::test]
    async fn richer_tool_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(tools_used_exactly(["calc", "read"]), &s).await.pass);
        assert!(!run(tools_used_exactly(["calc"]), &s).await.pass);
        assert!(run(tool_called_before("read", "calc"), &s).await.pass);
        assert!(!run(tool_called_before("calc", "read"), &s).await.pass);
        assert!(!run(tool_called_before("read", "grep"), &s).await.pass);
    }

    /// A transcript whose trajectory carries structured tool calls with
    /// arguments and correlated observations (plain and multimodal).
    fn trajectory_transcript() -> Transcript {
        use crate::trajectory::*;
        let mut t = Trajectory::new(Agent::new("a", "1"));
        t.steps
            .push(Step::new(1, StepSource::User, "price of GOOGL?"));
        let mut step = Step::new(2, StepSource::Agent, "");
        step.tool_calls = vec![
            ToolCall::new(
                "c1",
                "financial_search",
                serde_json::json!({"ticker": "GOOGL", "metric": "price", "count": 3}),
            ),
            ToolCall::new(
                "c2",
                "fetch",
                serde_json::json!({"url": "https://example.com/q"}),
            ),
        ];
        step.observation = Some(Observation {
            results: vec![
                ObservationResult {
                    source_call_id: Some("c1".into()),
                    content: Some("GOOGL: $185.35".into()),
                    ..Default::default()
                },
                ObservationResult {
                    source_call_id: Some("c2".into()),
                    content: Some(StepContent::Parts(vec![ContentPart::text("page body")])),
                    ..Default::default()
                },
            ],
        });
        t.steps.push(step);
        t.steps
            .push(Step::new(3, StepSource::Agent, "GOOGL is at $185.35."));
        Transcript::from_trajectory(t)
    }

    #[tokio::test]
    async fn trajectory_scorers() {
        use serde_json::json;
        let s = Sample::new("a", "q");
        let t = trajectory_transcript();

        // Argument equality via JSON Pointer (string and non-string values).
        let hit = tool_called_with("financial_search", "/ticker", json!("GOOGL"));
        assert!(hit.score(&s, &t).await.pass);
        let count = tool_called_with("financial_search", "/count", json!(3));
        assert!(count.score(&s, &t).await.pass);
        let miss = tool_called_with("financial_search", "/ticker", json!("AAPL"));
        assert!(!miss.score(&s, &t).await.pass);
        let bad_ptr = tool_called_with("financial_search", "/absent", json!("GOOGL"));
        assert!(!bad_ptr.score(&s, &t).await.pass);

        // Regex over string arguments; non-string at pointer fails with reason.
        assert!(
            tool_arg_matches("fetch", "/url", "^https://")
                .score(&s, &t)
                .await
                .pass
        );
        assert!(
            !tool_arg_matches("fetch", "/url", "^ftp://")
                .score(&s, &t)
                .await
                .pass
        );
        let non_string = tool_arg_matches("financial_search", "/count", r"\d+")
            .score(&s, &t)
            .await;
        assert!(!non_string.pass);
        assert!(
            non_string.reason.contains("non-string"),
            "{}",
            non_string.reason
        );
        assert!(
            !tool_arg_matches("fetch", "/url", "(")
                .score(&s, &t)
                .await
                .pass
        ); // bad pattern

        // Observation join via source_call_id (plain text and parts).
        assert!(
            observation_contains("financial_search", "$185.35")
                .score(&s, &t)
                .await
                .pass
        );
        assert!(
            observation_contains("fetch", "page body")
                .score(&s, &t)
                .await
                .pass
        );
        assert!(
            !observation_contains("financial_search", "AAPL")
                .score(&s, &t)
                .await
                .pass
        );

        // Step-count budget.
        assert!(steps_within(3).score(&s, &t).await.pass);
        assert!(!steps_within(2).score(&s, &t).await.pass);
    }

    #[tokio::test]
    async fn trajectory_scorers_fail_without_trajectory() {
        // The ttft_within precedent: an unverifiable check fails (not N/A).
        let s = Sample::new("a", "q");
        let t = transcript(); // legacy transcript: names only, no trajectory
        for scorer in [
            tool_called_with("calc", "/x", serde_json::json!(1)),
            tool_arg_matches("calc", "/x", r"\d+"),
            observation_contains("calc", "42"),
            steps_within(10),
        ] {
            let score = scorer.score(&s, &t).await;
            assert!(
                !score.pass && !score.na,
                "{}: {}",
                score.scorer,
                score.reason
            );
            assert_eq!(score.reason, "subject reported no trajectory");
        }
    }

    #[tokio::test]
    async fn json_scorers() {
        let s = Sample::new("a", "q");
        let t = Transcript::response(r#"{"answer": "42", "ok": true}"#);
        assert!(json_valid().score(&s, &t).await.pass);
        assert!(json_field_equals("answer", "42").score(&s, &t).await.pass);
        assert!(!json_field_equals("answer", "43").score(&s, &t).await.pass);
        assert!(!json_valid().score(&s, &transcript()).await.pass);
    }

    #[tokio::test]
    async fn combinators() {
        let s = Sample::new("a", "q");
        assert!(
            run(all_of("both", vec![contains("42"), succeeded()]), &s)
                .await
                .pass
        );
        assert!(
            !run(all_of("both", vec![contains("42"), contains("zzz")]), &s)
                .await
                .pass
        );
        assert!(
            run(any_of("either", vec![contains("zzz"), contains("42")]), &s)
                .await
                .pass
        );
        assert!(run(not(contains("zzz")), &s).await.pass);
        assert!(!run(not(contains("42")), &s).await.pass);
    }

    #[tokio::test]
    async fn combinators_handle_na() {
        let s = Sample::new("a", "q");
        let na = || scorer("infra", |_, _| Score::na("infra", "unreachable"));
        // N/A inner scores are ignored: all_of passes on the remaining scorer.
        let r = run(all_of("mix", vec![contains("42"), na()]), &s).await;
        assert!(r.pass && !r.na);
        // When every inner scorer is N/A, the combinator is itself N/A.
        let r = run(all_of("all_na", vec![na(), na()]), &s).await;
        assert!(r.na);
        // not() of an N/A stays N/A rather than flipping to pass.
        let r = run(not(na()), &s).await;
        assert!(r.na);
    }

    #[tokio::test]
    async fn file_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(file_exists("out.txt"), &s).await.pass);
        assert!(!run(file_exists("nope.txt"), &s).await.pass);
        assert!(run(file_contains("out.txt", "hello"), &s).await.pass);
        assert!(!run(file_contains("out.txt", "bye"), &s).await.pass);
    }

    #[tokio::test]
    async fn produced_modality_scorer() {
        use crate::Part;
        let s = Sample::new("a", "draw a cat");
        let t = Transcript::response("here you go")
            .with_output([Part::text("here you go"), Part::image_uri("image/png", "u")]);
        assert!(produced_modality("image").score(&s, &t).await.pass);
        assert!(produced_modality("text").score(&s, &t).await.pass);
        assert!(!produced_modality("audio").score(&s, &t).await.pass);
    }

    #[tokio::test]
    async fn model_graded_is_just_a_scorer() {
        let judge: JudgeFn = Box::new(|rubric, t| {
            Box::pin(async move {
                let pass = t.final_response.contains("42");
                Score::graded("judge", if pass { 1.0 } else { 0.0 }, 0.5, rubric)
            })
        });
        let s = run(
            model_graded("is it answered?", judge),
            &Sample::new("a", "q"),
        )
        .await;
        assert!(s.pass);
    }
}
