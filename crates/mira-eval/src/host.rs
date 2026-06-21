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
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};

use crate::Metadata;
use crate::protocol::{
    ExecuteResult, InitializeResult, ListResult, Notification, PROTOCOL_VERSION, Request, Response,
    RunParams, RunResult, ScoreParams,
};

/// Callback invoked for each progress notification (e.g. to render a live log).
type EventCb = Arc<dyn Fn(&Notification) + Send + Sync>;

/// One in-flight request's slot: the reader fulfils it by `id`.
type Pending =
    Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>;

/// A cheaply-cloneable client over the study's framed stdio channel. Every method
/// takes `&self`, so clones can issue requests concurrently — responses are
/// demultiplexed by request `id`. Obtain one with [`Host::handle`].
#[derive(Clone)]
pub struct HostHandle {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl HostHandle {
    pub async fn initialize(&self, host_name: &str) -> Result<InitializeResult, String> {
        let value = self
            .request(
                "initialize",
                serde_json::json!({ "protocol_version": PROTOCOL_VERSION, "host": host_name }),
            )
            .await?;
        let info: InitializeResult = serde_json::from_value(value).map_err(|e| e.to_string())?;
        // Forward/backward compatibility: a mismatched *major* is a hard
        // incompatibility; a differing minor is additive and tolerated.
        if !crate::protocol::version_compatible(&info.protocol_version) {
            return Err(format!(
                "incompatible protocol: study speaks {}, host speaks {} (major mismatch)",
                info.protocol_version, PROTOCOL_VERSION
            ));
        }
        Ok(info)
    }

    pub async fn list(&self) -> Result<ListResult, String> {
        let value = self.request("list", serde_json::Value::Null).await?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    /// Run one matrix cell. `params` carries the chosen value per extra axis
    /// (empty for a model-only matrix). Safe to call concurrently from clones.
    pub async fn run(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Metadata,
    ) -> Result<RunResult, String> {
        let params = RunParams {
            eval: eval.into(),
            sample: sample.into(),
            model: model.into(),
            params: params.clone(),
        };
        let value = self
            .request("run", serde_json::to_value(params).unwrap())
            .await?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    /// Execute one cell's subject without scoring, returning the full transcript
    /// (for run-now, score-later). Requires the study to advertise the `execute`
    /// capability. Safe to call concurrently from clones.
    pub async fn execute(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Metadata,
    ) -> Result<ExecuteResult, String> {
        let params = RunParams {
            eval: eval.into(),
            sample: sample.into(),
            model: model.into(),
            params: params.clone(),
        };
        let value = self
            .request("execute", serde_json::to_value(params).unwrap())
            .await?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    /// Score a previously-captured transcript without re-executing the subject
    /// (deferred scoring / re-scoring). Requires the study to advertise the
    /// `score` capability. Safe to call concurrently from clones.
    pub async fn score(&self, captured: &ExecuteResult) -> Result<RunResult, String> {
        let params = ScoreParams {
            eval: captured.eval.clone(),
            sample: captured.sample.clone(),
            model: captured.model.clone(),
            params: captured.params.clone(),
            transcript: captured.transcript.clone(),
        };
        let value = self
            .request("score", serde_json::to_value(params).unwrap())
            .await?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    /// Send one request and await its correlated response. Concurrency-safe: the
    /// `id` is registered before the line is written, and the reader task routes
    /// the reply back here.
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id, tx);

        let request = Request {
            id,
            method: method.into(),
            params,
        };
        let mut line = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        line.push(b'\n');
        // If the write fails, drop the pending slot too — otherwise the entry
        // leaks and the reader holds a sender for a request that never completes.
        let write = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(&line).await?;
            stdin.flush().await
        };
        if let Err(e) = write.await {
            self.pending
                .lock()
                .expect("pending mutex poisoned")
                .remove(&id);
            return Err(e.to_string());
        }

        match rx.await {
            Ok(result) => result,
            // Reader dropped the sender without replying ⇒ the channel closed.
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&id);
                Err("study closed the connection".into())
            }
        }
    }
}

/// A spawned study process and the framed stdio channel to it.
pub struct Host {
    child: Child,
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

        let pending: Pending = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let on_event: Arc<std::sync::Mutex<EventCb>> =
            Arc::new(std::sync::Mutex::new(Arc::new(|_: &Notification| {})));

        let reader = tokio::spawn(reader_loop(
            BufReader::new(stdout).lines(),
            pending.clone(),
            on_event.clone(),
        ));

        Ok(Self {
            child,
            handle: HostHandle {
                stdin: Arc::new(Mutex::new(stdin)),
                pending,
                next_id: Arc::new(AtomicU64::new(0)),
            },
            reader: Some(reader),
            on_event,
        })
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

    pub async fn initialize(&self, host_name: &str) -> Result<InitializeResult, String> {
        self.handle.initialize(host_name).await
    }

    pub async fn list(&self) -> Result<ListResult, String> {
        self.handle.list().await
    }

    /// Run one matrix cell (sequential convenience; see [`Host::handle`] for the
    /// concurrent path).
    pub async fn run(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Metadata,
    ) -> Result<RunResult, String> {
        self.handle.run(eval, sample, model, params).await
    }

    /// Execute one cell's subject without scoring (sequential convenience; see
    /// [`HostHandle::execute`]).
    pub async fn execute(
        &self,
        eval: &str,
        sample: &str,
        model: &str,
        params: &Metadata,
    ) -> Result<ExecuteResult, String> {
        self.handle.execute(eval, sample, model, params).await
    }

    /// Score a captured transcript without re-executing (sequential convenience;
    /// see [`HostHandle::score`]).
    pub async fn score(&self, captured: &ExecuteResult) -> Result<RunResult, String> {
        self.handle.score(captured).await
    }

    /// Close stdin and wait for the study to exit. Drops the host's own handle so
    /// that — once any outstanding [`HostHandle`] clones are gone — the study's
    /// stdin pipe closes and it sees EOF.
    pub async fn shutdown(mut self) -> std::io::Result<()> {
        drop(self.handle);
        if let Some(reader) = self.reader.take() {
            let _ = reader.await;
        }
        self.child.wait().await.map(|_| ())
    }
}

/// Read framed lines until EOF: route responses to their waiters by `id`, hand
/// notifications to `on_event`. On EOF, fail any still-pending requests.
async fn reader_loop(
    mut lines: Lines<BufReader<ChildStdout>>,
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
                (None, Some(err)) => Err(err.message),
                (None, None) => Err("empty response".into()),
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
        let _ = tx.send(Err("study closed the connection".into()));
    }
}
