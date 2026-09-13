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
//! wire — the host addresses targets by *label*.
//!
//! ## Framing
//! One JSON object per line, classified by **fields** (not by which pipe it
//! arrived on): a line bearing `method` is a [`Request`] (with `id`) or a
//! [`Notification`] (no `id`); a line without `method` is a [`Response`], routed
//! by `id`. Today [`Request`]s flow host→study and [`Response`]/[`Notification`]
//! flow study→host. Classifying on `method` rather than direction is deliberate:
//! it keeps a future **reverse** request (study→host) an additive, minor change
//! rather than a breaking one (see the reverse-channel seam in
//! `docs/protocol.md`).
//!
//! ## Methods
//! * `initialize` → [`InitializeResult`]
//! * `list` → [`ListResult`] — the eval catalogue, with the **first page** of
//!   each eval's samples inline
//! * `list_samples` ([`ListSamplesParams`]) → [`ListSamplesResult`] — the next
//!   page of one eval's samples, for datasets too large (or too lazy) to
//!   enumerate in one `list` (advertised by the `paginate` capability)
//! * `run` ([`RunParams`]) → [`RunResult`] — execute + score in one call
//! * `execute` ([`RunParams`]) → [`ExecuteResult`] — execute the subject only,
//!   returning the **full** transcript (for run-now, score-later)
//! * `score` ([`ScoreParams`]) → [`RunResult`] — score a supplied transcript
//!   (for deferred scoring and re-scoring)
//! * `cancel` ([`CancelParams`]) → [`CancelResult`] — abort one in-flight `run`
//!   /`execute`/`score` by its request `id` (for per-case timeouts, cost caps,
//!   fail-fast)
//!
//! See `docs/protocol.md` for the full reference.

use serde::{Deserialize, Serialize};

use crate::{Metadata, Params, Score, Timing, Transcript, Usage};

/// The protocol version advertised by `initialize`, as `MAJOR.MINOR`.
///
/// **Compatibility contract** (so old and new peers interoperate):
/// * The **major** version changes only on a breaking wire change. Peers with
///   different majors are incompatible — [`version_compatible`] returns false
///   and the host warns. A peer older than [`MIN_PROTOCOL_VERSION`] is refused
///   for the same reason.
/// * The **minor** version increments for backwards-compatible additions (new
///   methods, new optional fields). A newer peer talking to an older one must
///   tolerate missing additions; an older peer must ignore unknown fields.
///
/// Every payload struct here is *non-exhaustive on the wire*: unknown fields are
/// ignored (no `deny_unknown_fields`) and new fields are `#[serde(default)]`, so
/// adding a field is a minor, non-breaking change.
///
/// The `1.0` baseline carries the full method set (`initialize`, `list`,
/// `list_samples`, `run`, `execute`, `score`, `cancel`), typed and
/// `request_id`-correlated `event`/`log` notifications
/// ([`EventParams`]/[`LogParams`]), JSON-RPC-shaped [`RpcError`]s, trials/seed
/// repetitions (pass@k / variance), cursor-paginated sample listing,
/// eval/sample/target `metadata` (open-ended JSON), and multimodal `output` plus
/// structured `capability_params`.
///
/// **`1.1`** (current) adds the structured **ATIF trajectory**: one optional
/// field (`Transcript::trajectory`, riding `execute` results and `score`
/// params) plus the [`capabilities::TRAJECTORY`] token — the primary structured
/// trajectory contract (see [`crate::trajectory`]). Additive per the contract
/// above: a `1.0` peer ignores the unknown field and token and keeps
/// interoperating. A later addition bumps the **minor** and must stay additive
/// (a new optional field, method, or capability token); a breaking wire change
/// bumps the **major**.
pub const PROTOCOL_VERSION: &str = "1.1";

/// The oldest protocol version this build can still talk to.
pub const MIN_PROTOCOL_VERSION: &str = "1.0";

/// What this build implements and the oldest peer it accepts, as one value.
///
/// Backed by [`lanok_core::Negotiation`], which is the shared implementation of
/// the same `MAJOR.MINOR` contract the yolop extension protocol follows. Using
/// it here means the rule is written once rather than reimplemented per
/// protocol, and it is what makes [`MIN_PROTOCOL_VERSION`] load-bearing:
/// mira published that constant in `meta.json` but never checked it.
pub fn negotiation() -> lanok_core::Negotiation {
    declaration::NEGOTIATION
}

/// The major component of a `MAJOR.MINOR` version string (0 if malformed).
pub fn version_major(v: &str) -> u32 {
    v.parse::<lanok_core::Version>()
        .map(|version| version.major)
        .unwrap_or(0)
}

