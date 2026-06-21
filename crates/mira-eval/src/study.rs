//! The **study** side of the eval protocol. A [`Study`] is your eval program:
//! it bundles the evals you're investigating and, when you call
//! [`serve`](Study::serve), runs the stdio loop that answers the host's
//! `initialize` / `list` / `run` requests.
//!
//! ```no_run
//! # async fn f() -> std::io::Result<()> {
//! // Every `#[eval]`-registered eval in the binary:
//! mira::Study::registered().serve().await
//! # }
//! ```
//!
//! ```no_run
//! # fn greet() -> mira::Eval { unimplemented!() }
//! # fn coding() -> mira::Eval { unimplemented!() }
//! # async fn f() -> std::io::Result<()> {
//! // …or an explicit set:
//! mira::Study::new().eval(greet()).eval(coding()).serve().await
//! # }
//! ```
//!
//! Keep stdout clean: only protocol JSON goes there. Logging belongs on stderr.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::eval::Eval;
use crate::protocol::{
    AxisInfo, EvalInfo, ExecuteResult, InitializeResult, ListResult, ModelInfo, Notification,
    PROTOCOL_VERSION, Request, Response, RpcError, RunParams, RunResult, SampleInfo, ScoreParams,
    TranscriptSummary, capabilities, codes,
};
use crate::registry::registered_evals;
use crate::runner::{aggregate_value, execute_cell, run_cell, score_transcript, verdict};

/// Your eval program: a named bundle of [`Eval`]s exposed to the host over the
/// protocol. Build one, then [`serve`](Study::serve) it.
pub struct Study {
    /// Name advertised to the host in `initialize` (defaults to the crate name).
    name: String,
    evals: Vec<Eval>,
}

impl Default for Study {
    fn default() -> Self {
        Self::new()
    }
}

