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

use crate::{Metadata, Params, Score, Timing, Transcript, Usage};

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
/// added the optional `transcript.metrics` map; `1.3` added the optional
/// `transcript.error_kind` (subject vs. infrastructure) so the host can retry
/// infra-errored cells; `1.4` widened `metadata` values from strings to
/// open-ended JSON (a newer peer may now send a number/bool/object/array where
/// an older one only sent strings); `1.5` promoted [`RpcError`] from
/// `{ message }` to the JSON-RPC-shaped `{ code, message, retryable, data }` (all
/// new fields optional/defaulted), so a protocol-level failure can be classified
/// and retried without parsing the human message; `1.6` added trials/repetitions —
/// the optional `trial`/`trials`/`seed` fields on `RunParams`/`ScoreParams`/
/// `RunResult`/`ExecuteResult` and the optional `EvalInfo.trials`/`EvalInfo.seed`,
/// so a cell can be run N times (seeded for reproducibility) and the host
/// aggregates pass@k / pass-rate / variance over the repetitions.
pub const PROTOCOL_VERSION: &str = "1.6";

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// study → host, correlated to a [`Request`] by `id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// A non-retryable error response carrying a plain message (code
    /// [`codes::INTERNAL_ERROR`]). Use [`Response::err_with`] to attach a
    /// specific code, the `retryable` hint, or structured `data`.
    pub fn err(id: u64, message: impl Into<String>) -> Self {
        Self::err_with(id, RpcError::new(message))
    }

    /// An error response carrying a fully-formed [`RpcError`].
    pub fn err_with(id: u64, error: RpcError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A structured, JSON-RPC-shaped protocol-level error.
///
/// Distinct from a transcript's `error`/`error_kind`, which classify a *subject*
/// failure (the model under test got it wrong). An [`RpcError`] is the failure of
/// the RPC itself — bad params, an unknown method, a study-side crash, a provider
/// outage surfaced at the transport. `code` and `retryable` let the host classify
/// and retry the request **without parsing the human `message`**, and `data`
/// carries optional structured context.
///
/// All fields beyond `message` are optional and defaulted, so a `1.4`-era peer
/// that sends bare `{ "message": "…" }` still parses (forward/backward compat).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RpcError {
    /// Numeric class of the failure (JSON-RPC convention; see [`codes`]). `0`
    /// when unclassified. Defaulted so older peers that omit it still parse.
    #[serde(default)]
    pub code: i32,
    /// Human-readable description. The only required field.
    pub message: String,
    /// Hint that retrying the identical request may succeed — a *transient
    /// infrastructure* fault (provider outage, timeout, rate limit), not the
    /// caller's mistake. Defaulted `false` so older peers parse and unknown
    /// failures aren't retried blindly.
    #[serde(default)]
    pub retryable: bool,
    /// Optional structured payload for programmatic handling (JSON-RPC `data`).
    /// Omitted from the wire when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC error codes used by [`RpcError::code`]. The negative range mirrors
/// the JSON-RPC 2.0 reserved codes; `0` means unclassified.
pub mod codes {
    /// Invalid method parameters (e.g. a malformed `RunParams`, an unknown
    /// eval/sample/model). The caller's mistake — not retryable.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Method not found / unsupported.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Internal study-side error. The default for [`super::RpcError::new`].
    pub const INTERNAL_ERROR: i32 = -32603;
}

impl RpcError {
    /// A non-retryable internal error ([`codes::INTERNAL_ERROR`]).
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: codes::INTERNAL_ERROR,
            message: message.into(),
            retryable: false,
            data: None,
        }
    }

    /// Set the error [`code`](RpcError::code).
    pub fn with_code(mut self, code: i32) -> Self {
        self.code = code;
        self
    }

    /// Mark this error as [`retryable`](RpcError::retryable) — a transient infra
    /// fault the host may re-attempt.
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    /// Attach a structured [`data`](RpcError::data) payload.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RpcError {}