/// Whether this build can talk to a peer advertising version `other`.
///
/// Same major, and not older than [`MIN_PROTOCOL_VERSION`]. The minimum used to
/// be advertised and ignored: a study announcing a version this build had
/// dropped support for was accepted anyway, and failed later at whichever
/// method it could not satisfy. A malformed version is refused rather than
/// treated as major `0`, which previously made `"not-a-version"` merely
/// incompatible instead of invalid.
pub fn version_compatible(other: &str) -> bool {
    match other.parse::<lanok_core::Version>() {
        Ok(peer) => negotiation().accepts(peer).is_ok(),
        Err(_) => false,
    }
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
        Self::err_with(id, RpcError::internal(message))
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

/// A JSON-RPC error object, re-exported from [`lanok_core`].
///
/// Lanok implements exactly the shape this protocol already used: `code`,
/// `message`, a top-level `retryable` hint omitted when false, and optional
/// `data`, with everything beyond `message` defaulted so a peer sending bare
/// `{ "message": "…" }` still parses. The wire is unchanged. The definition
/// simply stopped existing twice, here and in the yolop extension protocol.
pub use lanok_core::RpcError;

/// Error codes, from [`lanok_core`].
///
/// The three mira defined are the JSON-RPC reserved values and keep their
/// meanings. Lanok adds the rest outside the reserved range: cancelled,
/// timeout, capability-unsupported, version-incompatible, transport-closed.
pub use lanok_core::codes;

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

impl Notification {
    /// The `method` of a progress `event` notification, from the declaration.
    pub const EVENT: &'static str = method::EVENT;
    /// The `method` of a free-form `log` notification, from the declaration.
    pub const LOG: &'static str = method::LOG;

    /// Build a typed `event` progress notification.
    pub fn event(params: EventParams) -> Self {
        Self {
            method: Self::EVENT.into(),
            // Infallible for a plain struct of scalars/strings; `expect` keeps a
            // serialization bug loud rather than silently emitting `params: null`.
            params: serde_json::to_value(params).expect("EventParams serializes"),
        }
    }

    /// Build a `log` notification. `request_id` is the request being serviced
    /// (0 for connection-level logs, e.g. a malformed request line).
    pub fn log(message: impl Into<String>, request_id: u64) -> Self {
        Self {
            method: Self::LOG.into(),
            params: serde_json::to_value(LogParams {
                message: message.into(),
                request_id,
            })
            .expect("LogParams serializes"),
        }
    }

    /// Parse the payload as [`EventParams`], if this is an `event` notification.
    pub fn as_event(&self) -> Option<EventParams> {
        (self.method == Self::EVENT)
            .then(|| serde_json::from_value(self.params.clone()).ok())
            .flatten()
    }

    /// Parse the payload as [`LogParams`], if this is a `log` notification.
    pub fn as_log(&self) -> Option<LogParams> {
        (self.method == Self::LOG)
            .then(|| serde_json::from_value(self.params.clone()).ok())
            .flatten()
    }
}

/// Typed payload of an `event` progress [`Notification`] for one in-flight run.
///
/// Correlated to its originating `run`/`execute` request by `request_id` — the
/// same demultiplexing key responses use — so a host can bind progress to a
/// specific call even when many cases (including repeated trials of one case)
/// are multiplexed over the single pipe.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventParams {
    /// The `id` of the `run`/`execute` request this event belongs to. Defaulted
    /// to 0 ("uncorrelated") so a study that omits it still parses.
    #[serde(default)]
    pub request_id: u64,
    pub eval: String,
    pub sample: String,
    pub target: String,
    /// Extra matrix-axis values for the case (empty for a target-only matrix).
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// One of the [`event`] kinds. A future kind an older host doesn't recognise
    /// is carried through verbatim (forward-compat), not rejected.
    pub kind: String,
    /// Reasoning-turn index, for an [`event::TURN`] event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<usize>,
    /// Tool name, for an [`event::TOOL_CALL`] event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Streamed output delta, for an [`event::OUTPUT`] event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Typed payload of a `log` [`Notification`] — a free-form diagnostic line.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LogParams {
    pub message: String,
    /// The request this log relates to (0 for connection-level logs). Same
    /// `#[serde(default)]` treatment as `EventParams::request_id`, so both carry a
    /// consistent `default: 0` in the generated schema (no `skip_serializing_if`,
    /// which would suppress the schema default and desync SDK generators).
    #[serde(default)]
    pub request_id: u64,
}

// ----- method payloads ------------------------------------------------------

/// `initialize` params: what the host tells the study about itself.
///
/// Both fields are defaulted: a study SDK calling `handle("initialize", {})`
/// (which the Python and TypeScript conformance suites do) must still parse.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InitializeParams {
    /// The protocol version the host speaks, as `MAJOR.MINOR`.
    #[serde(default)]
    pub protocol_version: String,
    /// The host's name, for the study's diagnostics.
    #[serde(default)]
    pub host: String,
}

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
    /// Structured **config** for capabilities, keyed by capability token — the
    /// data a bare [`capabilities`](Self::capabilities) string can't carry (which
    /// event kinds the study emits, the input/output modalities it supports, a
    /// concurrency hint, …). Open-vocabulary like `metadata`, so new keys need no
    /// version bump; a host reads it additively, falling back to today's
    /// behaviour when a token is absent.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub capability_params: Metadata,
}

impl InitializeResult {
    /// The structured config a study advertised for `capability`, if any (see
    /// [`capability_params`](Self::capability_params)).
    pub fn capability_param(&self, capability: &str) -> Option<&serde_json::Value> {
        self.capability_params.get(capability)
    }
}

