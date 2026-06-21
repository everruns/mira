//! Builds the machine-readable JSON Schema for the Mira eval protocol from the
//! canonical Rust types in [`mira::protocol`]. The artifacts under
//! `schema/v<major>/` are *derived* here — never hand-edited — so the
//! language-neutral contract stays in lockstep with the wire format (the
//! approach the Agent Client Protocol uses for its `schema/` dir).
//!
//! The schema describes only the **stable** protocol: this crate depends on
//! `mira-eval` without its `protocol-unstable` feature, so fields still being
//! trialled behind that flag are excluded until they're promoted.

use std::path::PathBuf;

use mira::protocol::{
    self, ExecuteResult, InitializeResult, ListResult, Notification, Request, Response, RunParams,
    RunResult, ScoreParams,
};
use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;

/// Build the combined JSON Schema document: a root `anyOf` over the three wire
/// envelopes, with every payload type collected into `$defs`.
///
/// `anyOf` (not `oneOf`): the envelopes are open schemas (no
/// `additionalProperties: false`, per the protocol's ignore-unknown-fields
/// contract), so a `Request` structurally also satisfies the looser `Response`.
/// `oneOf` would reject that as ambiguous; classification is by presence of
/// `id`/`method`, which `anyOf` captures correctly.
pub fn build_schema() -> serde_json::Value {
    let mut generator: SchemaGenerator = SchemaSettings::draft2020_12().into_generator();

    // Envelopes: every protocol line is exactly one of these three.
    let request = generator.subschema_for::<Request>();
    let response = generator.subschema_for::<Response>();
    let notification = generator.subschema_for::<Notification>();

    // Method result/params payloads. They ride inside the envelopes' free-form
    // `params`/`result` (typed as `serde_json::Value` on the wire), so register
    // them explicitly to publish their shapes in `$defs` for study authors.
    generator.subschema_for::<InitializeResult>();
    generator.subschema_for::<ListResult>();
    generator.subschema_for::<RunParams>();
    generator.subschema_for::<RunResult>();
    generator.subschema_for::<ExecuteResult>();
    generator.subschema_for::<ScoreParams>();

    let defs = generator.take_definitions(true);

    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        // Keyed off the protocol major, like `schema_dir()`, so a future major
        // bump emits a matching canonical id (no stale `v1` claim under `v2/`).
        "$id": format!(
            "https://everruns.com/mira/schema/v{}/schema.json",
            protocol::version_major(protocol::PROTOCOL_VERSION),
        ),
        "title": "Mira Eval Protocol",
        "description": format!(
            "Wire schema for the Mira host<->study protocol (newline-delimited \
             JSON over stdio). Protocol version {}. Generated from mira::protocol \
             by `mira-schema-gen` (`just schema`); do not edit by hand.",
            protocol::PROTOCOL_VERSION,
        ),
        "anyOf": [request, response, notification],
        "$defs": defs,
    })
}

/// The version-and-methods sidecar (mirrors ACP's `meta.json`): a tiny, stable
/// index a host can read without parsing the full schema.
pub fn build_meta() -> serde_json::Value {
    serde_json::json!({
        "version": protocol::PROTOCOL_VERSION,
        "min_version": protocol::MIN_PROTOCOL_VERSION,
        "schema": "schema.json",
        "methods": ["initialize", "list", "run", "execute", "score"],
        "capabilities": [
            protocol::capabilities::AXES,
            protocol::capabilities::EVENTS,
            protocol::capabilities::USAGE,
            protocol::capabilities::EXECUTE,
            protocol::capabilities::SCORE,
            protocol::capabilities::TRIALS,
        ],
    })
}

/// `schema/v<major>` under the repo root, resolved from this crate's manifest
/// dir so it's invariant to the caller's CWD.
pub fn schema_dir() -> PathBuf {
    let major = protocol::version_major(protocol::PROTOCOL_VERSION);
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema")
        .join(format!("v{major}"))
}

/// Pretty JSON with a trailing newline, so files are diff- and editor-friendly.
pub fn render(value: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("schema serializes");
    s.push('\n');
    s
}

/// The artifact name → rendered-body pairs that make up the committed schema dir.
pub fn artifacts() -> Vec<(&'static str, String)> {
    vec![
        ("schema.json", render(&build_schema())),
        ("meta.json", render(&build_meta())),
    ]
}
