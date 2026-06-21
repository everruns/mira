//! Validates real serialized protocol messages against the committed schema,
//! and asserts the unstable staging convention keeps experimental fields out of
//! the stable artifact. This is the runtime counterpart to the `--check` drift
//! guard: `--check` proves the file matches the types; this proves the file
//! actually describes the messages studies emit.

use mira::protocol::{
    EvalInfo, ExecuteResult, InitializeResult, ListResult, ModelInfo, Notification, Request,
    Response, RunParams, RunResult, SampleInfo, ScoreParams, TranscriptSummary,
};
use mira::{Score, Transcript, Usage};
use mira_schema_gen::schema_dir;
use serde_json::{Value, json};

/// The committed root schema (the artifact downstream consumers validate against).
fn committed_schema() -> Value {
    let path = schema_dir().join("schema.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("committed schema is valid JSON")
}

/// Assert `instance` is valid against the committed root schema.
fn assert_valid_at_root(instance: &Value) {
    let schema = committed_schema();
    let validator = jsonschema::validator_for(&schema).expect("compile root schema");
    if !validator.is_valid(instance) {
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| e.to_string())
            .collect();
        panic!(
            "instance not valid at root:\n{}\n{instance:#}",
            errors.join("\n")
        );
    }
}

/// Assert `instance` is valid against `#/$defs/<name>` of the committed schema
/// (carrying the schema's own `$defs` along so internal `$ref`s resolve).
fn assert_valid_against_def(name: &str, instance: &Value) {
    let committed = committed_schema();
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$ref": format!("#/$defs/{name}"),
        "$defs": committed["$defs"].clone(),
    });
    let validator = jsonschema::validator_for(&schema).expect("compile subschema");
    if !validator.is_valid(instance) {
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| e.to_string())
            .collect();
        panic!(
            "instance not valid against {name}:\n{}\n{instance:#}",
            errors.join("\n")
        );
    }
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serialize")
}

#[test]
fn envelopes_validate_at_root() {
    let request = Request {
        id: 1,
        method: "run".into(),
        params: json!({ "eval": "greet", "sample": "hi", "model": "sim" }),
    };
    let response = Response::ok(1, json!({ "passed": true }));
    let error = Response::err(2, "no such eval: greet");
    let notification = Notification {
        method: "event".into(),
        params: json!({ "kind": "started" }),
    };

    assert_valid_at_root(&to_value(&request));
    assert_valid_at_root(&to_value(&response));
    assert_valid_at_root(&to_value(&error));
    assert_valid_at_root(&to_value(&notification));
}

#[test]
fn payloads_validate_against_their_defs() {
    let init = InitializeResult {
        protocol_version: "1.2".into(),
        study: "demo".into(),
        evals: 1,
        study_version: Some("0.1.0".into()),
        capabilities: vec!["axes".into(), "execute".into(), "score".into()],
    };
    assert_valid_against_def("InitializeResult", &to_value(&init));

    let list = ListResult {
        evals: vec![EvalInfo {
            name: "greet".into(),
            description: "Greets the user".into(),
            samples: vec![SampleInfo {
                id: "hi".into(),
                tags: vec!["smoke".into()],
                metadata: Default::default(),
            }],
            scorers: vec!["succeeded".into()],
            models: vec![ModelInfo {
                label: "sim".into(),
                provider: "sim".into(),
                available: true,
                metadata: Default::default(),
            }],
            axes: vec![],
            max_turns: 12,
            trials: 3,
            seed: Some(7),
            metadata: Default::default(),
        }],
    };
    assert_valid_against_def("ListResult", &to_value(&list));

    let params = RunParams {
        eval: "greet".into(),
        sample: "hi".into(),
        model: "sim".into(),
        params: Default::default(),
        trial: 1,
        trials: 3,
        seed: Some(8),
    };
    assert_valid_against_def("RunParams", &to_value(&params));

    // A transcript exercising the open metrics map and typed usage.
    let transcript = Transcript::response("Hi!")
        .with_metric("recall@5", 0.8)
        .with_duration_ms(420);

    let result = RunResult {
        eval: "greet".into(),
        sample: "hi".into(),
        model: "sim".into(),
        params: Default::default(),
        trial: 1,
        trials: 3,
        seed: Some(8),
        passed: true,
        aggregate: 1.0,
        scores: vec![Score::pass("succeeded", "ok")],
        transcript: TranscriptSummary {
            usage: Usage {
                input_tokens: 12,
                output_tokens: 8,
                cost_usd: 0.0001,
                ..Default::default()
            },
            ..TranscriptSummary::of(&transcript)
        },
        skipped: false,
    };
    assert_valid_against_def("RunResult", &to_value(&result));

    // `execute` / `score` payloads carry the *full* transcript.
    let execute = ExecuteResult {
        eval: "greet".into(),
        sample: "hi".into(),
        model: "sim".into(),
        params: Default::default(),
        trial: 1,
        trials: 3,
        seed: Some(8),
        transcript: transcript.clone(),
        skipped: false,
    };
    assert_valid_against_def("ExecuteResult", &to_value(&execute));

    let score = ScoreParams {
        eval: "greet".into(),
        sample: "hi".into(),
        model: "sim".into(),
        params: Default::default(),
        trial: 1,
        trials: 3,
        seed: Some(8),
        transcript,
    };
    assert_valid_against_def("ScoreParams", &to_value(&score));
}

/// The `protocol-unstable` staging convention's guarantee: fields gated behind
/// the feature (here `TranscriptSummary.experimental`) must NOT appear in the
/// committed, stable schema — the generator builds without that feature.
#[test]
fn unstable_field_absent_from_stable_schema() {
    let schema = committed_schema();
    let props = &schema["$defs"]["TranscriptSummary"]["properties"];
    assert!(
        props.get("final_response").is_some(),
        "sanity: stable transcript fields are present",
    );
    assert!(
        props.get("experimental").is_none(),
        "experimental field leaked into the stable schema; the generator must \
         build mira-eval without `protocol-unstable`",
    );
}