/// The eval protocol, declared once.
///
/// Everything below this point, method names, directions, which are requests
/// and which are notifications, the capability tokens and what each promises,
/// used to be written twice: as string literals in [`crate::host`]'s call sites
/// and again as match arms in [`crate::study`]'s dispatch, with nothing
/// checking that the two lists agreed. The declaration is now the single
/// source, and both sides are generated from it.
///
/// It lives in a private module because two of the constants the macro emits
/// (`PROTOCOL_VERSION`, `MIN_PROTOCOL_VERSION`) are typed
/// [`lanok::Version`]s, while mira publishes the same two as strings in
/// `meta.json` and across its SDKs. The strings above stay the public spelling;
/// `versions_agree` below is the guard that keeps them equal.
///
/// # Reserved (not yet declared)
/// `host_requests` is the negotiation handle for the **reverse channel**, a
/// study→host request direction (host-brokered model access, shared resources,
/// human-in-the-loop). It is *reserved*, not declared: no token is minted and
/// it is absent from `meta.json` until the channel lands (then as a minor
/// bump). Declaring it is additive when the time comes, because a method's
/// direction is a property of the method rather than of the process: see the
/// reverse-channel seam in `docs/protocol.md`.
mod declaration {
    use super::{
        CancelParams, CancelResult, EventParams, ExecuteResult, InitializeParams, InitializeResult,
        ListResult, ListSamplesParams, ListSamplesResult, LogParams, RunParams, RunResult,
        ScoreParams,
    };

    lanok::protocol! {
        name    = "mira";
        version = "1.1";
        min     = "1.0";

        /// Announce the host and learn what the study is and can do.
        initiator fn initialize(InitializeParams) -> InitializeResult;

        /// The eval catalogue, with the first page of each eval's samples
        /// inline.
        initiator fn list() -> ListResult;

        /// The next page of one eval's samples, for datasets too large (or too
        /// lazy) to enumerate in one `list`.
        initiator fn list_samples(ListSamplesParams) -> ListSamplesResult
            requires "paginate";

        /// Execute and score one matrix case in a single call.
        initiator fn run(RunParams) -> RunResult;

        /// Execute one case's subject without scoring, returning the full
        /// transcript, for run-now-score-later workflows.
        initiator fn execute(RunParams) -> ExecuteResult requires "execute";

        /// Score a supplied transcript without re-executing the subject, for
        /// deferred scoring and re-scoring.
        initiator fn score(ScoreParams) -> RunResult requires "score";

        /// Abort one in-flight `run`/`execute`/`score` by its request `id`.
        ///
        /// A request rather than a notification, and acknowledged: the host
        /// learns whether the run was still in flight. That divergence from
        /// other protocols, where cancel is fire-and-forget, is why lanok's
        /// cancellation is a hook the protocol fills rather than a setting.
        initiator fn cancel(CancelParams) -> CancelResult requires "cancel";

        /// Live progress for one in-flight run, correlated to its request by
        /// `request_id`.
        responder notify event(EventParams) requires "events";

        /// Free-form study output, for the host's log pane.
        responder notify log(LogParams);

        capabilities {
            /// Study advertises extra matrix axes in `list` and honours
            /// `run.params`.
            axes,
            /// Study emits `event` progress notifications during `run`.
            events,
            /// Study reports token/cost usage and timing in transcripts.
            usage,
            /// Study answers `execute` (run the subject only, returning a full
            /// transcript) for run-now-score-later workflows.
            execute,
            /// Study answers `score` (run scorers over a supplied transcript)
            /// for deferred scoring and re-scoring of stored transcripts.
            score,
            /// Study honours the `trial`/`seed` run params: it threads the seed
            /// into the subject so repetitions are reproducible. Trials run
            /// regardless (the host drives the repetition); this advertises
            /// that seeding actually takes effect, not just that the case is
            /// re-run.
            trials,
            /// Study answers `cancel` (abort one in-flight run by its request
            /// `id`). Without it, a host can only stop work by closing stdin,
            /// which ends every in-flight run at once.
            cancel,
            /// Study answers `list_samples` and may return a non-empty
            /// `EvalInfo.next_cursor` from `list`, so the host pages large or
            /// lazily generated sample sets instead of receiving them all in
            /// one `list`.
            paginate,
            /// Study attaches a structured ATIF trajectory to transcripts
            /// (`Transcript::trajectory` on `execute` results / `score`
            /// params), and its scorers can grade trajectory structure. The
            /// format/version pair rides `capability_params`
            /// (`{"trajectory": {"format": "ATIF", "version": "1.7"}}`), so a
            /// non-ATIF or ATIF-v2 representation needs no new token. See
            /// [`crate::trajectory`].
            trajectory,
        }
    }
}

/// The protocol's vocabulary as data: methods, directions, capability tokens.
pub use declaration::META;
/// Wire method names, so no call site spells one as a string literal.
pub use declaration::method;
/// Typed stubs for the host side, implemented for [`lanok::Peer`].
pub use declaration::{InitiatorApi, InitiatorDispatch, InitiatorHandler};
/// Typed handlers and dispatch for the study side.
pub use declaration::{ResponderDispatch, ResponderHandler};

