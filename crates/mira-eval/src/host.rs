//! Host side of the eval protocol. Spawns the study process and issues
//! `initialize` / `list` / `run` requests, handling interleaved progress
//! notifications. The `mira` CLI (`mira-cli`) is the user-facing driver built on
//! top of this.
//!
//! ## Concurrency
//!
//! A single study process serves **many in-flight requests at once**. [`Host`]
//! spawns one reader task that owns the study's stdout, routes each response to
//! the waiter that registered its `id`, and dispatches notifications to the
//! `on_event` callback. Requests are written under a stdin mutex, so a caller can
//! fire several `run`s concurrently (see [`crate::exec`]) over the one pipe. The
//! cheaply-cloneable [`HostHandle`] is what concurrent callers share.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use crate::protocol::{
    CancelResult, ExecuteResult, InitializeResult, ListResult, Notification, PROTOCOL_VERSION,
    Request, Response, RpcError, RunParams, RunResult, ScoreParams, capabilities,
};
use crate::{Params, Trial};

/// Callback invoked for each progress notification (e.g. to render a live log).
type EventCb = Arc<dyn Fn(&Notification) + Send + Sync>;

/// One in-flight request's slot: the reader fulfils it by `id`. The error is the
/// structured [`RpcError`] so callers (and the executor) can classify/retry a
/// protocol-level failure without parsing the message.
type Pending =
    Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, RpcError>>>>>;

/// Boxed transports so the host works over both a child process's stdio and
/// in-memory pipes (the latter for in-process host↔study tests).
type BoxedWriter = Box<dyn AsyncWrite + Send + Unpin>;
type BoxedReader = Box<dyn AsyncRead + Send + Unpin>;

/// A cheaply-cloneable client over the study's framed stdio channel. Every method
/// takes `&self`, so clones can issue requests concurrently — responses are
/// demultiplexed by request `id`. Obtain one with [`Host::handle`].
#[derive(Clone)]
pub struct HostHandle {
    stdin: Arc<Mutex<BoxedWriter>>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    /// Set once `initialize` sees the study advertise the `cancel` capability.
    /// Gates both explicit [`cancel`](HostHandle::cancel) and cancel-on-drop, so
    /// the host never sends `cancel` to a study that wouldn't understand it.
    supports_cancel: Arc<AtomicBool>,
}

impl HostHandle {
    pub async fn initialize(&self, host_name: &str) -> Result<InitializeResult, RpcError> {
        let value = self
            .request(
                "initialize",
                serde_json::json!({ "protocol_version": PROTOCOL_VERSION, "host": host_name }),
                false,
            )
            .await?;
        let info: InitializeResult =
            serde_json::from_value(value).map_err(|e| RpcError::new(e.to_string()))?;
        // Forward/backward compatibility: a mismatched *major* is a hard
        // incompatibility; a differing minor is additive and tolerated.
        if !crate::protocol::version_compatible(&info.protocol_version) {
            return Err(RpcError::new(format!(
                "incompatible protocol: study speaks {}, host speaks {} (major mismatch)",
                info.protocol_version, PROTOCOL_VERSION
            )));
        }
        // Remember whether cancellation is available for later runs.
        let can_cancel = info.capabilities.iter().any(|c| c == capabilities::CANCEL);
        self.supports_cancel.store(can_cancel, Ordering::Relaxed);
        Ok(info)
    }

    pub async fn list(&self) -> Result<ListResult, RpcError> {
        let value = self.request("list", serde_json::Value::Null, false).await?;
        serde_json::from_value(value).map_err(|e| RpcError::new(e.to_string()))
    }

    /// Whether the study advertised the `cancel` capability at `initialize`.
    pub fn supports_cancel(&self) -> bool {
        self.supports_cancel.load(Ordering::Relaxed)
    }

