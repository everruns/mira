//! Server side of the eval protocol. Your eval program defines its evals in
//! Rust and hands them to [`serve`] (or registers them and calls
//! [`serve_registered`]); this runs the stdio loop that answers the host's
//! `initialize` / `list` / `run` requests.
//!
//! ```no_run
//! # async fn f(evals: Vec<mira::Eval>) -> std::io::Result<()> {
//! mira::serve(evals).await
//! # }
//! ```
//!
//! Keep stdout clean: only protocol JSON goes there. Logging belongs on stderr.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::eval::Eval;
use crate::protocol::{
    AxisInfo, EvalInfo, InitializeResult, ListResult, ModelInfo, Notification, PROTOCOL_VERSION,
    Request, Response, RunParams, RunResult, SampleInfo, TranscriptSummary, capabilities,
};
use crate::registry::registered_evals;
use crate::runner::run_cell;

/// Serve every [`register_eval!`](crate::register_eval)-registered eval.
pub async fn serve_registered() -> std::io::Result<()> {
    serve(registered_evals()).await
}

/// Serve `evals` over newline-delimited JSON on stdin/stdout until EOF.
pub async fn serve(evals: Vec<Eval>) -> std::io::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                // Can't correlate a malformed line to an id; report and move on.
                write_line(&mut stdout, &log_notification(format!("bad request: {e}"))).await?;
                continue;
            }
        };

        let response = dispatch(&evals, &request, &mut stdout).await;
        write_line(&mut stdout, &response).await?;
    }
    Ok(())
}

async fn dispatch(evals: &[Eval], request: &Request, stdout: &mut tokio::io::Stdout) -> Response {
    match request.method.as_str() {
        "initialize" => Response::ok(
            request.id,
            json(&InitializeResult {
                protocol_version: PROTOCOL_VERSION.into(),
                server: env!("CARGO_PKG_NAME").into(),
                evals: evals.len(),
                server_version: Some(env!("CARGO_PKG_VERSION").into()),
                capabilities: vec![
                    capabilities::AXES.into(),
                    capabilities::EVENTS.into(),
                    capabilities::USAGE.into(),
                ],
            }),
        ),
        "list" => Response::ok(request.id, json(&list(evals))),
        "run" => {
            let params: RunParams = match serde_json::from_value(request.params.clone()) {
                Ok(p) => p,
                Err(e) => return Response::err(request.id, format!("bad run params: {e}")),
            };
            // Progress so the host can render a live spinner / log.
            let _ = write_line(
                stdout,
                &event_notification(&params.eval, &params.sample, &params.model, "started"),
            )
            .await;
            match run(evals, &params).await {
                Ok(result) => Response::ok(request.id, json(&result)),
                Err(e) => Response::err(request.id, e),
            }
        }
        other => Response::err(request.id, format!("unknown method: {other}")),
    }
}

/// Build the `list` advertisement from the in-memory evals.
pub fn list(evals: &[Eval]) -> ListResult {
    let evals = evals
        .iter()
        .map(|eval| EvalInfo {
            name: eval.name.clone(),
            description: eval.description.clone(),
            samples: eval
                .dataset
                .samples
                .iter()
                .map(|s| SampleInfo {
                    id: s.id.clone(),
                    tags: s.tags.clone(),
                })
                .collect(),
            scorers: eval.scorers.iter().map(|s| s.name()).collect(),
            models: eval
                .models
                .iter()
                .map(|m| ModelInfo {
                    label: m.label.clone(),
                    available: m.available,
                })
                .collect(),
            axes: eval
                .axes
                .iter()
                .map(|a| AxisInfo {
                    name: a.name.clone(),
                    values: a.values.clone(),
                })
                .collect(),
            max_turns: eval.max_turns,
            metadata: eval.metadata.clone(),
        })
        .collect();
    ListResult { evals }
}

async fn run(evals: &[Eval], params: &RunParams) -> Result<RunResult, String> {
    let eval = evals
        .iter()
        .find(|e| e.name == params.eval)
        .ok_or_else(|| format!("no such eval: {}", params.eval))?;
    let sample = eval
        .dataset
        .samples
        .iter()
        .find(|s| s.id == params.sample)
        .ok_or_else(|| format!("no such sample: {}/{}", params.eval, params.sample))?;
    let model = eval
        .models
        .iter()
        .find(|m| m.label == params.model)
        .ok_or_else(|| format!("no such model: {}@{}", params.eval, params.model))?;

    // Don't burn time on an unrunnable cell; report it as skipped.
    if !model.available {
        return Ok(RunResult {
            eval: params.eval.clone(),
            sample: params.sample.clone(),
            model: params.model.clone(),
            params: params.params.clone(),
            passed: false,
            aggregate: 0.0,
            scores: Vec::new(),
            transcript: TranscriptSummary::default(),
            skipped: true,
        });
    }

    let outcome = run_cell(eval, sample, model, &params.params).await;
    Ok(RunResult {
        eval: outcome.eval,
        sample: outcome.sample_id,
        model: outcome.model,
        params: outcome.params,
        passed: outcome.passed,
        aggregate: outcome.aggregate,
        scores: outcome.scores,
        transcript: TranscriptSummary {
            final_response: outcome.transcript.final_response,
            iterations: outcome.transcript.iterations,
            tool_calls_count: outcome.transcript.tool_calls_count,
            tool_calls: outcome.transcript.tool_calls,
            usage: outcome.transcript.usage,
            timing: outcome.transcript.timing,
            metadata: outcome.transcript.metadata,
            error: outcome.transcript.error,
        },
        skipped: false,
    })
}

fn json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

fn log_notification(message: String) -> Notification {
    Notification {
        method: "log".into(),
        params: serde_json::json!({ "message": message }),
    }
}

fn event_notification(eval: &str, sample: &str, model: &str, kind: &str) -> Notification {
    Notification {
        method: "event".into(),
        params: serde_json::json!({
            "eval": eval, "sample": sample, "model": model, "kind": kind,
        }),
    }
}

async fn write_line<T: serde::Serialize>(
    stdout: &mut tokio::io::Stdout,
    value: &T,
) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(value).unwrap_or_default();
    buf.push(b'\n');
    stdout.write_all(&buf).await?;
    stdout.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::contains;
    use crate::subject::subject_fn;
    use crate::{Eval, Sample, Transcript};

    fn evals() -> Vec<Eval> {
        vec![
            Eval::new("greet")
                .describe("greeting eval")
                .meta("suite", "smoke")
                .sample(Sample::new("hi", "say hi").tag("smoke"))
                .subject(subject_fn(|_, _| async {
                    Transcript::response("hi there")
                }))
                .scorer(contains("hi"))
                .build(),
        ]
    }

    #[test]
    fn list_advertises_everything() {
        let listing = list(&evals());
        assert_eq!(listing.evals.len(), 1);
        let e = &listing.evals[0];
        assert_eq!(e.description, "greeting eval");
        assert_eq!(e.metadata.get("suite").unwrap(), "smoke");
        assert_eq!(e.samples[0].tags, vec!["smoke"]);
        assert_eq!(e.models[0].label, "sim");
        assert!(e.models[0].available);
    }

    #[tokio::test]
    async fn run_scores_a_cell() {
        let params = RunParams {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
        };
        let result = run(&evals(), &params).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.transcript.final_response, "hi there");
    }

    #[tokio::test]
    async fn run_rejects_unknown_eval() {
        let params = RunParams {
            eval: "nope".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
        };
        assert!(run(&evals(), &params).await.is_err());
    }
}