/// Capability tokens a study may advertise in [`InitializeResult::capabilities`],
/// generated from the `capabilities` block of the declaration above.
pub use declaration::capability as capabilities;

/// Defined `event` notification kinds — the value of [`EventParams::kind`].
///
/// Like [`capabilities`], this is an **open, growing** token vocabulary, not a
/// closed Rust enum: an older host must carry an unrecognised *future* kind
/// through rather than fail to parse it (forward-compat contract #1), so `kind`
/// stays a `String` and these constants name the stable vocabulary without
/// freezing it. (Contrast [`crate::ErrorKind`], a closed two-state enum.)
pub mod event {
    /// A case's run has begun. Emitted once, before any other event for the case.
    pub const STARTED: &str = "started";
    /// A reasoning turn / iteration started; [`super::EventParams::turn`] carries
    /// its index.
    pub const TURN: &str = "turn";
    /// A tool was invoked; [`super::EventParams::tool`] carries its name.
    pub const TOOL_CALL: &str = "tool_call";
    /// A chunk of streamed output; [`super::EventParams::text`] carries the delta.
    pub const OUTPUT: &str = "output";
    /// The case's run finished (success or error). Emitted once, last.
    pub const FINISHED: &str = "finished";

    /// Every defined kind, for the machine-readable `meta.json` index.
    pub const ALL: &[&str] = &[STARTED, TURN, TOOL_CALL, OUTPUT, FINISHED];
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SampleInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Free-form sample provenance (repo, difficulty, dataset split, …). Mirrors
    /// `Sample::metadata` onto the wire so the host can group reports by it
    /// (`--group-by`). Defaulted/omitted when empty, so an older study still parses.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TargetInfo {
    pub label: String,
    /// Provider id (e.g. `sim`, `anthropic`, `openai`). Lets the host bucket
    /// concurrency per provider so one provider's rate limits can't be flooded.
    /// Defaulted (empty) so an older/foreign study that omits it still parses;
    /// such cases share the empty-provider bucket.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// False when a real provider's API key is absent in the study's env.
    pub available: bool,
    /// Free-form per-target config that rides the target column: agent, underlying
    /// model, effort, price, sandbox, observability links, … Mirrors
    /// `Target::metadata` onto the wire so the host can surface and group by
    /// it. Defaulted/omitted when empty, so an older study still parses.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
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
/// `samples × targets` grid and apply selection without running anything.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EvalInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The **first page** of this eval's samples. When `next_cursor` is present,
    /// more pages follow — fetch them with `list_samples` (see
    /// [`ListSamplesParams`]) until the cursor runs out. A study that fits its
    /// whole dataset here omits `next_cursor`.
    pub samples: Vec<SampleInfo>,
    /// Opaque continuation token: present iff more samples remain beyond
    /// `samples`. Pass it back via `list_samples` to fetch the next page. The
    /// host treats it as an opaque blob — only the study interprets it.
    /// Defaulted/omitted, so an older study that sends all samples inline (and a
    /// host that ignores pagination) interoperate unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub scorers: Vec<String>,
    pub targets: Vec<TargetInfo>,
    /// Extra matrix axes beyond the target. Defaulted so older servers that omit
    /// the field still parse (forward compatibility).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<AxisInfo>,
    /// Defaulted so a foreign/older study that omits it still parses, per the
    /// protocol's forward-compatibility contract (see docs/protocol.md).
    #[serde(default)]
    pub max_turns: usize,
    /// How many times each case of this eval should be run (trials/repetitions),
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

/// `list_samples` params: ask for one more page of `eval`'s samples, continuing
/// from the opaque `cursor` last handed back (in [`EvalInfo::next_cursor`] or a
/// prior [`ListSamplesResult::next_cursor`]). Only studies advertising the
/// `paginate` capability answer this.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListSamplesParams {
    pub eval: String,
    /// Opaque token from the previous page. The host echoes it verbatim.
    pub cursor: String,
}

/// `list_samples` result: one page of samples plus the cursor for the page after
/// it (`None` once the dataset is exhausted).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListSamplesResult {
    pub samples: Vec<SampleInfo>,
    /// Opaque token for the next page, or `None` when this was the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// `run` params: address one matrix case by `(eval, sample, target label)` plus
/// any extra axis `params` (axis name → chosen value).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunParams {
    pub eval: String,
    pub sample: String,
    pub target: String,
    /// Chosen value per extra matrix axis. Empty/omitted for a target-only
    /// matrix; defaulted so older hosts/servers interoperate.
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index when this case is being repeated; `0` for a single
    /// run. Defaulted so older hosts/studies interoperate.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials planned for this case (`0`/`1` = single run). Lets the study
    /// echo the case's trial identity back so its key matches the host's plan.
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

/// `cancel` params: the `id` of the in-flight `run`/`execute`/`score` [`Request`]
/// to abort. It is the request's own `id` (the one the host assigned and awaits a
/// response on), not a case key — so a host can cancel a specific outstanding call
/// even when several runs of the same case are in flight.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CancelParams {
    pub id: u64,
}