impl Study {
    /// An empty study. Add evals with [`eval`](Study::eval) /
    /// [`evals`](Study::evals).
    pub fn new() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").into(),
            evals: Vec::new(),
        }
    }

    /// A study of every [`register_eval!`](crate::register_eval)-registered eval
    /// in the binary (the `#[eval]` / `cargo test`-style discovery path).
    pub fn registered() -> Self {
        Self::new().evals(registered_evals())
    }

    /// Add one eval (builder style).
    pub fn eval(mut self, eval: Eval) -> Self {
        self.evals.push(eval);
        self
    }

    /// Add many evals.
    pub fn evals(mut self, evals: impl IntoIterator<Item = Eval>) -> Self {
        self.evals.extend(evals);
        self
    }

    /// Override the name advertised to the host (defaults to the crate name).
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Serve this study over newline-delimited JSON on stdin/stdout until EOF.
    /// The host drives the loop; this returns when stdin closes.
    ///
    /// Requests are dispatched **concurrently**: each `run` is handled on its own
    /// task so a host can keep many cells in flight at once. Writes are serialized
    /// through a shared stdout mutex (one whole line per lock), so responses and
    /// `event`/`log` notifications never interleave mid-line. The host bounds how
    /// many runs are in flight (see [`crate::exec`]).
    pub async fn serve(self) -> std::io::Result<()> {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
        let me = Arc::new(self);
        let mut tasks: JoinSet<()> = JoinSet::new();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let request: Request = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    // Can't correlate a malformed line to an id; report and move on.
                    write_line(&stdout, &log_notification(format!("bad request: {e}"))).await?;
                    continue;
                }
            };

            let me = me.clone();
            let stdout = stdout.clone();
            tasks.spawn(async move {
                let response = me.dispatch(&request, &stdout).await;
                let _ = write_line(&stdout, &response).await;
            });
        }

        // Drain in-flight runs before returning so no response is lost on EOF.
        while tasks.join_next().await.is_some() {}
        Ok(())
    }

    async fn dispatch(&self, request: &Request, stdout: &Arc<Mutex<Stdout>>) -> Response {
        match request.method.as_str() {
            "initialize" => Response::ok(
                request.id,
                json(&InitializeResult {
                    protocol_version: PROTOCOL_VERSION.into(),
                    study: self.name.clone(),
                    evals: self.evals.len(),
                    study_version: Some(env!("CARGO_PKG_VERSION").into()),
                    capabilities: vec![
                        capabilities::AXES.into(),
                        capabilities::EVENTS.into(),
                        capabilities::USAGE.into(),
                        capabilities::EXECUTE.into(),
                        capabilities::SCORE.into(),
                        capabilities::TRIALS.into(),
                    ],
                }),
            ),
            "list" => Response::ok(request.id, json(&self.list())),
            "run" => {
                let params: RunParams = match serde_json::from_value(request.params.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Response::err_with(
                            request.id,
                            RpcError::new(format!("bad run params: {e}"))
                                .with_code(codes::INVALID_PARAMS),
                        );
                    }
                };
                // Progress so the host can render a live spinner / log.
                let _ = write_line(
                    stdout,
                    &event_notification(&params.eval, &params.sample, &params.model, "started"),
                )
                .await;
                match self.run(&params).await {
                    Ok(result) => Response::ok(request.id, json(&result)),
                    Err(e) => Response::err(request.id, e),
                }
            }
            "execute" => {
                let params: RunParams = match serde_json::from_value(request.params.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Response::err_with(
                            request.id,
                            RpcError::new(format!("bad execute params: {e}"))
                                .with_code(codes::INVALID_PARAMS),
                        );
                    }
                };
                let _ = write_line(
                    stdout,
                    &event_notification(&params.eval, &params.sample, &params.model, "started"),
                )
                .await;
                match self.execute(&params).await {
                    Ok(result) => Response::ok(request.id, json(&result)),
                    Err(e) => Response::err(request.id, e),
                }
            }
            "score" => {
                let params: ScoreParams = match serde_json::from_value(request.params.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return Response::err_with(
                            request.id,
                            RpcError::new(format!("bad score params: {e}"))
                                .with_code(codes::INVALID_PARAMS),
                        );
                    }
                };
                match self.score(&params).await {
                    Ok(result) => Response::ok(request.id, json(&result)),
                    Err(e) => Response::err(request.id, e),
                }
            }
            other => Response::err_with(
                request.id,
                RpcError::new(format!("unknown method: {other}"))
                    .with_code(codes::METHOD_NOT_FOUND),
            ),
        }
    }

    /// Build the `list` advertisement from this study's evals.
    pub fn list(&self) -> ListResult {
        let evals = self
            .evals
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
                        provider: m.provider.clone(),
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
                trials: eval.trials,
                seed: eval.seed,
                metadata: eval.metadata.clone(),
            })
            .collect();
        ListResult { evals }
    }

    async fn run(&self, params: &RunParams) -> Result<RunResult, String> {
        let (eval, sample, model) = self.locate(&params.eval, &params.sample, &params.model)?;

        // Don't burn time on an unrunnable cell; report it as skipped.
        if !model.available {
            return Ok(skipped_result(params));
        }

        let outcome = run_cell(eval, sample, model, &params.params, params.trial()).await;
        Ok(RunResult {
            eval: outcome.eval,
            sample: outcome.sample_id,
            model: outcome.model,
            params: outcome.params,
            trial: params.trial,
            trials: params.trials,
            seed: params.seed,
            passed: outcome.passed,
            aggregate: outcome.aggregate,
            scores: outcome.scores,
            transcript: TranscriptSummary::of(&outcome.transcript),
            skipped: false,
        })
    }

    /// Execute a cell's subject only, returning the **full** transcript with no
    /// scoring (the run-now-score-later half of `run`).
    async fn execute(&self, params: &RunParams) -> Result<ExecuteResult, String> {
        let (eval, sample, model) = self.locate(&params.eval, &params.sample, &params.model)?;
        if !model.available {
            return Ok(ExecuteResult {
                eval: params.eval.clone(),
                sample: params.sample.clone(),
                model: params.model.clone(),
                params: params.params.clone(),
                trial: params.trial,
                trials: params.trials,
                seed: params.seed,
                transcript: Default::default(),
                skipped: true,
            });
        }
        let transcript = execute_cell(eval, sample, model, &params.params, params.trial()).await;
        Ok(ExecuteResult {
            eval: params.eval.clone(),
            sample: params.sample.clone(),
            model: params.model.clone(),
            params: params.params.clone(),
            trial: params.trial,
            trials: params.trials,
            seed: params.seed,
            transcript,
            skipped: false,
        })
    }

    /// Score a supplied transcript with an eval's scorers, without re-executing
    /// the subject (the deferred-/re-scoring half of `run`). The model label
    /// need not still exist — scoring depends only on the eval + sample.
    async fn score(&self, params: &ScoreParams) -> Result<RunResult, String> {
        let eval = self
            .evals
            .iter()
            .find(|e| e.name == params.eval)
            .ok_or_else(|| format!("no such eval: {}", params.eval))?;
        let sample = eval
            .dataset
            .samples
            .iter()
            .find(|s| s.id == params.sample)
            .ok_or_else(|| format!("no such sample: {}/{}", params.eval, params.sample))?;

        let scores = score_transcript(eval, sample, &params.transcript).await;
        Ok(RunResult {
            eval: params.eval.clone(),
            sample: params.sample.clone(),
            model: params.model.clone(),
            params: params.params.clone(),
            trial: params.trial,
            trials: params.trials,
            seed: params.seed,
            passed: verdict(&scores),
            aggregate: aggregate_value(&scores),
            scores,
            transcript: TranscriptSummary::of(&params.transcript),
            skipped: false,
        })
    }

    /// Resolve a cell to its `(eval, sample, model)` definitions.
    fn locate(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
    ) -> Result<(&Eval, &crate::Sample, &crate::ModelSpec), String> {
        let e = self
            .evals
            .iter()
            .find(|e| e.name == eval)
            .ok_or_else(|| format!("no such eval: {eval}"))?;
        let s = e
            .dataset
            .samples
            .iter()
            .find(|s| s.id == sample)
            .ok_or_else(|| format!("no such sample: {eval}/{sample}"))?;
        let m = e
            .models
            .iter()
            .find(|m| m.label == model)
            .ok_or_else(|| format!("no such model: {eval}@{model}"))?;
        Ok((e, s, m))
    }
}