/// study → host, fire-and-forget progress (no `id`). Carries live events (a
/// turn started, a tool was called, tokens spent) so the host can render
/// progress and, later, stream into a transcript viewer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Notification {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

// ----- method payloads ------------------------------------------------------

/// `initialize` result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Study honours the `trial`/`seed` run params — it threads the seed into the
    /// subject so repetitions are reproducible. Trials run regardless (the host
    /// drives the repetition); this advertises that seeding actually takes
    /// effect, not just that the cell is re-run.
    pub const TRIALS: &str = "trials";
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SampleInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AxisInfo {
    pub name: String,
    pub values: Vec<String>,
}

/// One eval, as advertised by `list`. Enough for the host to plan the full
/// `samples × models` grid and apply selection without running anything.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// How many times each cell of this eval should be run (trials/repetitions),
    /// for pass@k / variance over a stochastic subject. `0`/`1` mean a single
    /// run (no trial dimension). The host may override with `--trials`. Defaulted
    /// so older/foreign studies that omit it still parse.
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Base seed the study declared for reproducible trials (trial `t` uses
    /// `seed + t`). The host threads it into runs unless `--seed` overrides.
    /// Defaulted/omitted when the study left seeding to the subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Serde skip helper: a trial count of `0` or `1` is a single, unrepeated run, so
/// it's omitted on the wire (the common case stays clean).
fn is_single_trial(n: &usize) -> bool {
    *n <= 1
}

/// Serde skip helper for the 0-based `trial` index (omitted for the first/only).
fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListResult {
    pub evals: Vec<EvalInfo>,
}

/// `run` params: address one matrix cell by `(eval, sample, model label)` plus
/// any extra axis `params` (axis name → chosen value).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunParams {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Chosen value per extra matrix axis. Empty/omitted for a model-only
    /// matrix; defaulted so older hosts/servers interoperate.
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index when this cell is being repeated; `0` for a single
    /// run. Defaulted so older hosts/studies interoperate.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials planned for this cell (`0`/`1` = single run). Lets the study
    /// echo the cell's trial identity back so its key matches the host's plan.
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed for reproducibility, when the host set one. The study
    /// threads it to the subject (see [`crate::Trial::seed`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

impl RunParams {
    /// The [`Trial`](crate::Trial) this run addresses.
    pub fn trial(&self) -> crate::Trial {
        crate::Trial {
            index: self.trial,
            count: self.trials.max(1),
            seed: self.seed,
        }
    }
}

/// Lightweight transcript carried in results and checkpoints (the raw event
/// stream is omitted to keep the artifact small).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Classifies `error` (subject vs. infrastructure). Lets the host retry
    /// infra-errored cells. Defaulted/omitted for the common subject case.
    #[serde(default, skip_serializing_if = "crate::ErrorKind::is_subject")]
    pub error_kind: crate::ErrorKind,
    /// EXPERIMENTAL (gated behind `protocol-unstable`): reserved staging slot for
    /// the next *structural* wire addition — the kind the open `metrics`/`metadata`
    /// maps can't express (those carry numeric/string key-values; a new typed
    /// field or nested shape still needs staging). The worked example of the
    /// unstable convention: present on the wire and in the generated schema only
    /// when the feature is enabled, so it can be trialled before promotion. A
    /// placeholder — replace it with the real addition; don't depend on it.
    #[cfg(feature = "protocol-unstable")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental: Option<String>,
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
            error_kind: t.error_kind,
            // No source on the core `Transcript` yet — left unset until promoted.
            #[cfg(feature = "protocol-unstable")]
            experimental: None,
        }
    }
}