/// `cancel` result: whether a matching in-flight request was found and aborted.
/// `false` is normal and benign — the targeted request had already completed (or
/// was never in flight) by the time the cancel arrived. Cancellation is therefore
/// best-effort: a `run` that finishes first still returns its real result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CancelResult {
    pub cancelled: bool,
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
    /// infra-errored cases. Defaulted/omitted for the common subject case.
    #[serde(default, skip_serializing_if = "crate::ErrorKind::is_subject")]
    pub error_kind: crate::ErrorKind,
    /// The run's multimodal output parts (see
    /// [`Transcript::output`](crate::Transcript::output)), carried in the
    /// lightweight summary so results/checkpoints retain the non-text modalities,
    /// not just `final_response`. Empty for the common text-only case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<crate::Part>,
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
            output: t.output.clone(),
            // No source on the core `Transcript` yet — left unset until promoted.
            #[cfg(feature = "protocol-unstable")]
            experimental: None,
        }
    }
}

/// `execute` result for one case: the **full** [`Transcript`] (raw events and
/// captured files included), so the host can persist it as an execution
/// artifact and `score` it later. Distinct from [`RunResult`], which carries the
/// lightweight [`TranscriptSummary`] plus scores.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExecuteResult {
    pub eval: String,
    pub sample: String,
    pub target: String,
    /// Extra matrix-axis values for this case (empty for a target-only matrix).
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index when this case is repeated (see [`RunParams::trial`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials for this case (`0`/`1` = single run). Part of the case key
    /// when `> 1`, so trial artifacts stay distinct.
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed this transcript was produced with, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The complete transcript, unlike the summary carried in [`RunResult`].
    pub transcript: Transcript,
    /// True when the case was not executed (e.g. target unavailable).
    #[serde(default)]
    pub skipped: bool,
}

impl ExecuteResult {
    /// Stable case identity (see [`RunResult::key`]), trial-aware.
    pub fn key(&self) -> String {
        format!(
            "{}{}",
            crate::case_key(&self.eval, &self.sample, &self.target, &self.params),
            crate::trial_suffix(self.trial, self.trials),
        )
    }
}

/// `score` params: a case identity plus the full [`Transcript`] to score. The
/// transcript travels over the wire so the host can replay a stored one — the
/// study scores it without re-running the subject.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScoreParams {
    pub eval: String,
    pub sample: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index for the case this transcript came from (echoed into
    /// the resulting [`RunResult`] so it keeps its trial identity on re-score).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials for this case (`0`/`1` = single run).
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed this transcript was produced with, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub transcript: Transcript,
}

/// `run` result for one case. Also the unit persisted in checkpoints.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunResult {
    pub eval: String,
    pub sample: String,
    pub target: String,
    /// Extra matrix-axis values for this case (empty for a target-only matrix).
    #[serde(default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// 0-based trial index when this case is repeated (see [`RunParams::trial`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trial: usize,
    /// Total trials for this case (`0`/`1` = single run). Part of the case key
    /// when `> 1`; the host groups results by their *logical* key (without the
    /// trial suffix) to aggregate pass@k / variance (see [`crate::aggregate`]).
    #[serde(default, skip_serializing_if = "is_single_trial")]
    pub trials: usize,
    /// Per-trial seed this result was produced with, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The sample's input turns (the prompt sent), copied through so a persisted
    /// result is self-describing without the dataset. Empty when unavailable
    /// (e.g. a synthetic error result).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    /// The sample's expected/reference value, when the dataset provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<serde_json::Value>,
    pub passed: bool,
    pub aggregate: f64,
    pub scores: Vec<Score>,
    pub transcript: TranscriptSummary,
    /// True when the case was not executed (e.g. target unavailable).
    #[serde(default)]
    pub skipped: bool,
}

impl RunResult {
    /// Stable case identity: `eval/sample@target` (with an `[k=v,…]` axis suffix
    /// and a `#trial` suffix when this case is repeated). Used for selection,
    /// dedupe, and checkpoint resume.
    pub fn key(&self) -> String {
        format!(
            "{}{}",
            self.logical_key(),
            crate::trial_suffix(self.trial, self.trials)
        )
    }