/// A skipped (unexecuted) cell result, e.g. when the model is unavailable.
fn skipped_result(params: &RunParams) -> RunResult {
    RunResult {
        eval: params.eval.clone(),
        sample: params.sample.clone(),
        model: params.model.clone(),
        params: params.params.clone(),
        trial: params.trial,
        trials: params.trials,
        seed: params.seed,
        passed: false,
        aggregate: 0.0,
        scores: Vec::new(),
        transcript: TranscriptSummary::default(),
        skipped: true,
    }
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

/// Serialize `value` as one line and write it under the shared stdout lock, so
/// concurrent tasks never interleave partial lines.
async fn write_line<T: serde::Serialize>(
    stdout: &Arc<Mutex<Stdout>>,
    value: &T,
) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(value).unwrap_or_default();
    buf.push(b'\n');
    let mut stdout = stdout.lock().await;
    stdout.write_all(&buf).await?;
    stdout.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::contains;
    use crate::subject::subject_fn;
    use crate::{Eval, Sample, Transcript};

    fn study() -> Study {
        Study::new().eval(
            Eval::new("greet")
                .describe("greeting eval")
                .meta("suite", "smoke")
                .sample(Sample::new("hi", "say hi").tag("smoke"))
                .subject(subject_fn(|_, _| async {
                    Transcript::response("hi there")
                }))
                .scorer(contains("hi"))
                .build(),
        )
    }

    #[test]
    fn list_advertises_everything() {
        let listing = study().list();
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
            trial: 0,
            trials: 0,
            seed: None,
        };
        let result = study().run(&params).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.transcript.final_response, "hi there");
    }

    #[tokio::test]
    async fn run_echoes_trial_identity_and_threads_seed() {
        // A study whose subject echoes its seed, so we can confirm the host's
        // trial/seed params reached the subject and round-tripped into the result.
        let s = Study::new().eval(
            Eval::new("rng")
                .case("a", "x")
                .trials(4)
                .subject(subject_fn(|_, cx| async move {
                    Transcript::response(format!("seed={:?}", cx.seed()))
                }))
                .scorer(contains("seed="))
                .build(),
        );
        let params = RunParams {
            eval: "rng".into(),
            sample: "a".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 2,
            trials: 4,
            seed: Some(77),
        };
        let result = s.run(&params).await.unwrap();
        assert_eq!(result.trial, 2);
        assert_eq!(result.trials, 4);
        assert_eq!(result.seed, Some(77));
        assert_eq!(result.key(), "rng/a@sim#2");
        assert!(result.transcript.final_response.contains("77"));
    }

    #[tokio::test]
    async fn run_rejects_unknown_eval() {
        let params = RunParams {
            eval: "nope".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
        };
        assert!(study().run(&params).await.is_err());
    }

    #[tokio::test]
    async fn execute_returns_full_transcript_without_scoring() {
        let params = RunParams {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
        };
        let captured = study().execute(&params).await.unwrap();
        assert!(!captured.skipped);
        assert_eq!(captured.transcript.final_response, "hi there");
    }

    #[tokio::test]
    async fn execute_then_score_matches_run() {
        let s = study();
        let rp = RunParams {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
        };
        let fused = s.run(&rp).await.unwrap();

        // Split path: execute, then score the captured transcript.
        let captured = s.execute(&rp).await.unwrap();
        let sp = ScoreParams {
            eval: captured.eval.clone(),
            sample: captured.sample.clone(),
            model: captured.model.clone(),
            params: captured.params.clone(),
            trial: captured.trial,
            trials: captured.trials,
            seed: captured.seed,
            transcript: captured.transcript.clone(),
        };
        let split = s.score(&sp).await.unwrap();

        assert_eq!(split.passed, fused.passed);
        assert_eq!(split.aggregate, fused.aggregate);
        assert_eq!(split.scores, fused.scores);
        assert_eq!(
            split.transcript.final_response,
            fused.transcript.final_response
        );
    }

    #[tokio::test]
    async fn score_is_repeatable_for_rescoring() {
        let s = study();
        let sp = ScoreParams {
            eval: "greet".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            transcript: Transcript::response("hi there"),
        };
        let first = s.score(&sp).await.unwrap();
        let second = s.score(&sp).await.unwrap();
        assert_eq!(first.scores, second.scores);
        assert!(first.passed && second.passed);
    }

    #[tokio::test]
    async fn score_rejects_unknown_eval() {
        let sp = ScoreParams {
            eval: "nope".into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            transcript: Transcript::response("x"),
        };
        assert!(study().score(&sp).await.is_err());
    }
}