    /// Ask the study to abort an in-flight `run`/`execute`/`score` by its request
    /// `id`. Returns whether the study found and cancelled it (`false` if it had
    /// already finished, was never in flight, or the study can't cancel).
    ///
    /// Most callers don't need the id: dropping a `run` future (e.g. via
    /// [`tokio::time::timeout`] or `select!` for fail-fast) already sends a
    /// best-effort cancel for that run. This is the explicit lever for when you
    /// hold the id and want the study's acknowledgement.
    pub async fn cancel(&self, run_id: u64) -> Result<bool, RpcError> {
        if !self.supports_cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let value = self
            .request("cancel", serde_json::json!({ "id": run_id }), false)
            .await?;
        let result: CancelResult =
            serde_json::from_value(value).map_err(|e| RpcError::new(e.to_string()))?;
        Ok(result.cancelled)
    }

    /// Run one matrix cell. `params` carries the chosen value per extra axis
    /// (empty for a model-only matrix); `trial` carries the repetition index and
    /// seed (use [`Trial::single`] for an unrepeated cell). Safe to call
    /// concurrently from clones.
    pub async fn run(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<RunResult, RpcError> {
        let params = run_params(eval, sample, model, params, trial);
        let value = self
            .request("run", serde_json::to_value(params).unwrap(), true)
            .await?;
        serde_json::from_value(value).map_err(|e| RpcError::new(e.to_string()))
    }

    /// Execute one cell's subject without scoring, returning the full transcript
    /// (for run-now, score-later). Requires the study to advertise the `execute`
    /// capability. Safe to call concurrently from clones.
    pub async fn execute(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<ExecuteResult, RpcError> {
        let params = run_params(eval, sample, model, params, trial);
        let value = self
            .request("execute", serde_json::to_value(params).unwrap(), true)
            .await?;
        serde_json::from_value(value).map_err(|e| RpcError::new(e.to_string()))
    }

    /// Score a previously-captured transcript without re-executing the subject
    /// (deferred scoring / re-scoring). Requires the study to advertise the
    /// `score` capability. Safe to call concurrently from clones.
    pub async fn score(&self, captured: &ExecuteResult) -> Result<RunResult, RpcError> {
        let params = ScoreParams {
            eval: captured.eval.clone(),
            sample: captured.sample.clone(),
            model: captured.model.clone(),
            params: captured.params.clone(),
            trial: captured.trial,
            trials: captured.trials,
            seed: captured.seed,
            transcript: captured.transcript.clone(),
        };
        let value = self
            .request("score", serde_json::to_value(params).unwrap(), true)
            .await?;
        serde_json::from_value(value).map_err(|e| RpcError::new(e.to_string()))
    }

    /// Send one request and await its correlated response. Concurrency-safe: the
    /// `id` is registered before the line is written, and the reader task routes
    /// the reply back here.
    ///
    /// `cancelable` arms cancel-on-drop: if the caller drops this future before
    /// the response arrives (a per-cell `timeout`, a fail-fast `select!`), the
    /// guard best-effort tells the study to abort the run — so an abandoned run
    /// stops burning cost instead of running to completion unobserved.
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        cancelable: bool,
    ) -> Result<serde_json::Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id, tx);

        // The guard frees the pending slot on every exit path (including the
        // caller dropping this future), so a leaked id can't pin the reader.
        let mut guard = RequestGuard {
            id,
            pending: self.pending.clone(),
            cancel: None,
            completed: false,
        };

        let request = Request {
            id,
            method: method.into(),
            params,
        };
        let mut line = serde_json::to_vec(&request).map_err(|e| RpcError::new(e.to_string()))?;
        line.push(b'\n');
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(&line)
                .await
                .map_err(|e| RpcError::new(e.to_string()))?;
            stdin
                .flush()
                .await
                .map_err(|e| RpcError::new(e.to_string()))?;
        }

        // The request is genuinely in flight now: arm cancel-on-drop (only for a
        // cancelable method against a study that supports it).
        if cancelable && self.supports_cancel.load(Ordering::Relaxed) {
            guard.cancel = Some((self.stdin.clone(), self.next_id.clone()));
        }

        let out = match rx.await {
            Ok(result) => result,
            // Reader dropped the sender without replying ⇒ the channel closed.
            Err(_) => Err(RpcError::new("study closed the connection")),
        };
        guard.completed = true;
        out
    }
}