    /// The case identity **without** the `#trial` suffix — the key all trials of
    /// one case share, so [`crate::aggregate`] can group them.
    pub fn logical_key(&self) -> String {
        crate::case_key(&self.eval, &self.sample, &self.target, &self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declaration owns the version; these two strings are the published
    /// spelling of it (`meta.json`, the Python and TypeScript SDKs). They are
    /// separate values, so this is the guard that keeps them one fact.
    #[test]
    fn versions_agree_with_the_declaration() {
        assert_eq!(PROTOCOL_VERSION, declaration::PROTOCOL_VERSION.to_string());
        assert_eq!(
            MIN_PROTOCOL_VERSION,
            declaration::MIN_PROTOCOL_VERSION.to_string()
        );
    }

    /// The method list is what the SDKs, the conformance vectors and
    /// `meta.json` are all built against, so a method added to the declaration
    /// without being published is worth catching here rather than in a study.
    #[test]
    fn the_declaration_carries_the_whole_method_surface() {
        let declared: Vec<&str> = META.methods.iter().map(|m| m.name).collect();
        assert_eq!(
            declared,
            [
                "initialize",
                "list",
                "list_samples",
                "run",
                "execute",
                "score",
                "cancel",
                "event",
                "log",
            ]
        );

        // Directions are the point: everything the host calls is `initiator`,
        // and the two notifications come back the other way. A reverse request
        // would be an additive `responder fn`, not a redesign.
        use lanok::{Direction, MethodKind};
        for name in ["initialize", "list", "run", "cancel"] {
            let method = META.method(name).unwrap();
            assert_eq!(method.direction, Direction::Initiator);
            assert_eq!(method.kind, MethodKind::Request);
        }
        for name in ["event", "log"] {
            let method = META.method(name).unwrap();
            assert_eq!(method.direction, Direction::Responder);
            assert_eq!(method.kind, MethodKind::Notification);
        }
    }

    /// Gating is declared, not remembered at each call site. `run` and `list`
    /// are the base surface every study answers; the rest are opt-in.
    #[test]
    fn optional_methods_declare_the_capability_they_need() {
        assert_eq!(META.method("run").unwrap().requires, None);
        assert_eq!(META.method("list").unwrap().requires, None);
        assert_eq!(META.method("execute").unwrap().requires, Some("execute"));
        assert_eq!(META.method("score").unwrap().requires, Some("score"));
        assert_eq!(META.method("cancel").unwrap().requires, Some("cancel"));
        assert_eq!(
            META.method("list_samples").unwrap().requires,
            Some("paginate")
        );
        assert_eq!(META.method("event").unwrap().requires, Some("events"));
    }

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
    fn a_peer_older_than_the_minimum_is_refused() {
        // The regression this adoption fixes: MIN_PROTOCOL_VERSION was
        // published in meta.json and never checked, so a peer below it was
        // accepted and failed later at whichever method it could not satisfy.
        let build =
            lanok_core::Negotiation::with_min("1.5".parse().unwrap(), "1.2".parse().unwrap());
        assert!(build.accepts("1.2".parse().unwrap()).is_ok());
        assert!(
            build.accepts("1.9".parse().unwrap()).is_ok(),
            "a newer minor is additive"
        );
        assert_eq!(
            build.accepts("1.1".parse().unwrap()),
            Err(lanok_core::Incompatible::TooOld)
        );
        assert_eq!(
            build.accepts("2.0".parse().unwrap()),
            Err(lanok_core::Incompatible::MajorMismatch)
        );
    }

    #[test]
    fn a_malformed_version_is_invalid_rather_than_major_zero() {
        // Previously these parsed as major 0 and were merely "incompatible".
        assert!(!version_compatible("not-a-version"));
        assert!(!version_compatible("1"));
        assert!(!version_compatible("1.2.3"));
        assert_eq!(version_major("not-a-version"), 0);
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
    fn cancel_params_and_result_roundtrip() {
        let p = CancelParams { id: 42 };
        let line = serde_json::to_string(&p).unwrap();
        assert_eq!(line, r#"{"id":42}"#);
        let back: CancelParams = serde_json::from_value(serde_json::json!({ "id": 42 })).unwrap();
        assert_eq!(back.id, 42);

        let r = CancelResult { cancelled: true };
        let back: CancelResult = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(back.cancelled);
    }

    #[test]
    fn version_compatibility() {
        assert!(version_compatible(PROTOCOL_VERSION));
        // 1.1 (trajectory) is additive: a 1.0 study/host keeps interoperating.
        assert!(version_compatible(MIN_PROTOCOL_VERSION));
        assert!(version_compatible("1.0"));
        assert!(version_compatible("1.5")); // a future minor, same major
        assert!(!version_compatible("2.0")); // newer major
        assert!(!version_compatible("0.9")); // older major
        assert_eq!(version_major("1.4"), 1);
        assert_eq!(version_major("garbage"), 0);
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compat() {
        // A future study adds fields the host doesn't know — must still parse.
        let line = r#"{"protocol_version":"1.1","study":"x","evals":2,
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
            "scorers":["succeeded"],"targets":[{"label":"sim","available":true}]}"#;
        let info: EvalInfo = serde_json::from_str(line).unwrap();
        assert_eq!(info.max_turns, 0);
        assert!(info.axes.is_empty());
        assert_eq!(info.samples.len(), 1);
        // The per-sample / per-target metadata defaults to empty.
        assert!(info.samples[0].metadata.is_empty());
        assert!(info.targets[0].metadata.is_empty());
    }

    #[test]
    fn sample_and_model_metadata_omitted_when_empty() {
        // Forward-compat: a study that sets no sample/target metadata must omit
        // the `metadata` key entirely, so an older host reading it sees nothing
        // new. `skip_serializing_if` guarantees this.
        let sample = serde_json::to_string(&SampleInfo {
            id: "hi".into(),
            tags: vec![],
            metadata: Default::default(),
        })
        .unwrap();
        assert!(!sample.contains("metadata"), "got: {sample}");
        let target = serde_json::to_string(&TargetInfo {
            label: "sim".into(),
            provider: "sim".into(),
            available: true,
            metadata: Default::default(),
        })
        .unwrap();
        assert!(!target.contains("metadata"), "got: {target}");
    }

    #[test]
    fn sample_and_model_metadata_roundtrip() {
        let mut metadata = Metadata::new();
        metadata.insert("difficulty".into(), serde_json::json!("hard"));
        metadata.insert("retries".into(), serde_json::json!(3));
        let info = SampleInfo {
            id: "hi".into(),
            tags: vec!["smoke".into()],
            metadata: metadata.clone(),
        };
        let back: SampleInfo =
            serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
        assert_eq!(back.metadata.get("difficulty").unwrap(), "hard");
        assert_eq!(back.metadata.get("retries").unwrap(), &serde_json::json!(3));
    }

    #[test]
    fn event_notification_roundtrips_and_correlates() {
        let n = Notification::event(EventParams {
            request_id: 42,
            eval: "greet".into(),
            sample: "hi".into(),
            target: "sim".into(),
            kind: event::TOOL_CALL.into(),
            tool: Some("search".into()),
            ..Default::default()
        });
        // A notification never carries the envelope `id` — that classifies a
        // Response. The request id rides in the payload instead.
        let line = serde_json::to_string(&n).unwrap();
        assert!(!line.contains("\"id\""));
        assert!(serde_json::from_str::<Response>(&line).is_err());

        let back: Notification = serde_json::from_str(&line).unwrap();
        let ev = back.as_event().expect("parses as event");
        assert_eq!(ev.request_id, 42);
        assert_eq!(ev.kind, event::TOOL_CALL);
        assert_eq!(ev.tool.as_deref(), Some("search"));
        // A `log` is not an `event` and vice versa.
        assert!(back.as_log().is_none());
    }

    #[test]
    fn untyped_event_still_parses() {
        // A study may emit events with no `request_id` and no typed payload
        // fields. Forward-compat: the host must still parse them, defaulting
        // the correlation id to 0 ("uncorrelated").
        let line = r#"{"method":"event","params":
            {"eval":"greet","sample":"hi","target":"sim","kind":"started"}}"#;
        let n: Notification = serde_json::from_str(line).unwrap();
        let ev = n.as_event().expect("legacy event parses");
        assert_eq!(ev.request_id, 0);
        assert_eq!(ev.kind, "started");
    }

    #[test]
    fn log_notification_roundtrips() {
        let n = Notification::log("warming up", 7);
        let back: Notification = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        let log = back.as_log().expect("parses as log");
        assert_eq!(log.message, "warming up");
        assert_eq!(log.request_id, 7);

        // An untyped log (no request_id) still parses, id defaulting to 0.
        let legacy: Notification =
            serde_json::from_str(r#"{"method":"log","params":{"message":"hi"}}"#).unwrap();
        let log = legacy.as_log().unwrap();
        assert_eq!(log.message, "hi");
        assert_eq!(log.request_id, 0);
    }

    #[test]
    fn eval_info_without_cursor_omits_it_and_defaults_on_read() {
        // A study that fits all samples inline must not emit `next_cursor`, and
        // an older study that never knew the field still parses (defaults None).
        let info = EvalInfo {
            name: "greet".into(),
            description: String::new(),
            samples: vec![SampleInfo {
                id: "hi".into(),
                tags: vec![],
                metadata: Metadata::default(),
            }],
            next_cursor: None,
            scorers: vec![],
            targets: vec![],
            axes: vec![],
            max_turns: 0,
            trials: 0,
            seed: None,
            metadata: Metadata::default(),
        };
        let line = serde_json::to_string(&info).unwrap();
        assert!(!line.contains("next_cursor"));
        let minimal = r#"{"name":"greet","samples":[{"id":"hi"}],
            "scorers":[],"targets":[]}"#;
        let back: EvalInfo = serde_json::from_str(minimal).unwrap();
        assert!(back.next_cursor.is_none());
    }

    #[test]
    fn list_samples_roundtrips_with_and_without_next() {
        let params = ListSamplesParams {
            eval: "swe".into(),
            cursor: "500".into(),
        };
        let back: ListSamplesParams =
            serde_json::from_str(&serde_json::to_string(&params).unwrap()).unwrap();
        assert_eq!(back.eval, "swe");
        assert_eq!(back.cursor, "500");

        let last = ListSamplesResult {
            samples: vec![SampleInfo {
                id: "case-999".into(),
                tags: vec![],
                metadata: Metadata::default(),
            }],
            next_cursor: None,
        };
        let line = serde_json::to_string(&last).unwrap();
        assert!(!line.contains("next_cursor")); // omitted on the final page
        let more = ListSamplesResult {
            samples: vec![],
            next_cursor: Some("1000".into()),
        };
        let back: ListSamplesResult =
            serde_json::from_str(&serde_json::to_string(&more).unwrap()).unwrap();
        assert_eq!(back.next_cursor.as_deref(), Some("1000"));
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
        let err = RpcError::internal("provider 503")
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
        let plain = RpcError::internal("nope");
        assert!(!plain.retryable);
        assert!(!serde_json::to_string(&plain).unwrap().contains("data"));
    }

    #[test]
    fn the_error_wire_shape_is_what_studies_parse() {
        // Byte-level, because the Python and TypeScript study SDKs parse this
        // and neither knows lanok exists. The one difference from before the
        // adoption is recorded here deliberately: `retryable` used to be
        // written even when false, and is now omitted. Every mira
        // implementation defaults it (`retryable: bool = False` in Python,
        // `retryable?: boolean` in TypeScript, `#[serde(default)]` here), and
        // the published schema has only `message` required, so absence is
        // inside the protocol's own forward-compatibility contract.
        assert_eq!(
            serde_json::to_string(&RpcError::internal("boom")).unwrap(),
            r#"{"code":-32603,"message":"boom"}"#
        );
        assert_eq!(
            serde_json::to_string(&RpcError::internal("busy").retryable()).unwrap(),
            r#"{"code":-32603,"message":"busy","retryable":true}"#
        );

        // Everything else on the wire is byte-identical to before.
        let request = Request {
            id: 7,
            method: "run".into(),
            params: serde_json::json!({ "a": 1 }),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"id":7,"method":"run","params":{"a":1}}"#
        );
        assert_eq!(
            serde_json::to_string(&Response::ok(7, serde_json::json!({ "ok": true }))).unwrap(),
            r#"{"id":7,"result":{"ok":true}}"#
        );
        let notification = Notification {
            method: "event".into(),
            params: serde_json::json!({ "k": 1 }),
        };
        assert_eq!(
            serde_json::to_string(&notification).unwrap(),
            r#"{"method":"event","params":{"k":1}}"#
        );
    }

    #[test]
    fn a_study_that_omits_retryable_still_parses() {
        // The other half of the same contract: an older study that never sends
        // the field, and one that sends it explicitly false, both read as
        // not-retryable.
        for line in [
            r#"{"code":-32603,"message":"x"}"#,
            r#"{"code":-32603,"message":"x","retryable":false}"#,
        ] {
            let error: RpcError = serde_json::from_str(line).unwrap();
            assert!(!error.retryable, "{line}");
        }
    }

    #[test]
    fn rpc_error_backward_compatible_with_bare_message() {
        // A peer sends only `message`; the optional fields default.
        let back: RpcError = serde_json::from_str(r#"{"message":"no such eval"}"#).unwrap();
        assert_eq!(back.message, "no such eval");
        // A missing code now reads as INTERNAL_ERROR rather than 0. Nothing on
        // the wire changed: this is how an error that declines to classify
        // itself is interpreted, and 0 is not a JSON-RPC code, so it said
        // nothing. Retry behaviour keys on `retryable` and the message, not on
        // this value.
        assert_eq!(back.code, codes::INTERNAL_ERROR);
        assert!(!back.retryable);
        assert!(back.data.is_none());
    }

    #[test]
    fn run_result_key() {
        let r = RunResult {
            eval: "greet".into(),
            sample: "hi".into(),
            target: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            input: Vec::new(),
            expected: None,
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
        // A repeated case (trials > 1) carries a `#index` suffix in its key, but
        // all trials share one logical key so the host can group them.
        let r = RunResult {
            eval: "greet".into(),
            sample: "hi".into(),
            target: "sim".into(),
            params: Default::default(),
            trial: 2,
            trials: 5,
            seed: Some(42),
            input: Vec::new(),
            expected: None,
            passed: true,
            aggregate: 1.0,
            scores: vec![],
            transcript: TranscriptSummary::default(),
            skipped: false,
        };
        assert_eq!(r.key(), "greet/hi@sim#2");
        assert_eq!(r.logical_key(), "greet/hi@sim");

        // A single-trial case (trials <= 1) keeps the plain key — backward compat.
        let single = RunResult {
            trial: 0,
            trials: 1,
            ..r.clone()
        };
        assert_eq!(single.key(), "greet/hi@sim");
    }

    #[test]
    fn pre_trials_payloads_parse_as_single_trial() {
        // A study may omit trial/trials/seed entirely. The host must parse
        // such a RunResult and treat it as a single, unrepeated case (plain key).
        let line = r#"{"eval":"greet","sample":"hi","target":"sim","passed":true,
            "aggregate":1.0,"scores":[],
            "transcript":{"final_response":"hi","iterations":1,"tool_calls_count":0,
            "usage":{"input_tokens":1,"output_tokens":1,"cost_usd":0.0}}}"#;
        let r: RunResult = serde_json::from_str(line).unwrap();
        assert_eq!(r.trial, 0);
        assert_eq!(r.trials, 0);
        assert_eq!(r.seed, None);
        assert_eq!(r.key(), "greet/hi@sim"); // no `#trial` suffix
        assert_eq!(r.logical_key(), "greet/hi@sim");

        // Likewise an EvalInfo may omit trials/seed.
        let line = r#"{"name":"greet","samples":[{"id":"hi"}],"scorers":["s"],
            "targets":[{"label":"sim","available":true}]}"#;
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
            target: "m".into(),
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
