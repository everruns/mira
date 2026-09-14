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

use mira::protocol::{self, Notification, Request, Response};
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

    // Envelopes: every protocol line is exactly one of these three. They are
    // mira's own framing, not part of the method declaration, so they are
    // registered here.
    let request = generator.subschema_for::<Request>();
    let response = generator.subschema_for::<Response>();
    let notification = generator.subschema_for::<Notification>();
    let mut defs = generator.take_definitions(true);

    // Payloads: from the declaration, via the `schema_document()` the
    // `protocol!` macro generates. They ride inside the envelopes' free-form
    // `params`/`result` (typed as `serde_json::Value` on the wire), so their
    // shapes have to be published for study authors some other way.
    //
    // This used to be a hand-written list of `subschema_for::<T>()` calls, and
    // it had drifted: `InitializeParams` was missing, so `meta.json` named a
    // payload type that `schema.json` did not define. Registering them from the
    // declaration makes the set exhaustive by construction — a method cannot be
    // added to the protocol without its payloads appearing here.
    let declared = protocol::schema_document().schema();
    if let Some(declared_defs) = declared.get("$defs").and_then(|d| d.as_object()) {
        for (name, schema) in declared_defs {
            defs.entry(name.clone()).or_insert_with(|| schema.clone());
        }
    }

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
        // Per-method payload shapes, keyed by wire method name: what an SDK
        // generator reads to emit a typed method. `meta.json` names the types;
        // this is where those names resolve.
        "messages": declared.get("messages").cloned().unwrap_or(serde_json::json!({})),
        "$defs": defs,
    })
}

/// The version-and-methods sidecar (mirrors ACP's `meta.json`): a tiny, stable
/// index a host can read without parsing the full schema, and the artifact the
/// Python and TypeScript SDK generators walk.
///
/// The vocabulary comes from [`protocol::META`], the `lanok::protocol!`
/// declaration as data, rather than being listed again here. It used to be a
/// third copy of the method names and a second of the capability tokens, kept
/// in step by nothing: adding a method to the protocol and forgetting this list
/// published a `meta.json` that described the previous protocol, and every SDK
/// generated from it inherited the omission silently.
///
/// Declaring them once also makes the artifact say *more* than a name list. Each
/// method now carries its direction, whether it expects a response, the
/// capability it needs, and its documentation — enough for a generator to emit a
/// typed method, not just a string constant.
///
/// The two mira-specific keys stay: `schema` names the sibling artifact, and
/// `event_kinds` is the `event` notification's own token vocabulary, which is
/// payload content rather than protocol vocabulary and so has no place in the
/// declaration.
pub fn build_meta() -> serde_json::Value {
    let mut meta = serde_json::to_value(protocol::META).expect("protocol metadata serializes");
    let object = meta.as_object_mut().expect("metadata is an object");
    object.insert("schema".into(), serde_json::json!("schema.json"));
    object.insert(
        "event_kinds".into(),
        serde_json::json!(protocol::event::ALL),
    );
    meta
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