/// Cleans up an in-flight request when its [`HostHandle::request`] future exits.
/// Always frees the pending slot; if armed and the future was dropped before the
/// response arrived, it also fires a best-effort `cancel` so the study aborts the
/// abandoned run.
struct RequestGuard {
    id: u64,
    pending: Pending,
    cancel: Option<(Arc<Mutex<BoxedWriter>>, Arc<AtomicU64>)>,
    completed: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(&self.id);
        if self.completed {
            return;
        }
        // Dropped before the response arrived. Fire-and-forget a cancel for this
        // run id (a fresh request id, no reply awaited). Needs a runtime to spawn
        // the write; if there isn't one (e.g. drop during shutdown), skip it.
        if let Some((stdin, next_id)) = self.cancel.take() {
            let run_id = self.id;
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    let _ = send_cancel(&stdin, &next_id, run_id).await;
                });
            }
        }
    }
}

/// Build the `run`/`execute` params for one cell + trial. Trial fields ride
/// along so the study can echo the cell's trial identity back (its key must match
/// the host's plan).
fn run_params(eval: &str, sample: &str, model: &str, params: &Params, trial: Trial) -> RunParams {
    RunParams {
        eval: eval.into(),
        sample: sample.into(),
        model: model.into(),
        params: params.clone(),
        trial: trial.index,
        trials: trial.count,
        seed: trial.seed,
    }
}

/// Write a fire-and-forget `cancel { id: run_id }` line. No pending slot is
/// registered: the study's ack arrives with an unknown id and the reader ignores
/// it, which is exactly what best-effort cancellation wants.
async fn send_cancel(
    stdin: &Arc<Mutex<BoxedWriter>>,
    next_id: &Arc<AtomicU64>,
    run_id: u64,
) -> std::io::Result<()> {
    let id = next_id.fetch_add(1, Ordering::SeqCst) + 1;
    let request = Request {
        id,
        method: "cancel".into(),
        params: serde_json::json!({ "id": run_id }),
    };
    let mut line = serde_json::to_vec(&request).unwrap_or_default();
    line.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin.write_all(&line).await?;
    stdin.flush().await
}

/// A study connection and the framed channel to it. Usually a spawned child
/// process ([`spawn`](Host::spawn)); also constructible over arbitrary pipes
/// ([`connect`](Host::connect)) for in-process tests.
pub struct Host {
    child: Option<Child>,
    handle: HostHandle,
    reader: Option<tokio::task::JoinHandle<()>>,
    /// Swappable progress callback, read by the reader task per notification.
    on_event: Arc<std::sync::Mutex<EventCb>>,
}

