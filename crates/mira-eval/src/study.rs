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

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinSet;

use crate::eval::Eval;
use crate::protocol::{
    AxisInfo, CancelParams, CancelResult, EvalInfo, EventParams, ExecuteResult, InitializeResult,
    ListResult, ListSamplesParams, ListSamplesResult, ModelInfo, Notification, PROTOCOL_VERSION,
    Request, Response, RpcError, RunParams, RunResult, SampleInfo, ScoreParams, TranscriptSummary,
    capabilities, codes, event,
};
use crate::registry::registered_evals;
use crate::runner::{aggregate_value, execute_cell, run_cell, score_transcript, verdict};

/// The shared, line-serialized output sink. Boxed (not a concrete `Stdout`) so
/// the serve loop is testable over in-memory pipes via [`Study::serve_io`].
type SharedWriter = Arc<Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;

/// In-flight cancellable requests: request `id` → a signal that, when fired,
/// aborts that request's run via the task's `select!`. Held under a sync mutex
/// (never across an `.await`), like the host's pending map.
type Inflight = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<()>>>>;

/// Default samples-per-page when paginating `list`. Chosen so realistic small
/// studies (examples, smoke tests) fit in one page — `list` then behaves exactly
/// as before `1.10` — while a thousands-of-samples dataset (e.g. SWE-bench full)
/// is chunked across `list` + `list_samples` instead of one giant line.
pub const DEFAULT_PAGE_SIZE: usize = 500;

/// Your eval program: a named bundle of [`Eval`]s exposed to the host over the
/// protocol. Build one, then [`serve`](Study::serve) it.
pub struct Study {
    /// Name advertised to the host in `initialize` (defaults to the crate name).
    name: String,
    evals: Vec<Eval>,
    /// Max samples per `list`/`list_samples` page. `None` disables pagination
    /// (every sample is enumerated inline in `list`, the pre-`1.5` behaviour).
    page_size: Option<usize>,
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
            page_size: Some(DEFAULT_PAGE_SIZE),
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

    /// Set the samples-per-page for `list` pagination. `0` disables pagination
    /// (all samples are enumerated inline in `list`). Defaults to
    /// [`DEFAULT_PAGE_SIZE`]; lower it to chunk a very large dataset more
    /// aggressively, or raise it to send bigger pages.
    pub fn page_size(mut self, size: usize) -> Self {
        self.page_size = (size > 0).then_some(size);
        self
    }

    /// Serve this study over newline-delimited JSON on stdin/stdout until EOF.
    /// The host drives the loop; this returns when stdin closes.
    pub async fn serve(self) -> std::io::Result<()> {
        self.serve_io(tokio::io::stdin(), tokio::io::stdout()).await
    }

