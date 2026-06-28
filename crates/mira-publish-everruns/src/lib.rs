//! Publish a saved Mira run to an everruns instance.
//!
//! everruns hosts and visualizes eval results it did **not** execute (the
//! `POST /v1/evals/import` endpoint). This crate maps a finished Mira run —
//! [`RunMeta`] plus its per-case [`RunResult`]s — onto that import payload and
//! POSTs it, so a study run with any provider/CLI subject can be published for
//! hosted comparison without onboarding into everruns' session system.
//!
//! Design notes:
//! - **Integration crate, kept out of the core.** Like `mira-everruns`, this
//!   lives beside `mira-eval` so the provider-agnostic core never grows an HTTP
//!   client or everruns coupling.
//! - **CLI auth pass-through.** Credentials resolve the same way the everruns
//!   CLI resolves them — explicit override, then `EVERRUNS_API_KEY` /
//!   `EVERRUNS_API_URL`, then the everruns credentials file
//!   (`~/.config/everruns/credentials.json`). Run `everruns login` once and
//!   `mira publish` just works.
//! - **everruns trusts the verdict.** We send Mira's pass/fail and scores as
//!   data; everruns stores and displays them, it does not re-grade.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use mira::Score;
use mira::protocol::RunResult;
use mira::run::RunMeta;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Default everruns API base when none is configured.
pub const DEFAULT_API_URL: &str = "https://app.everruns.com/api";

/// Resolved everruns connection: where to POST and how to authenticate.
#[derive(Clone, Debug)]
pub struct EverrunsTarget {
    /// API base, e.g. `https://app.everruns.com/api` (no trailing `/v1`).
    pub base_url: String,
    /// Personal access token (`evr_pat_…`), sent as `Authorization: Bearer`.
    pub api_key: String,
    /// Public org id (`org_…`) sent as `X-Org-Id`. Optional: a single-org user
    /// is resolved server-side without it.
    pub org_id: Option<String>,
}

/// Overrides for credential resolution (from CLI flags). Any `None` falls back
/// to env vars, then the everruns credentials file.
#[derive(Clone, Debug, Default)]
pub struct PublishOptions {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub org_id: Option<String>,
    /// everruns credentials profile to read (defaults to its `current_profile`).
    pub profile: Option<String>,
}

/// What `GET /v1/evals/import/preflight` reports.
#[derive(Clone, Debug, Deserialize)]
pub struct Preflight {
    pub evals_enabled: bool,
    pub can_import: bool,
}

/// Result of a successful publish.
#[derive(Clone, Debug)]
pub struct PublishOutcome {
    /// everruns EvalRun public ids created (one per Mira eval).
    pub run_ids: Vec<String>,
    /// Number of evals published.
    pub evals: usize,
    /// Number of case-results published.
    pub cases: usize,
}

// ============================================================================
// Credential resolution (mirrors the everruns CLI order)
// ============================================================================

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(default)]
    current_profile: Option<String>,
    #[serde(default)]
    profiles: BTreeMap<String, CredentialProfile>,
}

#[derive(Deserialize, Default)]
struct CredentialProfile {
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    org_id: Option<String>,
}

