//! The Mira eval protocol: newline-delimited JSON over stdio, MCP-style.
//!
//! Two processes talk:
//! * the **study** (your eval program) — defines evals in Rust, owns subject
//!   construction and scoring, and knows nothing about selection, the matrix,
//!   aggregation, checkpoints, or rendering. See [`crate::study`].
//! * the **host** (the `mira` CLI) — compiles + spawns the study, enumerates
//!   evals, plans the run (selection × matrix), drives execution, then
//!   aggregates / saves / checkpoints / visualizes. See [`crate::host`].
//!
//! Provider API keys live only in the study's environment and never cross the
//! wire — the host addresses models by *label*.
//!
//! ## Framing
//! One JSON object per line. A line with `id` is a [`Response`]; a line with
//! `method` but no `id` is a [`Notification`]. [`Request`]s flow host→study;
//! [`Response`]s and [`Notification`]s flow study→host.
//!
//! ## Methods
//! * `initialize` → [`InitializeResult`]
//! * `list` → [`ListResult`]
//! * `run` ([`RunParams`]) → [`RunResult`] — execute + score in one call
//! * `execute` ([`RunParams`]) → [`ExecuteResult`] — execute the subject only,
//!   returning the **full** transcript (for run-now, score-later)
//! * `score` ([`ScoreParams`]) → [`RunResult`] — score a supplied transcript
//!   (for deferred scoring and re-scoring)
//!
//! See `docs/protocol.md` for the full reference.

use serde::{Deserialize, Serialize};

use crate::{Metadata, Score, Timing, Transcript, Usage};

/// The protocol version advertised by `initialize`, as `MAJOR.MINOR`.
///
/// **Compatibility contract** (so old and new peers interoperate):
/// * The **major** version changes only on a breaking wire change. Peers with
///   different majors are incompatible — [`version_compatible`] returns false
///   and the host warns.
/// * The **minor** version increments for backwards-compatible additions (new
///   methods, new optional fields). A newer peer talking to an older one must
///   tolerate missing additions; an older peer must ignore unknown fields.
///
/// Every payload struct here is *non-exhaustive on the wire*: unknown fields are
/// ignored (no `deny_unknown_fields`) and new fields are `#[serde(default)]`, so
/// adding a field is a minor, non-breaking change.
///
/// History (all additive over `1.0`): `1.1` added `ModelInfo.provider`; `1.2`
/// added the optional `transcript.metrics` map.
pub const PROTOCOL_VERSION: &str = "1.2";

/// The oldest protocol version this build can still talk to.
pub const MIN_PROTOCOL_VERSION: &str = "1.0";

/// The major component of a `MAJOR.MINOR` version string (0 if malformed).
pub fn version_major(v: &str) -> u32 {
    v.split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Whether this build can talk to a peer advertising version `other`. Same major
/// ⇒ compatible (minor differences are additive by contract).
pub fn version_compatible(other: &str) -> bool {
    version_major(other) == version_major(PROTOCOL_VERSION)
}

/// host → study.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// study → host, correlated to a [`Request`] by `id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(RpcError {
                message: message.into(),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub message: String,
}

/// study → host, fire-and-forget progress (no `id`). Carries live events (a
/// turn started, a tool was called, tokens spent) so the host can render
/// progress and, later, stream into a transcript viewer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

// ----- method payloads ------------------------------------------------------

/// `initialize` result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub study: String,
    pub evals: usize,
    /// Optional study version string (e.g. the study crate's version). For
    /// diagnostics; defaulted for forward/backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study_version: Option<String>,
    /// Named capabilities this study supports beyond the base methods, so hosts
    /// can feature-detect additively (e.g. `"axes"`, `"events"`). Defaulted, so
    /// an older study that omits it is treated as base-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Capability tokens a study may advertise in [`InitializeResult::capabilities`].
pub mod capabilities {
    /// Study advertises extra matrix axes in `list` and honours `run.params`.
    pub const AXES: &str = "axes";
    /// Study emits `event` progress notifications during `run`.
    pub const EVENTS: &str = "events";
    /// Study reports token/cost usage and timing in transcripts.
    pub const USAGE: &str = "usage";
    /// Study answers `execute` (run the subject only, returning a full
    /// transcript) for run-now-score-later workflows.
    pub const EXECUTE: &str = "execute";
    /// Study answers `score` (run scorers over a supplied transcript) for
    /// deferred scoring and re-scoring of stored transcripts.
    pub const SCORE: &str = "score";
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SampleInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    pub label: String,
    /// Provider id (e.g. `sim`, `anthropic`, `openai`). Lets the host bucket
    /// concurrency per provider so one provider's rate limits can't be flooded.
    /// Defaulted (empty) so an older/foreign study that omits it still parses;
    /// such cells share the empty-provider bucket.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// False when a real provider's API key is absent in the study's env.
    pub available: bool,
}

/// One extra matrix axis advertised by `list`, so the host can plan the full
/// cross-product without running anything.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AxisInfo {
    pub name: String,
    pub values: Vec<String>,
}

/// One eval, as advertised by `list`. Enough for the host to plan the full
/// `samples × models` grid and apply selection without running anything.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub samples: Vec<SampleInfo>,
    pub scorers: Vec<String>,
    pub models: Vec<ModelInfo>,
    /// Extra matrix axes beyond the model. Defaulted so older servers that omit
    /// the field still parse (forward compatibility).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<AxisInfo>,
    /// Defaulted so a foreign/older study that omits it still parses, per the
    /// protocol's forward-compatibility contract (see docs/protocol.md).
    #[serde(default)]
    pub max_turns: usize,
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListResult {
    pub evals: Vec<EvalInfo>,
}

