//! ATIF trajectories: the **primary structured trajectory contract** of the
//! Mira eval protocol.
//!
//! These types mirror the [Agent Trajectory Interchange Format][atif]
//! (ATIF-v1.7, harbor RFC 0001) field-for-field: an ordered list of [`Step`]s
//! (user/system/agent turns) carrying structured [`ToolCall`]s, correlated
//! [`Observation`]s, per-step [`StepMetrics`], and aggregate [`FinalMetrics`].
//! A subject that can produce one attaches it as
//! [`Transcript::trajectory`](crate::Transcript::trajectory); scorers and
//! consumers read it — never the raw `events` channel — for anything the
//! trajectory models (tool arguments, observations, reasoning, per-step
//! metrics).
//!
//! Design decisions (kept here, on the module):
//! * **Interchange fidelity beats internal uniformity.** ATIF's shapes are
//!   carried verbatim ([`ContentPart`]/[`ImageSource`] are *not* unified with
//!   Mira's [`Part`](crate::Part)/[`Source`](crate::Source)), so a Mira
//!   trajectory is a valid ATIF document for external SFT/RL/visualization
//!   tooling, byte-compatible with what harbor-ecosystem agents emit.
//! * **Lenient by construction.** Mira emits [`ATIF_VERSION`] and reads any
//!   `ATIF-v1.x` (v1 additions are optional fields); unknown fields are
//!   ignored everywhere (no `deny_unknown_fields`); [`Step::source`] is an
//!   open vocabulary ([`StepSource::Other`] carries unknown originators
//!   through). A non-v1 `schema_version` is rejected gracefully by
//!   [`Trajectory::from_json`] — an error, never a panic — because trajectory
//!   JSON is untrusted study output.
//! * **Zero client burden.** The flat `Transcript` fields (`final_response`,
//!   `tool_calls`, `iterations`, `usage`) are *projections* of the trajectory:
//!   [`Trajectory::project_into`] derives them, and the framework applies it
//!   wherever a transcript is produced or received, so a subject may set
//!   `trajectory` alone and everything else — including every existing
//!   name-based scorer — keeps working. `events` is never required alongside.
//! * **Reward stays Mira-side.** Mira's verdicts live in `Score`/`RunResult`
//!   and are computed (and re-computed) after the transcript exists; Mira
//!   never reads or writes `trajectory.extra.reward` on the wire.
//!
//! Cross-SDK parity: `sdks/python/mira/trajectory.py` and
//! `sdks/typescript/src/trajectory.ts` hand-mirror the projection; behaviour
//! is pinned by the shared vectors in `schema/v1/conformance/trajectory.json`
//! (three runners, one fixture — like `scorers.json`).
//!
//! [atif]: https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md

use serde::{Deserialize, Serialize};

use crate::{Metadata, Transcript, Usage};

/// The ATIF schema version Mira **emits** (`schema_version` on new
/// trajectories). Parsing is more lenient: any `ATIF-v1.x` document is read
/// (see [`is_supported_schema_version`]) — this constant is not a ceiling on
/// what Mira accepts.
pub const ATIF_VERSION: &str = "ATIF-v1.7";

/// The trajectory format name advertised in `capability_params`
/// (`{"trajectory": {"format": "ATIF", "version": "1.7"}}`).
pub const ATIF_FORMAT: &str = "ATIF";

/// True when `schema_version` names an ATIF v1 document this build can read.
/// Every `ATIF-v1.x` is accepted — v1 minor additions are optional fields, and
/// unknown fields are ignored crate-wide — while other prefixes (a future
/// `ATIF-v2.0`, garbage) are rejected gracefully by [`Trajectory::from_json`].
pub fn is_supported_schema_version(version: &str) -> bool {
    version == "ATIF-v1" || version.starts_with("ATIF-v1.")
}