/// Resolve the everruns connection: explicit override → env
/// (`EVERRUNS_API_KEY`/`EVERRUNS_API_URL`/`EVERRUNS_ORG_ID`) → credentials file.
pub fn resolve_target(opts: &PublishOptions) -> Result<EverrunsTarget> {
    let profile = load_credentials_profile(opts.profile.as_deref());

    let api_key = opts
        .api_key
        .clone()
        .or_else(|| non_empty_env("EVERRUNS_API_KEY"))
        .or_else(|| profile.as_ref().and_then(|p| p.api_key.clone()))
        .context(
            "no everruns API key found — set EVERRUNS_API_KEY, run `everruns login`, or pass --api-key",
        )?;

    let base_url = opts
        .base_url
        .clone()
        .or_else(|| non_empty_env("EVERRUNS_API_URL"))
        .or_else(|| profile.as_ref().and_then(|p| p.api_url.clone()))
        .unwrap_or_else(|| DEFAULT_API_URL.to_string());

    let org_id = opts
        .org_id
        .clone()
        .or_else(|| non_empty_env("EVERRUNS_ORG_ID"))
        .or_else(|| profile.as_ref().and_then(|p| p.org_id.clone()));

    Ok(EverrunsTarget {
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key,
        org_id,
    })
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn load_credentials_profile(profile: Option<&str>) -> Option<CredentialProfile> {
    let path = everruns_credentials_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let file: CredentialsFile = serde_json::from_str(&text).ok()?;
    let name = profile
        .map(str::to_string)
        .or_else(|| non_empty_env("EVERRUNS_PROFILE"))
        .or(file.current_profile)
        .unwrap_or_else(|| "default".to_string());
    file.profiles
        .into_iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// Location of the everruns CLI credentials file, matching its own resolution
/// (`dirs::config_dir()/everruns/credentials.json`).
fn everruns_credentials_path() -> Option<PathBuf> {
    let base = if let Some(xdg) = non_empty_env("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = non_empty_env("HOME")?;
        #[cfg(target_os = "macos")]
        {
            PathBuf::from(home).join("Library/Application Support")
        }
        #[cfg(not(target_os = "macos"))]
        {
            PathBuf::from(home).join(".config")
        }
    };
    Some(base.join("everruns").join("credentials.json"))
}

// ============================================================================
// Import payload (mirrors everruns' POST /v1/evals/import request shape)
// ============================================================================

#[derive(Serialize)]
struct ImportRequest {
    source: ImportSource,
    evals: Vec<ImportGroup>,
}

#[derive(Serialize)]
struct ImportSource {
    system: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

#[derive(Serialize)]
struct ImportGroup {
    name: String,
    cases: Vec<ImportCase>,
}

#[derive(Serialize)]
struct ImportCase {
    name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    input: Vec<String>,
    target: ImportTarget,
    status: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scores: Vec<ImportScore>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
}

#[derive(Serialize)]
struct ImportTarget {
    provider: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Serialize)]
struct ImportScore {
    scorer: String,
    value: f64,
    pass: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    reason: String,
    #[serde(skip_serializing_if = "is_false")]
    na: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Build the everruns import payload from a finished run. Pure (no IO), so the
/// mapping is unit-testable. Groups results by Mira `eval` (one everruns EvalRun
/// per eval, all sharing `source.run_id`).
fn build_payload(meta: &RunMeta, results: &[RunResult]) -> ImportRequest {
    let mut by_eval: BTreeMap<String, Vec<&RunResult>> = BTreeMap::new();
    for r in results {
        by_eval.entry(r.eval.clone()).or_default().push(r);
    }

    let evals = by_eval
        .into_iter()
        .map(|(name, rs)| ImportGroup {
            name,
            cases: rs.into_iter().map(import_case).collect(),
        })
        .collect();

    let version = meta
        .environment
        .as_ref()
        .and_then(|e| e.mira_version.clone());
    let metadata = json!({
        "study": meta.study,
        "study_version": meta.study_version,
        "environment": meta.environment,
    });

    ImportRequest {
        source: ImportSource {
            system: "mira".to_string(),
            version,
            run_id: meta.run_id.clone(),
            metadata: Some(metadata),
        },
        evals,
    }
}

fn import_case(r: &RunResult) -> ImportCase {
    let (provider, model) = split_target(&r.target);

    // Preserve the raw target label and any axis params so nothing is lost when
    // the label isn't a clean `provider/model`.
    let mut params = Map::new();
    params.insert("label".to_string(), json!(r.target));
    if let Ok(Value::Object(axis)) = serde_json::to_value(&r.params) {
        for (k, v) in axis {
            params.insert(k, v);
        }
    }

    let t = &r.transcript;
    let mut transcript = Map::new();
    if !r.input.is_empty() {
        transcript.insert("input".to_string(), json!(r.input));
    }
    if !t.final_response.is_empty() {
        transcript.insert("final_response".to_string(), json!(t.final_response));
    }
    if !t.tool_calls.is_empty() {
        transcript.insert("tool_calls".to_string(), json!(t.tool_calls));
    }
    if !t.output.is_empty() {
        transcript.insert("output".to_string(), json!(t.output));
    }
    transcript.insert("iterations".to_string(), json!(t.iterations));

    // Open-vocab metrics bag: study-supplied metrics plus the usage/timing
    // breakdowns that don't warrant top-level fields.
    let mut metrics: Map<String, Value> = t
        .metrics
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    if t.usage.cost_usd != 0.0 {
        metrics.insert("cost_usd".to_string(), json!(t.usage.cost_usd));
    }
    if t.usage.cache_read_tokens != 0 {
        metrics.insert(
            "cache_read_tokens".to_string(),
            json!(t.usage.cache_read_tokens),
        );
    }
    if t.usage.reasoning_tokens != 0 {
        metrics.insert(
            "reasoning_tokens".to_string(),
            json!(t.usage.reasoning_tokens),
        );
    }
    if let Some(ttft) = t.timing.time_to_first_token_ms {
        metrics.insert("time_to_first_token_ms".to_string(), json!(ttft));
    }

    ImportCase {
        name: r.sample.clone(),
        input: r.input.clone(),
        target: ImportTarget {
            provider,
            model,
            params: Some(Value::Object(params)),
        },
        status: case_status(r),
        scores: r.scores.iter().map(import_score).collect(),
        transcript: Some(Value::Object(transcript)),
        metrics: (!metrics.is_empty()).then_some(Value::Object(metrics)),
        turns: u32::try_from(t.iterations).ok(),
        latency_ms: (t.timing.duration_ms != 0).then_some(t.timing.duration_ms),
        input_tokens: (t.usage.input_tokens != 0).then_some(t.usage.input_tokens),
        output_tokens: (t.usage.output_tokens != 0).then_some(t.usage.output_tokens),
        error_message: t.error.clone(),
    }
}

fn import_score(s: &Score) -> ImportScore {
    // everruns requires a finite value in [0,1]; Mira values are already in
    // that range, but guard against a non-finite leaking through.
    let value = if s.value.is_finite() {
        s.value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    ImportScore {
        scorer: s.scorer.clone(),
        value,
        pass: s.pass,
        reason: s.reason.clone(),
        na: s.na,
    }
}

/// Map a Mira result onto an everruns case status. Mira's "ran but no verdict"
/// (all-N/A, no error) maps to `skipped` — excluded from pass/fail tallies, like
/// a skip — while an infra/subject error maps to `errored`.
fn case_status(r: &RunResult) -> &'static str {
    if r.skipped {
        return "skipped";
    }
    if r.transcript.error.is_some() {
        return "errored";
    }
    if r.scores.is_empty() || r.scores.iter().all(|s| s.na) {
        return "skipped";
    }
    if r.passed { "passed" } else { "failed" }
}

/// Split a Mira target label into `(provider, model)`. `anthropic/opus` →
/// `("anthropic", "opus")`; a bare label (e.g. `sim`, `yolop`) keeps the whole
/// thing as the provider with an empty model. The raw label is preserved in the
/// target params regardless.
fn split_target(label: &str) -> (String, String) {
    match label.split_once('/') {
        Some((p, m)) => (p.to_string(), m.to_string()),
        None => (label.to_string(), String::new()),
    }
}

// ============================================================================
// HTTP
// ============================================================================

/// Probe whether the target org can accept an import (feature enabled + the
/// caller holds the import permission). Lets the caller fail clearly before
/// sending a payload — evals are an optional everruns feature.
pub async fn preflight(target: &EverrunsTarget) -> Result<Preflight> {
    let url = format!("{}/v1/evals/import/preflight", target.base_url);
    let resp = authed(reqwest::Client::new().get(&url), target)
        .send()
        .await
        .context("preflight request to everruns failed")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("everruns preflight returned {status}: {body}");
    }
    serde_json::from_str(&body).context("could not parse preflight response")
}

/// Publish a finished run. Resolves credentials, maps the run, and POSTs it to
/// `/v1/evals/import`. Idempotent on the run id: re-publishing replaces the
/// prior run for each eval.
pub async fn publish(
    meta: &RunMeta,
    results: &[RunResult],
    opts: &PublishOptions,
) -> Result<PublishOutcome> {
    let target = resolve_target(opts)?;
    publish_to(&target, meta, results).await
}

/// Publish to an already-resolved target. Separated so callers that already have
/// credentials (or want to preflight first) can reuse the connection.
pub async fn publish_to(
    target: &EverrunsTarget,
    meta: &RunMeta,
    results: &[RunResult],
) -> Result<PublishOutcome> {
    let payload = build_payload(meta, results);
    let evals = payload.evals.len();
    let cases: usize = payload.evals.iter().map(|e| e.cases.len()).sum();
    if evals == 0 {
        bail!("nothing to publish: the run has no case results");
    }

    let url = format!("{}/v1/evals/import", target.base_url);
    let resp = authed(reqwest::Client::new().post(&url), target)
        .json(&payload)
        .send()
        .await
        .context("import request to everruns failed")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(import_error_hint(status, &body));
    }

    let run_ids = parse_run_ids(&body);
    Ok(PublishOutcome {
        run_ids,
        evals,
        cases,
    })
}

fn authed(req: reqwest::RequestBuilder, target: &EverrunsTarget) -> reqwest::RequestBuilder {
    let req = req.bearer_auth(&target.api_key);
    match &target.org_id {
        Some(org) => req.header("X-Org-Id", org),
        None => req,
    }
}

/// Turn a non-2xx import response into an actionable message.
fn import_error_hint(status: reqwest::StatusCode, body: &str) -> String {
    let extra = match status.as_u16() {
        401 => " — check your EVERRUNS_API_KEY / `everruns login` session",
        403 => " — your account lacks eval-import permission (run preflight)",
        404 => " — the evals feature may be disabled on this everruns instance",
        _ => "",
    };
    format!("everruns import returned {status}{extra}: {body}")
}

/// Extract created EvalRun ids from a `{ "data": [ { "id": … } ] }` response.
/// Best-effort: a parse miss yields an empty list, not an error, since the
/// import already succeeded.
fn parse_run_ids(body: &str) -> Vec<String> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("data").cloned())
        .and_then(|d| d.as_array().cloned())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mira::protocol::TranscriptSummary;

    fn result(eval: &str, sample: &str, target: &str, passed: bool) -> RunResult {
        RunResult {
            eval: eval.to_string(),
            sample: sample.to_string(),
            target: target.to_string(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            input: vec!["solve it".to_string()],
            expected: None,
            passed,
            aggregate: if passed { 1.0 } else { 0.0 },
            scores: vec![Score::graded(
                "contains",
                if passed { 1.0 } else { 0.0 },
                0.5,
                "r",
            )],
            transcript: TranscriptSummary {
                final_response: "done".to_string(),
                iterations: 2,
                tool_calls_count: 1,
                tool_calls: vec!["read".to_string()],
                ..Default::default()
            },
            skipped: false,
        }
    }

    fn meta() -> RunMeta {
        RunMeta {
            format: 1,
            run_id: "20260101T000000Z-abcd".to_string(),
            study: "coding".to_string(),
            study_version: Some("1.0".to_string()),
            started_unix: 0,
            finished_unix: 0,
            environment: None,
            summary: Default::default(),
        }
    }

    #[test]
    fn groups_by_eval_and_maps_targets() {
        let results = vec![
            result("arith", "add", "anthropic/opus", true),
            result("arith", "add", "openai/gpt", false),
            result("geo", "france", "sim", true),
        ];
        let payload = build_payload(&meta(), &results);
        assert_eq!(payload.source.system, "mira");
        assert_eq!(payload.source.run_id, "20260101T000000Z-abcd");
        // Two evals → two groups.
        assert_eq!(payload.evals.len(), 2);
        let arith = payload.evals.iter().find(|e| e.name == "arith").unwrap();
        // Same sample, two targets → two case-results (the matrix).
        assert_eq!(arith.cases.len(), 2);
        let opus = arith
            .cases
            .iter()
            .find(|c| c.target.model == "opus")
            .unwrap();
        assert_eq!(opus.target.provider, "anthropic");
        assert_eq!(opus.status, "passed");
        assert_eq!(opus.scores[0].scorer, "contains");
        // Bare label keeps whole thing as provider, empty model.
        let geo = payload.evals.iter().find(|e| e.name == "geo").unwrap();
        assert_eq!(geo.cases[0].target.provider, "sim");
        assert_eq!(geo.cases[0].target.model, "");
    }

    #[test]
    fn status_maps_skipped_errored_and_na() {
        let mut skipped = result("e", "s", "sim", false);
        skipped.skipped = true;
        assert_eq!(case_status(&skipped), "skipped");

        let mut errored = result("e", "s", "sim", false);
        errored.transcript.error = Some("boom".to_string());
        assert_eq!(case_status(&errored), "errored");

        let mut na = result("e", "s", "sim", false);
        na.scores = vec![Score::na("judge", "unreachable")];
        assert_eq!(case_status(&na), "skipped");

        assert_eq!(case_status(&result("e", "s", "sim", true)), "passed");
        assert_eq!(case_status(&result("e", "s", "sim", false)), "failed");
    }

    #[test]
    fn parses_run_ids_from_data_envelope() {
        let body = r#"{"data":[{"id":"evalrun_01","status":"completed"},{"id":"evalrun_02"}]}"#;
        assert_eq!(parse_run_ids(body), vec!["evalrun_01", "evalrun_02"]);
        assert!(parse_run_ids("not json").is_empty());
    }

    // End-to-end over real HTTP against a one-shot mock server: verifies the
    // actual request line, auth/org headers, and JSON body that everruns will
    // receive, plus response parsing — the wiring the pure mapping tests can't.
    #[tokio::test]
    async fn publish_to_sends_well_formed_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Read until headers + the Content-Length body are in hand.
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            loop {
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(idx) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..idx]).to_ascii_lowercase();
                    let clen = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= idx + 4 + clen {
                        break;
                    }
                }
            }
            // Body delimited by connection close (no Content-Length needed).
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"evalrun_smoke\"}]}";
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.shutdown().await.unwrap();
            String::from_utf8_lossy(&buf).to_string()
        });

        let target = EverrunsTarget {
            base_url: format!("http://{addr}"),
            api_key: "evr_pat_secret".to_string(),
            org_id: Some("org_42".to_string()),
        };
        let results = vec![result("arith", "add", "anthropic/opus", true)];
        let outcome = publish_to(&target, &meta(), &results).await.unwrap();
        assert_eq!(outcome.run_ids, vec!["evalrun_smoke"]);
        assert_eq!(outcome.evals, 1);
        assert_eq!(outcome.cases, 1);

        let req = server.await.unwrap();
        let lower = req.to_ascii_lowercase();
        assert!(
            req.starts_with("POST /v1/evals/import "),
            "request line: {req}"
        );
        assert!(lower.contains("authorization: bearer evr_pat_secret"));
        assert!(lower.contains("x-org-id: org_42"));
        assert!(req.contains("\"system\":\"mira\""));
        assert!(req.contains("\"name\":\"arith\""));
    }
}