/// `execute` result for one cell: the **full** [`Transcript`] (raw events and
/// captured files included), so the host can persist it as an execution
/// artifact and `score` it later. Distinct from [`RunResult`], which carries the
/// lightweight [`TranscriptSummary`] plus scores.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExecuteResult {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Extra matrix-axis values for this cell (empty for a model-only matrix).
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index when this cell is repeated (see [`RunParams::trial`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials for this cell (`0`/`1` = single run). Part of the cell key
    /// when `> 1`, so trial artifacts stay distinct.
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed this transcript was produced with, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The complete transcript, unlike the summary carried in [`RunResult`].
    pub transcript: Transcript,
    /// True when the cell was not executed (e.g. model unavailable).
    #[serde(default)]
    pub skipped: bool,
}

impl ExecuteResult {
    /// Stable cell identity (see [`RunResult::key`]), trial-aware.
    pub fn key(&self) -> String {
        format!(
            "{}{}",
            crate::cell_key(&self.eval, &self.sample, &self.model, &self.params),
            crate::trial_suffix(self.trial, self.trials),
        )
    }
}

/// `score` params: a cell identity plus the full [`Transcript`] to score. The
/// transcript travels over the wire so the host can replay a stored one — the
/// study scores it without re-running the subject.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScoreParams {
    pub eval: String,
    pub sample: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index for the cell this transcript came from (echoed into
    /// the resulting [`RunResult`] so it keeps its trial identity on re-score).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials for this cell (`0`/`1` = single run).
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed this transcript was produced with, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub transcript: Transcript,
}

/// `run` result for one cell. Also the unit persisted in checkpoints.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunResult {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Extra matrix-axis values for this cell (empty for a model-only matrix).
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index when this cell is repeated (see [`RunParams::trial`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials for this cell (`0`/`1` = single run). Part of the cell key
    /// when `> 1`; the host groups results by their *logical* key (without the
    /// trial suffix) to aggregate pass@k / variance (see [`crate::aggregate`]).
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed this result was produced with, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub passed: bool,
    pub aggregate: f64,
    pub scores: Vec<Score>,
    pub transcript: TranscriptSummary,
    /// True when the cell was not executed (e.g. model unavailable).
    #[serde(default)]
    pub skipped: bool,
}

impl RunResult {
    /// Stable cell identity: `eval/sample@model` (with an `[k=v,…]` axis suffix
    /// and a `#trial` suffix when this cell is repeated). Used for selection,
    /// dedupe, and checkpoint resume.
    pub fn key(&self) -> String {
        format!(
            "{}{}",
            self.logical_key(),
            crate::trial_suffix(self.trial, self.trials)
        )
    }

    /// The cell identity **without** the `#trial` suffix — the key all trials of
    /// one cell share, so [`crate::aggregate`] can group them.
    pub fn logical_key(&self) -> String {
        crate::cell_key(&self.eval, &self.sample, &self.model, &self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises the `protocol-unstable` staging mechanism: when the feature is
    // on, the experimental field is part of the wire type and round-trips. The
    // committed schema (generated *without* the feature) must not contain it —
    // see `unstable_field_absent_from_stable_schema` in mira-schema-gen.
    #[cfg(feature = "protocol-unstable")]
    #[test]
    fn unstable_field_roundtrips_when_enabled() {
        let t = TranscriptSummary {
            experimental: Some("staged".into()),
            ..Default::default()
        };
        let line = serde_json::to_string(&t).unwrap();
        assert!(line.contains("experimental"));
        let back: TranscriptSummary = serde_json::from_str(&line).unwrap();
        assert_eq!(back.experimental.as_deref(), Some("staged"));
    }

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
    fn rpc_error_is_classifiable_and_roundtrips() {
        let err = RpcError::new("provider 503")
            .with_code(codes::INTERNAL_ERROR)
            .retryable()
            .with_data(serde_json::json!({ "provider": "anthropic" }));
        let line = serde_json::to_string(&err).unwrap();
        let back: RpcError = serde_json::from_str(&line).unwrap();
        assert!(back.retryable);
        assert_eq!(back.code, codes::INTERNAL_ERROR);
        assert_eq!(
            back.data,
            Some(serde_json::json!({ "provider": "anthropic" }))
        );
        // Default constructor is non-retryable and carries no data.
        let plain = RpcError::new("nope");
        assert!(!plain.retryable);
        assert!(!serde_json::to_string(&plain).unwrap().contains("data"));
    }

    #[test]
    fn rpc_error_backward_compatible_with_bare_message() {
        // A 1.4-era peer sends only `message`; the new optional fields default.
        let back: RpcError = serde_json::from_str(r#"{"message":"no such eval"}"#).unwrap();
        assert_eq!(back.message, "no such eval");
        assert_eq!(back.code, 0);
        assert!(!back.retryable);
        assert!(back.data.is_none());
    }

    #[test]
    fn run_result_key() {
        let r = RunResult {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            passed: true,
            aggregate: 1.0,
            scores: vec![],
            transcript: TranscriptSummary::default(),
            skipped: false,
        };
        assert_eq!(r.key(), "greet/hi@sim");

        let mut params = Params::new();
        params.insert("effort".into(), "high".into());
        let r2 = RunResult {
            params,
            ..r.clone()
        };
        assert_eq!(r2.key(), "greet/hi@sim[effort=high]");
    }

    #[test]
    fn run_result_trial_key_and_logical_key() {
        // A repeated cell (trials > 1) carries a `#index` suffix in its key, but
        // all trials share one logical key so the host can group them.
        let r = RunResult {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 2,
            trials: 5,
            seed: Some(42),
            passed: true,
            aggregate: 1.0,
            scores: vec![],
            transcript: TranscriptSummary::default(),
            skipped: false,
        };
        assert_eq!(r.key(), "greet/hi@sim#2");
        assert_eq!(r.logical_key(), "greet/hi@sim");

        // A single-trial cell (trials <= 1) keeps the plain key — backward compat.
        let single = RunResult {
            trial: 0,
            trials: 1,
            ..r.clone()
        };
        assert_eq!(single.key(), "greet/hi@sim");
    }

    #[test]
    fn pre_trials_payloads_parse_as_single_trial() {
        // A pre-trials study (pre-1.6) omits trial/trials/seed entirely. The host must parse
        // such a RunResult and treat it as a single, unrepeated cell (plain key).
        let line = r#"{"eval":"greet","sample":"hi","model":"sim","passed":true,
            "aggregate":1.0,"scores":[],
            "transcript":{"final_response":"hi","iterations":1,"tool_calls_count":0,
            "usage":{"input_tokens":1,"output_tokens":1,"cost_usd":0.0}}}"#;
        let r: RunResult = serde_json::from_str(line).unwrap();
        assert_eq!(r.trial, 0);
        assert_eq!(r.trials, 0);
        assert_eq!(r.seed, None);
        assert_eq!(r.key(), "greet/hi@sim"); // no `#trial` suffix
        assert_eq!(r.logical_key(), "greet/hi@sim");

        // Likewise an EvalInfo from a pre-1.5 study omits trials/seed.
        let line = r#"{"name":"greet","samples":[{"id":"hi"}],"scorers":["s"],
            "models":[{"label":"sim","available":true}]}"#;
        let e: EvalInfo = serde_json::from_str(line).unwrap();
        assert_eq!(e.trials, 0); // host clamps 0 → 1 (single run)
        assert_eq!(e.seed, None);
    }

    #[test]
    fn trial_fields_omitted_on_wire_for_single_run() {
        // The common single-trial case adds nothing to the wire: no trial/trials/
        // seed keys when unrepeated and unseeded.
        let p = RunParams {
            eval: "e".into(),
            sample: "s".into(),
            model: "m".into(),
            params: Default::default(),
            trial: 0,
            trials: 1,
            seed: None,
        };
        let line = serde_json::to_string(&p).unwrap();
        assert!(!line.contains("trial"));
        assert!(!line.contains("seed"));

        // A real trial serializes its fields and round-trips.
        let p2 = RunParams {
            trial: 3,
            trials: 8,
            seed: Some(7),
            ..p
        };
        let line2 = serde_json::to_string(&p2).unwrap();
        let back: RunParams = serde_json::from_str(&line2).unwrap();
        assert_eq!(back.trial, 3);
        assert_eq!(back.trials, 8);
        assert_eq!(back.seed, Some(7));
        assert_eq!(back.trial().count, 8);
    }
}