/// One ATIF trajectory document: the root object (RFC 0001, ATIF-v1.7).
///
/// Attach it via [`Transcript::from_trajectory`] (or set
/// [`Transcript::trajectory`] directly): the flat transcript fields are then
/// derived automatically — see [`Trajectory::project_into`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Trajectory {
    /// ATIF compatibility marker, e.g. `"ATIF-v1.7"`. Required by ATIF; Mira
    /// emits [`ATIF_VERSION`] and parses any `ATIF-v1.x`.
    pub schema_version: String,
    /// Run-scoped identifier (may be shared by a parent and its subagents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Per-document identifier; required on embedded subagent trajectories,
    /// where it is the resolution key for [`SubagentTrajectoryRef`]s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<String>,
    /// The agent system that produced this trajectory.
    pub agent: Agent,
    /// The complete interaction history, ordered by `step_id` (1-based).
    pub steps: Vec<Step>,
    /// Free-form producer notes (design notes, format discrepancies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Aggregate token/cost totals for the whole trajectory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_metrics: Option<FinalMetrics>,
    /// Reference to a continuation trajectory file, when context management
    /// split the run across documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued_trajectory_ref: Option<String>,
    /// Embedded subagent trajectories (each a complete, independently valid
    /// ATIF document). Parsed and round-tripped; projections do **not**
    /// recurse into them — subagents are opaque-but-preserved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent_trajectories: Vec<Trajectory>,
    /// Custom root-level metadata. Mira never reads or writes `extra.reward`
    /// on the wire (verdicts live in `Score`/`RunResult`).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

/// The agent system a [`Trajectory`] was produced by (ATIF _AgentSchema_).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Agent {
    /// Agent system name (e.g. `"claude-code"`, `"mini-swe-agent"`).
    pub name: String,
    /// Agent system version (e.g. `"1.0.0"`).
    pub version: String,
    /// Default LLM for the trajectory; a step-level `model_name` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// Tool/function definitions available to the agent (OpenAI function
    /// schema). Opaque to Mira — carried, never introspected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_definitions: Vec<serde_json::Value>,
    /// Custom agent configuration not covered by the core schema.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

impl Agent {
    /// An agent identity with just the required `name` + `version`.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            ..Default::default()
        }
    }
}

/// The originator of a [`Step`]: `system`, `user`, or `agent`.
///
/// An **open** vocabulary, like `EventParams::kind`: a value this build
/// doesn't know parses into [`StepSource::Other`] and round-trips verbatim
/// (forward compatibility with future ATIF sources), rather than failing.
/// Serializes as a plain string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum StepSource {
    /// A system prompt or system-initiated operation.
    System,
    /// A (real or simulated) user message.
    User,
    /// An agent turn: LLM inference, tool calls, observations.
    Agent,
    /// A source this build doesn't recognise, carried through verbatim.
    Other(String),
}

impl StepSource {
    /// The wire string: `"system"` / `"user"` / `"agent"` / the raw value.
    pub fn as_str(&self) -> &str {
        match self {
            StepSource::System => "system",
            StepSource::User => "user",
            StepSource::Agent => "agent",
            StepSource::Other(s) => s,
        }
    }
}

impl From<String> for StepSource {
    fn from(s: String) -> Self {
        match s.as_str() {
            "system" => StepSource::System,
            "user" => StepSource::User,
            "agent" => StepSource::Agent,
            _ => StepSource::Other(s),
        }
    }
}

impl From<StepSource> for String {
    fn from(s: StepSource) -> Self {
        s.as_str().to_string()
    }
}

// The schema is a plain (open-vocabulary) string, not a closed enum — the
// derive would freeze the variant set and hide `Other`. Inlined at use sites so
// SDK codegens see an ordinary string field.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for StepSource {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "StepSource".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Step originator: \"system\", \"user\", or \"agent\" (open vocabulary; unknown values are carried through)."
        })
    }
}

/// A step's `message` (or an observation's `content`): plain text, or — since
/// ATIF v1.6 — an ordered list of multimodal [`ContentPart`]s. Untagged on the
/// wire: a JSON string or a JSON array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum StepContent {
    /// Plain text content.
    Text(String),
    /// Multimodal content (text and image parts).
    Parts(Vec<ContentPart>),
}

impl Default for StepContent {
    fn default() -> Self {
        StepContent::Text(String::new())
    }
}

impl From<String> for StepContent {
    fn from(s: String) -> Self {
        StepContent::Text(s)
    }
}

impl From<&str> for StepContent {
    fn from(s: &str) -> Self {
        StepContent::Text(s.to_string())
    }
}