impl Host {
    /// Spawn `command` as the eval study. Its stderr is inherited (build logs,
    /// tracing); only stdout carries protocol JSON. A background reader task is
    /// started immediately to demultiplex responses and notifications.
    pub async fn spawn(mut command: Command) -> std::io::Result<Self> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self::with_io(
            Some(child),
            Box::new(stdout),
            Box::new(stdin),
        ))
    }

    /// Connect to a study over arbitrary transports: `reader` carries the study's
    /// responses/notifications (host→study), `writer` carries the host's requests.
    /// The process-spawning [`spawn`](Host::spawn) is this over a child's stdio.
    pub fn connect<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::with_io(None, Box::new(reader), Box::new(writer))
    }

    /// Shared constructor: wire the reader task and the cheaply-cloneable handle
    /// over the boxed transports.
    fn with_io(child: Option<Child>, reader: BoxedReader, writer: BoxedWriter) -> Self {
        let pending: Pending = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let on_event: Arc<std::sync::Mutex<EventCb>> =
            Arc::new(std::sync::Mutex::new(Arc::new(|_: &Notification| {})));

        let reader = tokio::spawn(reader_loop(
            BufReader::new(reader).lines(),
            pending.clone(),
            on_event.clone(),
        ));

        Self {
            child,
            handle: HostHandle {
                stdin: Arc::new(Mutex::new(writer)),
                pending,
                next_id: Arc::new(AtomicU64::new(0)),
                supports_cancel: Arc::new(AtomicBool::new(false)),
            },
            reader: Some(reader),
            on_event,
        }
    }

    /// Register a callback for progress notifications.
    pub fn on_event(self, f: impl Fn(&Notification) + Send + Sync + 'static) -> Self {
        *self.on_event.lock().expect("on_event mutex poisoned") = Arc::new(f);
        self
    }

    /// A cheaply-cloneable client for issuing requests, including concurrently.
    pub fn handle(&self) -> HostHandle {
        self.handle.clone()
    }

    pub async fn initialize(&self, host_name: &str) -> Result<InitializeResult, RpcError> {
        self.handle.initialize(host_name).await
    }

    pub async fn list(&self) -> Result<ListResult, RpcError> {
        self.handle.list().await
    }

    /// Run one matrix cell (sequential convenience; see [`Host::handle`] for the
    /// concurrent path).
    pub async fn run(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<RunResult, RpcError> {
        self.handle.run(eval, sample, model, params, trial).await
    }

    /// Execute one cell's subject without scoring (sequential convenience; see
    /// [`HostHandle::execute`]).
    pub async fn execute(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<ExecuteResult, RpcError> {
        self.handle
            .execute(eval, sample, model, params, trial)
            .await
    }

    /// Score a captured transcript without re-executing (sequential convenience;
    /// see [`HostHandle::score`]).
    pub async fn score(&self, captured: &ExecuteResult) -> Result<RunResult, RpcError> {
        self.handle.score(captured).await
    }

    /// Abort an in-flight run by its request `id` (see [`HostHandle::cancel`]).
    pub async fn cancel(&self, run_id: u64) -> Result<bool, RpcError> {
        self.handle.cancel(run_id).await
    }

    /// Close stdin and wait for the study to exit. Drops the host's own handle so
    /// that — once any outstanding [`HostHandle`] clones are gone — the study's
    /// stdin pipe closes and it sees EOF.
    pub async fn shutdown(mut self) -> std::io::Result<()> {
        drop(self.handle);
        if let Some(reader) = self.reader.take() {
            let _ = reader.await;
        }
        match self.child.take() {
            Some(mut child) => child.wait().await.map(|_| ()),
            None => Ok(()),
        }
    }
}

/// Read framed lines until EOF: route responses to their waiters by `id`, hand
/// notifications to `on_event`. On EOF, fail any still-pending requests.
async fn reader_loop(
    mut lines: Lines<BufReader<BoxedReader>>,
    pending: Pending,
    on_event: Arc<std::sync::Mutex<EventCb>>,
) {
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        // A response carries `id`; a notification carries `method` only.
        if let Ok(response) = serde_json::from_str::<Response>(&line) {
            let result = match (response.result, response.error) {
                (Some(result), _) => Ok(result),
                (None, Some(err)) => Err(err),
                (None, None) => Err(RpcError::new("empty response")),
            };
            if let Some(tx) = pending
                .lock()
                .expect("pending mutex poisoned")
                .remove(&response.id)
            {
                let _ = tx.send(result);
            }
            continue;
        }
        if let Ok(notification) = serde_json::from_str::<Notification>(&line) {
            let cb = on_event.lock().expect("on_event mutex poisoned").clone();
            cb(&notification);
        }
    }
    // EOF: nothing more will arrive, so unblock every outstanding waiter.
    let mut pending = pending.lock().expect("pending mutex poisoned");
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(RpcError::new("study closed the connection")));
    }
}