/// `run` params: address one matrix cell by `(eval, sample, model label)` plus
/// any extra axis `params` (axis name → chosen value).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunParams {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Chosen value per extra matrix axis. Empty/omitted for a model-only
    /// matrix; defaulted so older hosts/servers interoperate.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub params: Metadata,
}

/// Lightweight transcript carried in results and checkpoints (the raw event
/// stream is omitted to keep the artifact small).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranscriptSummary {
    pub final_response: String,
    pub iterations: usize,
    pub tool_calls_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<String>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Timing::is_default")]
    pub timing: Timing,
    /// Custom open-vocabulary numeric metrics (see `Transcript::metrics`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metrics: std::collections::BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TranscriptSummary {
    /// Project a full [`Transcript`] onto the lightweight wire/checkpoint form,
    /// dropping the raw `events` and captured `files` to keep results small.
    pub fn of(t: &Transcript) -> Self {
        Self {
            final_response: t.final_response.clone(),
            iterations: t.iterations,
            tool_calls_count: t.tool_calls_count,
            tool_calls: t.tool_calls.clone(),
            usage: t.usage,
            timing: t.timing,
            metrics: t.metrics.clone(),
            metadata: t.metadata.clone(),
            error: t.error.clone(),
        }
    }
}

/// `execute` result for one cell: the **full** [`Transcript`] (raw events and
/// captured files included), so the host can persist it as an execution
/// artifact and `score` it later. Distinct from [`RunResult`], which carries the
/// lightweight [`TranscriptSummary`] plus scores.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecuteResult {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Extra matrix-axis values for this cell (empty for a model-only matrix).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub params: Metadata,
    /// The complete transcript, unlike the summary carried in [`RunResult`].
    pub transcript: Transcript,
    /// True when the cell was not executed (e.g. model unavailable).
    #[serde(default)]
    pub skipped: bool,
}

impl ExecuteResult {
    /// Stable cell identity (see [`RunResult::key`]).
    pub fn key(&self) -> String {
        crate::cell_key(&self.eval, &self.sample, &self.model, &self.params)
    }
}

/// `score` params: a cell identity plus the full [`Transcript`] to score. The
/// transcript travels over the wire so the host can replay a stored one — the
/// study scores it without re-running the subject.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreParams {
    pub eval: String,
    pub sample: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub params: Metadata,
    pub transcript: Transcript,
}

/// `run` result for one cell. Also the unit persisted in checkpoints.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Extra matrix-axis values for this cell (empty for a model-only matrix).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub params: Metadata,
    pub passed: bool,
    pub aggregate: f64,
    pub scores: Vec<Score>,
    pub transcript: TranscriptSummary,
    /// True when the cell was not executed (e.g. model unavailable).
    #[serde(default)]
    pub skipped: bool,
}

impl RunResult {
    /// Stable cell identity: `eval/sample@model` (with an `[k=v,…]` suffix when
    /// extra axes vary). Used for selection, dedupe, and checkpoint resume.
    pub fn key(&self) -> String {
        crate::cell_key(&self.eval, &self.sample, &self.model, &self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_response_roundtrip() {
        let req = Request {
            id: 7,
            method: "run".into(),
            params: serde_json::json!({"eval": "e"}),
        };
        let line = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.method, "run");
    }

    #[test]
    fn version_compatibility() {
        assert!(version_compatible(PROTOCOL_VERSION));
        assert!(version_compatible("1.5")); // newer minor, same major
        assert!(!version_compatible("2.0")); // newer major
        assert!(!version_compatible("0.9")); // older major
        assert_eq!(version_major("1.4"), 1);
        assert_eq!(version_major("garbage"), 0);
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compat() {
        // A future study adds fields the host doesn't know — must still parse.
        let line = r#"{"protocol_version":"1.7","study":"x","evals":2,
            "capabilities":["axes","future_thing"],"brand_new_field":{"a":1}}"#;
        let info: InitializeResult = serde_json::from_str(line).unwrap();
        assert_eq!(info.evals, 2);
        assert!(info.capabilities.contains(&"axes".to_string()));
    }

    #[test]
    fn eval_info_defaults_missing_optional_fields() {
        // A foreign/older study (e.g. the Python example) omits max_turns, axes,
        // description, and metadata. Per the forward-compat contract it must parse.
        let line = r#"{"name":"greet","samples":[{"id":"hi"}],
            "scorers":["succeeded"],"models":[{"label":"sim","available":true}]}"#;
        let info: EvalInfo = serde_json::from_str(line).unwrap();
        assert_eq!(info.max_turns, 0);
        assert!(info.axes.is_empty());
        assert_eq!(info.samples.len(), 1);
    }

    #[test]
    fn notification_has_no_id() {
        let n = Notification {
            method: "event".into(),
            params: serde_json::json!({"kind": "started"}),
        };
        let line = serde_json::to_string(&n).unwrap();
        assert!(!line.contains("\"id\""));
        // A notification must not parse as a Response (no id).
        assert!(serde_json::from_str::<Response>(&line).is_err());
    }

    #[test]
    fn run_result_key() {
        let r = RunResult {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            passed: true,
            aggregate: 1.0,
            scores: vec![],
            transcript: TranscriptSummary::default(),
            skipped: false,
        };
        assert_eq!(r.key(), "greet/hi@sim");

        let mut params = Metadata::new();
        params.insert("effort".into(), "high".into());
        let r2 = RunResult { params, ..r };
        assert_eq!(r2.key(), "greet/hi@sim[effort=high]");
    }
}