impl StepContent {
    /// The text projection: the string itself, or the `text` parts joined by
    /// newlines (image parts are skipped) — the same rule as
    /// [`crate::content::text_of`].
    pub fn text(&self) -> String {
        match self {
            StepContent::Text(s) => s.clone(),
            StepContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// True when there is no content at all (empty string or no parts).
    pub fn is_empty(&self) -> bool {
        match self {
            StepContent::Text(s) => s.is_empty(),
            StepContent::Parts(parts) => parts.is_empty(),
        }
    }
}

/// One multimodal content piece (ATIF _ContentPartSchema_, v1.6+): `type` is
/// `"text"` (with `text` set) or `"image"` (with `source` set). Kept an open
/// struct — not a tagged enum — so a future ATIF content type parses instead
/// of failing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContentPart {
    /// `"text"` or `"image"` (open vocabulary).
    #[serde(rename = "type")]
    pub kind: String,
    /// The text content, when `type == "text"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The image reference, when `type == "image"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ImageSource>,
    /// Custom part-level metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

impl ContentPart {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text".into(),
            text: Some(text.into()),
            ..Default::default()
        }
    }

    /// An image part referencing a stored file (see [`ImageSource`]).
    pub fn image(media_type: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            kind: "image".into(),
            source: Some(ImageSource {
                media_type: media_type.into(),
                path: path.into(),
            }),
            ..Default::default()
        }
    }
}

/// An image stored alongside the trajectory and referenced by path/URL (ATIF
/// _ImageSourceSchema_) — never embedded bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ImageSource {
    /// MIME type (`image/png`, `image/jpeg`, …).
    pub media_type: String,
    /// Relative or absolute file path, or a URL (conventionally an `images/`
    /// sidecar next to the trajectory file).
    pub path: String,
}

/// One interaction turn (ATIF _StepObject_): a system prompt, a user message,
/// or a complete agent turn (inference, tool calls, observation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Step {
    /// 1-based ordinal of this step.
    pub step_id: u64,
    /// ISO 8601 timestamp, carried as an opaque string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Who produced this step (open vocabulary — see [`StepSource`]).
    pub source: StepSource,
    /// The LLM used for this turn, overriding [`Agent::model_name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// Qualitative or quantitative effort (`"low"` / `0.3` / …). Carried as
    /// opaque JSON — ATIF allows string or float.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<serde_json::Value>,
    /// The dialogue message. Required by ATIF (it may be an empty string) and
    /// always serialized; parsing is lenient — a step that omits it defaults
    /// to empty text instead of rejecting the whole document.
    #[serde(default)]
    pub message: StepContent,
    /// The agent's explicit internal reasoning, when surfaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Structured tool/function invocations made in this step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Environment feedback for this step's actions, correlated to
    /// `tool_calls` via `source_call_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
    /// LLM operational metrics for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<StepMetrics>,
    /// Number of LLM inferences this step represents. `Some(0)` on an agent
    /// step marks a deterministic (non-LLM) dispatch; `None` means the
    /// producer didn't track it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_call_count: Option<u32>,
    /// True when this step was copied from a prior trajectory for context
    /// (e.g. retained across a compaction boundary) — not a new interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_copied_context: Option<bool>,
    /// Custom step-level metadata (e.g. the `context_management` convention).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

impl Step {
    /// A step with the required fields; everything else at its default.
    pub fn new(step_id: u64, source: StepSource, message: impl Into<StepContent>) -> Self {
        Self {
            step_id,
            timestamp: None,
            source,
            model_name: None,
            reasoning_effort: None,
            message: message.into(),
            reasoning_content: None,
            tool_calls: Vec::new(),
            observation: None,
            metrics: None,
            llm_call_count: None,
            is_copied_context: None,
            extra: Metadata::new(),
        }
    }

    /// True for an agent step (the only kind projections count).
    pub fn is_agent(&self) -> bool {
        self.source == StepSource::Agent
    }
}

/// One structured tool/function invocation (ATIF _ToolCallSchema_).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolCall {
    /// Unique id, correlated with [`ObservationResult::source_call_id`].
    pub tool_call_id: String,
    /// The tool/function name (what `tool_called(...)` and friends grade).
    pub function_name: String,
    /// The invocation arguments — a JSON object per ATIF (may be `{}`).
    pub arguments: serde_json::Value,
    /// Custom call-level metadata (timeout, retry count, …).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

impl ToolCall {
    /// A tool call with the required fields.
    pub fn new(
        tool_call_id: impl Into<String>,
        function_name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            function_name: function_name.into(),
            arguments,
            extra: Metadata::new(),
        }
    }
}