    /// Serve over arbitrary line-framed transports (e.g. in-memory pipes in
    /// tests). [`serve`](Study::serve) is this over real stdin/stdout.
    ///
    /// Requests are dispatched **concurrently**: each `run` is handled on its own
    /// task so a host can keep many cells in flight at once. Writes are serialized
    /// through a shared writer mutex (one whole line per lock), so responses and
    /// `event`/`log` notifications never interleave mid-line. The host bounds how
    /// many runs are in flight (see [`crate::exec`]).
    ///
    /// A `cancel` request aborts one in-flight `run`/`execute`/`score` by its
    /// request `id`: the run's task is dropped at its next await point and replies
    /// with a `cancelled` error, so the host's pending call resolves promptly
    /// instead of leaking until EOF. `cancel` is handled inline (not on a task)
    /// and is itself never cancellable.
    pub async fn serve_io<R, W>(self, reader: R, writer: W) -> std::io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let mut lines = BufReader::new(reader).lines();
        let out: SharedWriter = Arc::new(Mutex::new(Box::new(writer)));
        let me = Arc::new(self);
        let mut tasks: JoinSet<()> = JoinSet::new();
        let inflight: Inflight = Default::default();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let request: Request = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    // Can't correlate a malformed line to an id; report and move on.
                    write_line(&out, &Notification::log(format!("bad request: {e}"), 0)).await?;
                    continue;
                }
            };

            // `cancel` mutates the in-flight registry and must resolve promptly;
            // handle it inline rather than racing it against the runs it cancels.
            if request.method == "cancel" {
                let response = cancel(&request, &inflight);
                write_line(&out, &response).await?;
                continue;
            }

            // Register a cancel signal before spawning, so a `cancel` arriving the
            // instant after dispatch starts still finds it.
            let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
            inflight
                .lock()
                .expect("inflight mutex poisoned")
                .insert(request.id, cancel_tx);

            let me = me.clone();
            let out = out.clone();
            let inflight = inflight.clone();
            tasks.spawn(async move {
                let id = request.id;
                let response = tokio::select! {
                    resp = me.dispatch(&request, &out) => resp,
                    // Cancelled: drop the run future (stops work at its next await)
                    // and reply with an error the host correlates to its `run`.
                    _ = cancel_rx => Response::err(id, "cancelled"),
                };
                inflight
                    .lock()
                    .expect("inflight mutex poisoned")
                    .remove(&id);
                let _ = write_line(&out, &response).await;
            });
        }

        // Drain in-flight runs before returning so no response is lost on EOF.
        while tasks.join_next().await.is_some() {}
        Ok(())
    }

    async fn dispatch(&self, request: &Request, stdout: &SharedWriter) -> Response {
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
                        capabilities::CANCEL.into(),
                        capabilities::PAGINATE.into(),
                    ],
                    // EXPERIMENTAL: structured config for the advertised
                    // capabilities (event kinds, supported modalities). Gated —
                    // see InitializeResult::capability_params.
                    #[cfg(feature = "protocol-unstable")]
                    capability_params: advertised_capability_params(),
                }),
            ),
            "list" => Response::ok(request.id, json(&self.list())),
            "list_samples" => {
                let params: ListSamplesParams = match serde_json::from_value(request.params.clone())
                {
                    Ok(p) => p,
                    Err(e) => {
                        return Response::err(request.id, format!("bad list_samples params: {e}"));
                    }
                };
                match self.list_samples(&params) {
                    Ok(result) => Response::ok(request.id, json(&result)),
                    Err(e) => Response::err(request.id, e),
                }
            }
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
                // Progress so the host can render a live spinner / log. Each
                // event carries `request.id`, so the host correlates it to this
                // call even with many cells (or trials) multiplexed at once.
                let _ = write_line(stdout, &cell_event(request.id, &params, event::STARTED)).await;
                let result = self.run(&params).await;
                let _ = write_line(stdout, &cell_event(request.id, &params, event::FINISHED)).await;
                match result {
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
                let _ = write_line(stdout, &cell_event(request.id, &params, event::STARTED)).await;
                let result = self.execute(&params).await;
                let _ = write_line(stdout, &cell_event(request.id, &params, event::FINISHED)).await;
                match result {
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

    /// Build the `list` advertisement from this study's evals. Each eval carries
    /// the **first page** of its samples plus a `next_cursor` when more remain;
    /// the host fetches the rest with `list_samples`.
    pub fn list(&self) -> ListResult {
        let evals = self
            .evals
            .iter()
            .map(|eval| {
                let (samples, next_cursor) = self.sample_page(eval, 0);
                EvalInfo {
                    name: eval.name.clone(),
                    description: eval.description.clone(),
                    samples,
                    next_cursor,
                    scorers: eval.scorers.iter().map(|s| s.name()).collect(),
                    models: eval
                        .models
                        .iter()
                        .map(|m| ModelInfo {
                            label: m.label.clone(),
                            provider: m.provider.clone(),
                            available: m.available,
                            metadata: m.metadata.clone(),
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
                }
            })
            .collect();
        ListResult { evals }
    }

    /// Answer `list_samples`: the page of `eval`'s samples beginning at `cursor`.
    /// The cursor is the opaque token from a prior page (we encode it as the
    /// next sample offset); an unknown eval or malformed cursor is an error.
    pub fn list_samples(&self, params: &ListSamplesParams) -> Result<ListSamplesResult, String> {
        let eval = self
            .evals
            .iter()
            .find(|e| e.name == params.eval)
            .ok_or_else(|| format!("no such eval: {}", params.eval))?;
        let offset: usize = params
            .cursor
            .parse()
            .map_err(|_| format!("bad cursor: {}", params.cursor))?;
        let (samples, next_cursor) = self.sample_page(eval, offset);
        Ok(ListSamplesResult {
            samples,
            next_cursor,
        })
    }

    /// One page of an eval's samples starting at `offset`, plus the cursor for
    /// the page after it (`None` once the dataset is exhausted). With pagination
    /// disabled (`page_size == None`) a single page holds every remaining sample
    /// and there is never a next cursor — the pre-`1.10` behaviour.
    fn sample_page(&self, eval: &Eval, offset: usize) -> (Vec<SampleInfo>, Option<String>) {
        let all = &eval.dataset.samples;
        let start = offset.min(all.len());
        let end = match self.page_size {
            Some(p) => start.saturating_add(p).min(all.len()),
            None => all.len(),
        };
        let page = all[start..end]
            .iter()
            .map(|s| SampleInfo {
                id: s.id.clone(),
                tags: s.tags.clone(),
                metadata: s.metadata.clone(),
            })
            .collect();
        let next = (end < all.len()).then(|| end.to_string());
        (page, next)
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

/// Abort the in-flight request named by `params.id`, replying with whether one
/// was found. A miss (`cancelled: false`) is benign: the target already finished
/// or was never in flight. Synchronous — it only fires the run's cancel signal;
/// the run's own task writes its `cancelled` error and deregisters itself.
fn cancel(request: &Request, inflight: &Inflight) -> Response {
    let params: CancelParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::err_with(
                request.id,
                RpcError::new(format!("bad cancel params: {e}")).with_code(codes::INVALID_PARAMS),
            );
        }
    };
    let cancelled = inflight
        .lock()
        .expect("inflight mutex poisoned")
        .remove(&params.id)
        .is_some_and(|tx| tx.send(()).is_ok());
    Response::ok(request.id, json(&CancelResult { cancelled }))
}

fn json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// EXPERIMENTAL: the structured capability config advertised in `initialize` —
/// the `event` kinds this study emits and the content modalities it understands.
/// Keyed by capability token (see [`InitializeResult::capability_params`]).
#[cfg(feature = "protocol-unstable")]
fn advertised_capability_params() -> crate::Metadata {
    let modalities = serde_json::json!(["text", "image", "audio", "file", "json"]);
    crate::Metadata::from([
        (
            capabilities::EVENTS.to_string(),
            serde_json::json!({ "kinds": [event::STARTED, event::FINISHED] }),
        ),
        (
            "modalities".to_string(),
            serde_json::json!({ "input": modalities, "output": modalities }),
        ),
    ])
}

/// A typed cell-progress `event`, correlated to its request by `req_id`.
fn cell_event(req_id: u64, p: &RunParams, kind: &str) -> Notification {
    Notification::event(EventParams {
        request_id: req_id,
        eval: p.eval.clone(),
        sample: p.sample.clone(),
        model: p.model.clone(),
        params: p.params.clone(),
        kind: kind.into(),
        ..Default::default()
    })
}

/// Serialize `value` as one line and write it under the shared writer lock, so
/// concurrent tasks never interleave partial lines.
async fn write_line<T: serde::Serialize>(out: &SharedWriter, value: &T) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(value).unwrap_or_default();
    buf.push(b'\n');
    let mut out = out.lock().await;
    out.write_all(&buf).await?;
    out.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::contains;
    use crate::subject::subject_fn;
    use crate::{Eval, ModelSpec, Sample, Transcript};
    use serde_json::json;

    fn study() -> Study {
        Study::new().eval(
            Eval::new("greet")
                .describe("greeting eval")
                .meta("suite", "smoke")
                .sample(
                    Sample::new("hi", "say hi")
                        .tag("smoke")
                        .meta("difficulty", "easy"),
                )
                .subject(subject_fn(|_, _| async {
                    Transcript::response("hi there")
                }))
                .scorer(contains("hi"))
                .models([ModelSpec::sim().meta("agent", "demo")])
                .build(),
        )
    }

    #[cfg(feature = "protocol-unstable")]
    #[test]
    fn initialize_advertises_capability_params() {
        let init = advertised_capability_params();
        let info = InitializeResult {
            protocol_version: PROTOCOL_VERSION.into(),
            study: "x".into(),
            evals: 0,
            study_version: None,
            capabilities: vec![capabilities::EVENTS.into()],
            capability_params: init,
        };
        // Structured config keyed by capability token, readable via the accessor.
        let modalities = info.capability_param("modalities").unwrap();
        assert!(
            modalities["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "image")
        );
        assert!(info.capability_param("events").unwrap()["kinds"][0] == "started");
        assert!(info.capability_param("absent").is_none());
        // Round-trips on the wire when the feature is on.
        let back: InitializeResult =
            serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
        assert_eq!(back.capability_params, info.capability_params);
    }

    #[test]
    fn list_advertises_everything() {
        let listing = study().list();
        assert_eq!(listing.evals.len(), 1);
        let e = &listing.evals[0];
        assert_eq!(e.description, "greeting eval");
        assert_eq!(e.metadata.get("suite").unwrap(), "smoke");
        assert_eq!(e.samples[0].tags, vec!["smoke"]);
        // Per-sample and per-model metadata now ride their own wire columns (1.5).
        assert_eq!(e.samples[0].metadata.get("difficulty").unwrap(), "easy");
        assert_eq!(e.models[0].label, "sim");
        assert!(e.models[0].available);
        assert_eq!(e.models[0].metadata.get("agent").unwrap(), "demo");
    }

    fn big_study(samples: usize, page: usize) -> Study {
        let mut eval = Eval::new("big")
            .subject(subject_fn(|_, _| async { Transcript::response("ok") }))
            .scorer(contains("ok"));
        for i in 0..samples {
            eval = eval.sample(Sample::new(format!("s{i}"), "go"));
        }
        Study::new().page_size(page).eval(eval.build())
    }

    #[test]
    fn list_paginates_first_page_with_cursor() {
        let s = big_study(250, 100);
        let listing = s.list();
        let e = &listing.evals[0];
        assert_eq!(e.samples.len(), 100);
        assert_eq!(e.samples[0].id, "s0");
        assert_eq!(e.next_cursor.as_deref(), Some("100"));
    }

    #[test]
    fn list_samples_walks_every_page_then_stops() {
        let s = big_study(250, 100);
        // Reassemble the dataset by following cursors, as the host does.
        let mut ids: Vec<String> = s.list().evals[0]
            .samples
            .iter()
            .map(|x| x.id.clone())
            .collect();
        let mut cursor = s.list().evals[0].next_cursor.clone();
        while let Some(c) = cursor {
            let page = s
                .list_samples(&ListSamplesParams {
                    eval: "big".into(),
                    cursor: c,
                })
                .unwrap();
            ids.extend(page.samples.into_iter().map(|x| x.id));
            cursor = page.next_cursor;
        }
        assert_eq!(ids.len(), 250);
        assert_eq!(ids[0], "s0");
        assert_eq!(ids[249], "s249");
        // The final page (s200..s249, exactly one page) reports no continuation.
        let last = s
            .list_samples(&ListSamplesParams {
                eval: "big".into(),
                cursor: "200".into(),
            })
            .unwrap();
        assert_eq!(last.samples.len(), 50);
        assert!(last.next_cursor.is_none());
    }

    #[test]
    fn page_size_zero_disables_pagination() {
        let s = big_study(250, 0);
        let e = &s.list().evals[0];
        assert_eq!(e.samples.len(), 250);
        assert!(e.next_cursor.is_none());
    }

    #[test]
    fn list_samples_rejects_unknown_eval_and_bad_cursor() {
        let s = big_study(10, 5);
        assert!(
            s.list_samples(&ListSamplesParams {
                eval: "nope".into(),
                cursor: "0".into(),
            })
            .is_err()
        );
        assert!(
            s.list_samples(&ListSamplesParams {
                eval: "big".into(),
                cursor: "xyz".into(),
            })
            .is_err()
        );
        // An offset past the end is benign: an empty final page, no next cursor.
        let past = s
            .list_samples(&ListSamplesParams {
                eval: "big".into(),
                cursor: "999".into(),
            })
            .unwrap();
        assert!(past.samples.is_empty());
        assert!(past.next_cursor.is_none());
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

    /// A study whose only cell sleeps far longer than the test, so a `run`
    /// observably stays in flight until cancelled.
    fn slow_study() -> Study {
        Study::new().eval(
            Eval::new("slow")
                .sample(Sample::new("s", "go"))
                .subject(subject_fn(|_, _| async {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    Transcript::response("done")
                }))
                .scorer(contains("done"))
                .build(),
        )
    }

    /// Drive `serve_io` over in-memory pipes: a `cancel` aborts the in-flight
    /// `run` (which would otherwise sleep 30s), the run replies with a `cancelled`
    /// error, and the cancel reports it found the request.
    #[tokio::test]
    async fn cancel_aborts_inflight_run() {
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;

        let (mut host_w, study_r) = tokio::io::duplex(8192);
        let (study_w, host_r) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move { slow_study().serve_io(study_r, study_w).await });
        let mut reader = BufReader::new(host_r).lines();

        // Fire the slow run (id 1), then cancel it by that request id (id 2).
        host_w
            .write_all(
                b"{\"id\":1,\"method\":\"run\",\"params\":\
                  {\"eval\":\"slow\",\"sample\":\"s\",\"model\":\"sim\"}}\n",
            )
            .await
            .unwrap();
        host_w
            .write_all(b"{\"id\":2,\"method\":\"cancel\",\"params\":{\"id\":1}}\n")
            .await
            .unwrap();
        host_w.flush().await.unwrap();

        // Collect both responses, skipping the `event` notifications.
        let (mut run_resp, mut cancel_resp) = (None, None);
        while run_resp.is_none() || cancel_resp.is_none() {
            let line = tokio::time::timeout(Duration::from_secs(5), reader.next_line())
                .await
                .expect("response did not arrive — cancel did not abort the run")
                .expect("read line")
                .expect("study closed early");
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            match v.get("id").and_then(|i| i.as_u64()) {
                Some(1) => run_resp = Some(v),
                Some(2) => cancel_resp = Some(v),
                _ => {} // notification (no id)
            }
        }

        assert_eq!(cancel_resp.unwrap()["result"]["cancelled"], json!(true));
        let msg = run_resp.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(msg.contains("cancelled"), "run error was {msg:?}");

        drop(host_w);
        let _ = server.await;
    }

    /// Cancelling an `id` that isn't in flight (already done, or never sent) is a
    /// benign miss: `cancelled: false`, no error.
    #[tokio::test]
    async fn cancel_unknown_id_is_benign_miss() {
        use tokio::io::AsyncWriteExt;

        let (mut host_w, study_r) = tokio::io::duplex(8192);
        let (study_w, host_r) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move { study().serve_io(study_r, study_w).await });
        let mut reader = BufReader::new(host_r).lines();

        host_w
            .write_all(b"{\"id\":9,\"method\":\"cancel\",\"params\":{\"id\":123}}\n")
            .await
            .unwrap();
        host_w.flush().await.unwrap();

        let line = reader.next_line().await.unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], json!(9));
        assert_eq!(v["result"]["cancelled"], json!(false));

        drop(host_w);
        let _ = server.await;
    }
}