/// Environment feedback for one step (ATIF _ObservationSchema_).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Observation {
    /// One result per tool call or action.
    pub results: Vec<ObservationResult>,
}

/// One observation result (ATIF _ObservationResultSchema_).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ObservationResult {
    /// The [`ToolCall::tool_call_id`] this result answers; absent for actions
    /// outside the standard tool-calling format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_call_id: Option<String>,
    /// The tool/action output (text or multimodal parts). May be omitted when
    /// `subagent_trajectory_ref` carries the full detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<StepContent>,
    /// References to delegated subagent trajectories (embedded via
    /// `trajectory_id` or external via `trajectory_path`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent_trajectory_ref: Vec<SubagentTrajectoryRef>,
    /// Custom result-level metadata (retrieval score, source doc id, …).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

/// A reference to a delegated subagent trajectory (ATIF
/// _SubagentTrajectoryRefSchema_). At least one of `trajectory_id` (embedded
/// form) or `trajectory_path` (file-ref form) must be set to be resolvable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SubagentTrajectoryRef {
    /// Resolution key against the parent's `subagent_trajectories` array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<String>,
    /// External location of the subagent trajectory (file path, URL, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_path: Option<String>,
    /// The subagent's run identity — informational only, never a resolution key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Custom metadata about the subagent execution (summary, exit status, …).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

/// Per-step LLM metrics (ATIF _MetricsSchema_). All optional. Token ids and
/// logprobs are parsed and round-tripped, never interpreted by Mira.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StepMetrics {
    /// Total input tokens for this turn, **including** cached tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    /// Tokens generated by the response (reasoning + tool calls included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    /// The subset of `prompt_tokens` served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    /// Monetary cost of this step's API call, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Prompt token ids (carried opaquely for RL pipelines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_token_ids: Option<Vec<u64>>,
    /// Completion token ids (carried opaquely for RL pipelines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_token_ids: Option<Vec<u64>>,
    /// Per-completion-token log probabilities (carried opaquely).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Vec<f64>>,
    /// Provider-specific extras. ATIF has no first-class reasoning-token
    /// field; providers stash it here as `reasoning_tokens`, and the usage
    /// projection reads that key when present.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

/// Aggregate trajectory metrics (ATIF _FinalMetricsSchema_). All optional.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FinalMetrics {
    /// Sum of `prompt_tokens` across all steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_prompt_tokens: Option<u64>,
    /// Sum of `completion_tokens` across all steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_completion_tokens: Option<u64>,
    /// Sum of `cached_tokens` across all steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cached_tokens: Option<u64>,
    /// Total cost for the whole trajectory, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    /// Total steps (may differ from `steps.len()` when explained in `notes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<u64>,
    /// Custom aggregate metrics (`reasoning_tokens` is read from here by the
    /// usage projection when present).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub extra: Metadata,
}

/// A `u64` stashed in an ATIF `extra` map (e.g. `reasoning_tokens`), or 0.
fn extra_u64(extra: &Metadata, key: &str) -> u64 {
    extra
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

impl Trajectory {
    /// A new, empty trajectory for `agent`, stamped with [`ATIF_VERSION`].
    pub fn new(agent: Agent) -> Self {
        Self {
            schema_version: ATIF_VERSION.into(),
            session_id: None,
            trajectory_id: None,
            agent,
            steps: Vec::new(),
            notes: None,
            final_metrics: None,
            continued_trajectory_ref: None,
            subagent_trajectories: Vec::new(),
            extra: Metadata::new(),
        }
    }

    /// Parse one ATIF JSON document. Lenient within v1 — any `ATIF-v1.x`
    /// parses, unknown fields are ignored — but a non-v1 `schema_version`
    /// (e.g. a future `ATIF-v2.0`) is rejected with an error, never a panic:
    /// trajectory JSON is untrusted study/agent output, so malformed input
    /// must degrade to a message the caller can put on `transcript.error`.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("invalid ATIF trajectory: {e}"))?;
        Self::from_value(value)
    }

    /// [`from_json`](Self::from_json) over an already-parsed JSON value.
    pub fn from_value(value: serde_json::Value) -> Result<Self, String> {
        let trajectory: Trajectory =
            serde_json::from_value(value).map_err(|e| format!("invalid ATIF trajectory: {e}"))?;
        if !is_supported_schema_version(&trajectory.schema_version) {
            return Err(format!(
                "unsupported ATIF schema_version {:?}: this build reads ATIF-v1.x \
                 (emits {ATIF_VERSION})",
                trajectory.schema_version,
            ));
        }
        Ok(trajectory)
    }

    /// The text of the last agent step's message — the trajectory's
    /// `final_response` projection.
    pub fn final_agent_text(&self) -> Option<String> {
        self.steps
            .iter()
            .rev()
            .find(|s| s.is_agent())
            .map(|s| s.message.text())
    }

    /// Every `tool_calls[].function_name`, in step order (top level only —
    /// subagent trajectories are not recursed).
    pub fn tool_call_names(&self) -> Vec<String> {
        self.steps
            .iter()
            .flat_map(|s| s.tool_calls.iter().map(|c| c.function_name.clone()))
            .collect()
    }

    /// The `iterations` projection: agent steps that performed LLM inference —
    /// `llm_call_count != Some(0)` (`Some(0)` marks a deterministic dispatch;
    /// `None` counts, the producer just didn't track it).
    pub fn agent_iterations(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.is_agent() && s.llm_call_count != Some(0))
            .count()
    }

    /// The [`Usage`] projection: `final_metrics` when present, else the sum of
    /// per-step metrics. ATIF `prompt`/`completion`/`cached` map onto
    /// `input`/`output`/`cache_read`; `reasoning_tokens` is read from the
    /// corresponding `extra` map when a provider stashed it there (ATIF has no
    /// first-class slot for it).
    pub fn usage(&self) -> Usage {
        if let Some(fm) = &self.final_metrics {
            return Usage {
                input_tokens: fm.total_prompt_tokens.unwrap_or(0),
                output_tokens: fm.total_completion_tokens.unwrap_or(0),
                cache_read_tokens: fm.total_cached_tokens.unwrap_or(0),
                reasoning_tokens: extra_u64(&fm.extra, "reasoning_tokens"),
                cost_usd: fm.total_cost_usd.unwrap_or(0.0),
            };
        }
        let mut usage = Usage::default();
        for m in self.steps.iter().filter_map(|s| s.metrics.as_ref()) {
            usage.input_tokens += m.prompt_tokens.unwrap_or(0);
            usage.output_tokens += m.completion_tokens.unwrap_or(0);
            usage.cache_read_tokens += m.cached_tokens.unwrap_or(0);
            usage.reasoning_tokens += extra_u64(&m.extra, "reasoning_tokens");
            usage.cost_usd += m.cost_usd.unwrap_or(0.0);
        }
        usage
    }

    /// Fill a transcript's flat fields from this trajectory:
    /// `final_response` = the last agent step's text ([`final_agent_text`]);
    /// `tool_calls` = every `function_name` in step order
    /// ([`tool_call_names`]); `tool_calls_count` = that length; `iterations` =
    /// agent steps with `llm_call_count != Some(0)` ([`agent_iterations`]);
    /// `usage` = [`usage`](Self::usage).
    ///
    /// **Fills, never overwrites**: only fields still at their defaults are
    /// touched, so a subject that set a flat field explicitly wins. This is
    /// the zero-burden contract — set `trajectory` alone and the rest is
    /// derived; the framework calls this wherever a transcript is produced or
    /// received (see [`Transcript::project_trajectory`]), so neither studies
    /// nor SDKs need to. Top level only: subagent trajectories are opaque.
    ///
    /// [`final_agent_text`]: Self::final_agent_text
    /// [`tool_call_names`]: Self::tool_call_names
    /// [`agent_iterations`]: Self::agent_iterations
    pub fn project_into(&self, t: &mut Transcript) {
        if t.final_response.is_empty()
            && let Some(text) = self.final_agent_text()
        {
            t.final_response = text;
        }
        if t.tool_calls.is_empty() {
            t.tool_calls = self.tool_call_names();
        }
        if t.tool_calls_count == 0 {
            t.tool_calls_count = t.tool_calls.len();
        }
        if t.iterations == 0 {
            t.iterations = self.agent_iterations();
        }
        if t.usage == Usage::default() {
            t.usage = self.usage();
        }
    }
}

/// One structured tool invocation as seen by scorers: the name, the arguments
/// (when a trajectory carries them), and the correlated observation content.
/// Produced by [`Transcript::tool_invocations`].
#[derive(Clone, Copy, Debug)]
pub struct ToolInvocation<'a> {
    /// The tool/function name.
    pub name: &'a str,
    /// The call arguments — `None` when synthesized from the legacy
    /// name-only `tool_calls` list.
    pub arguments: Option<&'a serde_json::Value>,
    /// The observation content correlated via `source_call_id`, when any.
    pub result: Option<&'a StepContent>,
}

impl Transcript {
    /// Structured tool invocations, ATIF-first: from [`trajectory`] when
    /// present (names + arguments + observation content joined on
    /// `source_call_id`), else synthesized name-only from the legacy
    /// [`tool_calls`] list. Scorers use this — never `events`, whose
    /// producer-shaped walking stays quarantined in the adapters.
    ///
    /// [`trajectory`]: Transcript::trajectory
    /// [`tool_calls`]: Transcript::tool_calls
    pub fn tool_invocations(&self) -> Vec<ToolInvocation<'_>> {
        let Some(trajectory) = &self.trajectory else {
            return self
                .tool_calls
                .iter()
                .map(|name| ToolInvocation {
                    name,
                    arguments: None,
                    result: None,
                })
                .collect();
        };
        let mut out = Vec::new();
        for step in &trajectory.steps {
            for call in &step.tool_calls {
                let result = step
                    .observation
                    .as_ref()
                    .and_then(|o| {
                        o.results
                            .iter()
                            .find(|r| r.source_call_id.as_deref() == Some(&call.tool_call_id))
                    })
                    .and_then(|r| r.content.as_ref());
                out.push(ToolInvocation {
                    name: &call.function_name,
                    arguments: Some(&call.arguments),
                    result,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The conformance fixture (also run by both SDKs); the RFC's worked
    /// example lives there, so these unit tests reuse it instead of inlining a
    /// second copy.
    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schema/v1/conformance/trajectory.json"
    ));

    fn rfc_example() -> serde_json::Value {
        let doc: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "rfc worked example")
            .expect("fixture carries the RFC worked example")["trajectory"]
            .clone()
    }

    #[test]
    fn schema_version_gate() {
        assert!(is_supported_schema_version("ATIF-v1.7"));
        assert!(is_supported_schema_version("ATIF-v1.0"));
        assert!(is_supported_schema_version("ATIF-v1.99")); // future v1 minor
        assert!(!is_supported_schema_version("ATIF-v2.0"));
        assert!(!is_supported_schema_version("v1.7"));
        assert!(!is_supported_schema_version(""));
    }

    #[test]
    fn rfc_example_parses_and_round_trips() {
        let t = Trajectory::from_value(rfc_example()).unwrap();
        assert_eq!(t.schema_version, "ATIF-v1.5"); // older v1 minor: accepted
        assert_eq!(t.agent.name, "harbor-agent");
        assert_eq!(t.steps.len(), 3);
        assert_eq!(t.steps[0].source, StepSource::User);
        assert_eq!(t.steps[1].tool_calls.len(), 2);
        assert_eq!(
            t.steps[1].tool_calls[0].arguments,
            json!({"ticker": "GOOGL", "metric": "price"})
        );
        // Opaque carries: token ids, logprobs, metrics.extra.reasoning_tokens.
        let m3 = t.steps[2].metrics.as_ref().unwrap();
        assert_eq!(m3.completion_token_ids.as_ref().unwrap().len(), 37);
        assert_eq!(m3.logprobs.as_ref().unwrap().len(), 44);
        assert_eq!(extra_u64(&m3.extra, "reasoning_tokens"), 12);

        // Round-trip: serialize and re-parse to the same value.
        let back = Trajectory::from_json(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn rfc_example_projects_flat_fields() {
        let t = Trajectory::from_value(rfc_example()).unwrap();
        let transcript = Transcript::from_trajectory(t);
        assert!(
            transcript
                .final_response
                .starts_with("As of October 11, 2025")
        );
        assert_eq!(
            transcript.tool_calls,
            vec!["financial_search", "financial_search"]
        );
        assert_eq!(transcript.tool_calls_count, 2);
        assert_eq!(transcript.iterations, 2);
        // Usage from final_metrics (not the per-step sum).
        assert_eq!(transcript.usage.input_tokens, 1120);
        assert_eq!(transcript.usage.output_tokens, 124);
        assert_eq!(transcript.usage.cache_read_tokens, 200);
        assert!((transcript.usage.cost_usd - 0.00078).abs() < 1e-12);
    }

    #[test]
    fn unknown_fields_and_sources_are_tolerated() {
        // Forward compat: a future ATIF v1 minor adds fields and a new source
        // kind; this build must parse it, not reject it.
        let json = json!({
            "schema_version": "ATIF-v1.42",
            "agent": {"name": "a", "version": "1", "future_agent_field": true},
            "steps": [
                {"step_id": 1, "source": "environment", "message": "hi",
                 "future_step_field": {"x": 1}},
                {"step_id": 2, "source": "agent", "message": "ok"}
            ],
            "brand_new_root_field": [1, 2, 3]
        });
        let t = Trajectory::from_value(json).unwrap();
        assert_eq!(t.steps[0].source, StepSource::Other("environment".into()));
        // The unknown source round-trips verbatim.
        let line = serde_json::to_string(&t).unwrap();
        assert!(line.contains(r#""source":"environment""#));
    }

    #[test]
    fn non_v1_schema_version_is_rejected_gracefully() {
        let doc = json!({
            "schema_version": "ATIF-v2.0",
            "agent": {"name": "a", "version": "1"},
            "steps": []
        });
        let err = Trajectory::from_value(doc).unwrap_err();
        assert!(err.contains("ATIF-v2.0"), "got: {err}");
        // Malformed JSON errors too (never panics).
        assert!(Trajectory::from_json("{not json").is_err());
        assert!(Trajectory::from_json(r#"{"schema_version": 7}"#).is_err());
    }

    #[test]
    fn extra_maps_are_preserved() {
        let mut t = Trajectory::new(Agent::new("a", "1"));
        t.extra.insert("harness".into(), json!({"run": 3}));
        let mut step = Step::new(1, StepSource::Agent, "done");
        step.extra.insert("note".into(), json!("custom"));
        t.steps.push(step);

        let back = Trajectory::from_json(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.extra["harness"]["run"], json!(3));
        assert_eq!(back.steps[0].extra["note"], json!("custom"));
        assert_eq!(back, t);
    }

    #[test]
    fn multimodal_message_projects_text_parts() {
        let content = StepContent::Parts(vec![
            ContentPart::text("a cat"),
            ContentPart::image("image/png", "images/cat.png"),
            ContentPart::text("on a mat"),
        ]);
        assert_eq!(content.text(), "a cat\non a mat");
        // Untagged on the wire: a string parses as Text, an array as Parts.
        let s: StepContent = serde_json::from_str(r#""plain""#).unwrap();
        assert_eq!(s, StepContent::Text("plain".into()));
        let p: StepContent = serde_json::from_str(r#"[{"type": "text", "text": "hi"}]"#).unwrap();
        assert_eq!(p, StepContent::Parts(vec![ContentPart::text("hi")]));
    }

    #[test]
    fn usage_sums_step_metrics_when_no_final_metrics() {
        let mut t = Trajectory::new(Agent::new("a", "1"));
        let mut s1 = Step::new(1, StepSource::Agent, "one");
        s1.metrics = Some(StepMetrics {
            prompt_tokens: Some(100),
            completion_tokens: Some(20),
            cached_tokens: Some(10),
            cost_usd: Some(0.001),
            extra: Metadata::from([("reasoning_tokens".into(), json!(5))]),
            ..Default::default()
        });
        let mut s2 = Step::new(2, StepSource::Agent, "two");
        s2.metrics = Some(StepMetrics {
            prompt_tokens: Some(200),
            completion_tokens: Some(30),
            cost_usd: Some(0.002),
            ..Default::default()
        });
        t.steps = vec![s1, s2];

        let usage = t.usage();
        assert_eq!(usage.input_tokens, 300);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 10);
        assert_eq!(usage.reasoning_tokens, 5);
        assert!((usage.cost_usd - 0.003).abs() < 1e-12);
    }

    #[test]
    fn iterations_exclude_deterministic_dispatch_steps() {
        let mut t = Trajectory::new(Agent::new("a", "1"));
        t.steps = vec![
            Step::new(1, StepSource::User, "go"),
            Step::new(2, StepSource::Agent, "inferring"), // None: counts
            {
                let mut s = Step::new(3, StepSource::Agent, "");
                s.llm_call_count = Some(0); // deterministic dispatch: excluded
                s
            },
            {
                let mut s = Step::new(4, StepSource::Agent, "done");
                s.llm_call_count = Some(2); // aggregated inference: counts
                s
            },
        ];
        assert_eq!(t.agent_iterations(), 2);
    }

    #[test]
    fn project_into_fills_defaults_but_never_overwrites() {
        let mut t = Trajectory::new(Agent::new("a", "1"));
        let mut step = Step::new(1, StepSource::Agent, "derived response");
        step.tool_calls = vec![ToolCall::new("c1", "grep", json!({"q": "x"}))];
        t.steps.push(step);

        // Defaults are filled…
        let mut fresh = Transcript::default();
        t.project_into(&mut fresh);
        assert_eq!(fresh.final_response, "derived response");
        assert_eq!(fresh.tool_calls, vec!["grep"]);
        assert_eq!(fresh.tool_calls_count, 1);
        assert_eq!(fresh.iterations, 1);

        // …but a field the subject set explicitly is never overwritten.
        let mut set = Transcript::response("explicit answer");
        set.iterations = 7;
        set.usage.input_tokens = 9;
        t.project_into(&mut set);
        assert_eq!(set.final_response, "explicit answer");
        assert_eq!(set.iterations, 7);
        assert_eq!(set.usage.input_tokens, 9);
        // The still-default fields were filled alongside.
        assert_eq!(set.tool_calls, vec!["grep"]);
    }

    #[test]
    fn from_trajectory_needs_no_client_calls() {
        // The zero-burden path: hand over a trajectory, get a fully projected
        // transcript — nothing else to call.
        let mut t = Trajectory::new(Agent::new("a", "1"));
        let mut step = Step::new(1, StepSource::Agent, "hi there");
        step.tool_calls = vec![ToolCall::new("c1", "search", json!({}))];
        t.steps.push(step);

        let transcript = Transcript::from_trajectory(t.clone());
        assert_eq!(transcript.final_response, "hi there");
        assert_eq!(transcript.tool_calls, vec!["search"]);
        assert_eq!(transcript.trajectory, Some(t));
        // `events` stays empty — it is independent, never required alongside.
        assert!(transcript.events.is_empty());
    }

    #[test]
    fn tool_invocations_prefer_trajectory_then_fall_back_to_names() {
        // ATIF-first: names + arguments + observation joined on source_call_id.
        let mut t = Trajectory::new(Agent::new("a", "1"));
        let mut step = Step::new(1, StepSource::Agent, "");
        step.tool_calls = vec![
            ToolCall::new("c1", "search", json!({"q": "price"})),
            ToolCall::new("c2", "fetch", json!({"url": "u"})),
        ];
        step.observation = Some(Observation {
            results: vec![ObservationResult {
                source_call_id: Some("c1".into()),
                content: Some("$185.35".into()),
                ..Default::default()
            }],
        });
        t.steps.push(step);
        let transcript = Transcript::from_trajectory(t);

        let calls = transcript.tool_invocations();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments.unwrap()["q"], "price");
        assert_eq!(calls[0].result.unwrap().text(), "$185.35");
        assert_eq!(calls[1].name, "fetch");
        assert!(calls[1].result.is_none()); // no correlated observation

        // Legacy fallback: names only, from the flat tool_calls list.
        let legacy = Transcript {
            tool_calls: vec!["read".into(), "calc".into()],
            ..Default::default()
        };
        let calls = legacy.tool_invocations();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read");
        assert!(calls[0].arguments.is_none());
        assert!(calls[0].result.is_none());
    }

    #[test]
    fn subagent_trajectories_round_trip_but_stay_opaque_to_projections() {
        let mut sub = Trajectory::new(Agent::new("sub", "1"));
        sub.trajectory_id = Some("sub-1".into());
        let mut sub_step = Step::new(1, StepSource::Agent, "sub work");
        sub_step.tool_calls = vec![ToolCall::new("s1", "sub_tool", json!({}))];
        sub.steps.push(sub_step);

        let mut t = Trajectory::new(Agent::new("parent", "1"));
        t.steps.push(Step::new(1, StepSource::Agent, "delegated"));
        t.subagent_trajectories.push(sub);

        let back = Trajectory::from_json(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
        // Projections count only the top level; subagent tool calls stay out.
        let transcript = Transcript::from_trajectory(t);
        assert!(transcript.tool_calls.is_empty());
        assert_eq!(transcript.iterations, 1);
    }
}
