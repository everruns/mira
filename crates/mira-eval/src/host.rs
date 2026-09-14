//! Host side of the eval protocol. Spawns the study process and issues
//! `initialize` / `list` / `run` requests, handling interleaved progress
//! notifications. The `mira` CLI (`mira-cli`) is the user-facing driver built on
//! top of this.
//!
//! ## What is mira's here, and what isn't
//!
//! The JSON-RPC plumbing is [`lanok::Peer`]'s: framing, id allocation,
//! correlating a response to the caller that is waiting for it, classifying a
//! line by its fields, noticing that a caller abandoned a request, failing every
//! in-flight call at once when the study goes away. None of that is specific to
//! evals, and mira used to carry its own copy.
//!
//! What stays here is the part that *is* mira's: which methods exist (declared
//! in [`crate::protocol`], so the calls below are generated stubs rather than
//! string literals), what `initialize` means, that an abandoned `run` is worth a
//! `cancel` request, and that a transcript is projected from its trajectory on
//! receipt.
//!
//! ## Concurrency
//!
//! A single study process serves **many in-flight requests at once**. The peer
//! owns the study's stdio, routes each response to the waiter that registered its
//! id, and hands notifications to the handler installed below. Requests may be
//! issued concurrently from any clone (see [`crate::exec`]); the cheaply-cloneable
//! [`HostHandle`] is what concurrent callers share.

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::Command;

use lanok::{Abandoned, ChildTransport, NdjsonTransport, Peer, PeerInfo, Transport};

use crate::protocol::{
    CancelParams, ExecuteResult, InitializeParams, InitializeResult, InitiatorApi,
    InitiatorDispatch, InitiatorHandler, ListResult, ListSamplesParams, ListSamplesResult,
    Notification, PROTOCOL_VERSION, RpcError, RunParams, RunResult, ScoreParams, capabilities,
    method,
};
use crate::{Params, Trial};

/// Callback invoked for each progress notification (e.g. to render a live log).
type EventCb = Arc<dyn Fn(&Notification) + Send + Sync>;

/// The methods worth cancelling when their caller goes away: the long ones that
/// cost money. `list`/`initialize` are short enough that a cancel would race the
/// response it was meant to pre-empt.
const CANCELABLE: &[&str] = &[method::RUN, method::EXECUTE, method::SCORE];

/// A cheaply-cloneable client over the study's framed stdio channel. Every method
/// takes `&self`, so clones can issue requests concurrently — responses are
/// demultiplexed by request `id`. Obtain one with [`Host::handle`].
#[derive(Clone)]
pub struct HostHandle {
    peer: Peer,
}

impl HostHandle {
    pub async fn initialize(&self, host_name: &str) -> Result<InitializeResult, RpcError> {
        // Not `Peer::handshake`: that one exchanges lanok's own `Hello`, and
        // mira's `initialize` answers with the eval catalogue. So the version
        // check and the capability record are done here, against the protocol's
        // own payloads, and the generated `NEGOTIATION` is what decides.
        let info: InitializeResult = self
            .peer
            .handshake_with(
                method::INITIALIZE,
                &InitializeParams {
                    protocol_version: PROTOCOL_VERSION.to_string(),
                    host: host_name.into(),
                },
            )
            .await?;

        // Forward/backward compatibility: a mismatched *major* is a hard
        // incompatibility; a differing minor is additive and tolerated.
        if !crate::protocol::version_compatible(&info.protocol_version) {
            return Err(RpcError::internal(format!(
                "incompatible protocol: study speaks {}, host speaks {} (major mismatch)",
                info.protocol_version, PROTOCOL_VERSION
            )));
        }

        // Recording what the study advertised is what lights up capability
        // gating: the generated stubs consult it before writing to the wire, so
        // an unsupported method is a typed local answer instead of a round trip
        // that ends in `method not found`.
        self.peer.record_peer(PeerInfo {
            name: info.study.clone(),
            version: None,
            capabilities: info.capabilities.iter().cloned().collect(),
        });
        Ok(info)
    }

    /// The raw `list` response: the eval catalogue with the **first page** of
    /// each eval's samples. When an eval's `next_cursor` is set, more samples
    /// remain — use [`list_complete`](HostHandle::list_complete) to fetch them
    /// all, or page manually with [`list_samples`](HostHandle::list_samples).
    pub async fn list(&self) -> Result<ListResult, RpcError> {
        self.peer.list().await
    }

    /// Fetch one more page of an eval's samples, continuing from `cursor` (an
    /// opaque token from a prior page). Studies advertising the `paginate`
    /// capability answer this; the host treats the cursor as opaque.
    pub async fn list_samples(
        &self,
        eval: &str,
        cursor: &str,
    ) -> Result<ListSamplesResult, RpcError> {
        self.peer
            .list_samples(ListSamplesParams {
                eval: eval.into(),
                cursor: cursor.into(),
            })
            .await
    }

    /// The full catalogue with **every** sample materialized: call `list`, then
    /// follow each eval's `next_cursor` via `list_samples` until exhausted,
    /// appending the pages onto `samples` and clearing the cursor. A study that
    /// fits its whole dataset in `list` (no cursor) costs no extra round-trips,
    /// so this is a safe drop-in for `list` on the host's planning path.
    pub async fn list_complete(&self) -> Result<ListResult, RpcError> {
        let mut listing = self.list().await?;
        for eval in &mut listing.evals {
            let mut cursor = eval.next_cursor.take();
            while let Some(c) = cursor {
                let page = self.list_samples(&eval.name, &c).await?;
                eval.samples.extend(page.samples);
                cursor = page.next_cursor;
            }
        }
        Ok(listing)
    }

    /// Whether the study advertised the `cancel` capability at `initialize`.
    pub fn supports_cancel(&self) -> bool {
        self.peer.supports(capabilities::CANCEL)
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
        // A study that can't cancel is "nothing to cancel", not an error: this
        // is the best-effort lever, and its callers branch on the bool. The
        // generated stub's own gate would answer `capability unsupported`.
        if !self.supports_cancel() {
            return Ok(false);
        }
        let result = self.peer.cancel(CancelParams { id: run_id }).await?;
        Ok(result.cancelled)
    }

    /// Run one matrix case. `params` carries the chosen value per extra axis
    /// (empty for a target-only matrix); `trial` carries the repetition index and
    /// seed (use [`Trial::single`] for an unrepeated case). Safe to call
    /// concurrently from clones.
    pub async fn run(
        &self,
        eval: &str,
        sample: &str,
        target: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<RunResult, RpcError> {
        self.peer
            .run(run_params(eval, sample, target, params, trial))
            .await
    }

    /// Execute one case's subject without scoring, returning the full transcript
    /// (for run-now, score-later). Requires the study to advertise the `execute`
    /// capability. Safe to call concurrently from clones.
    pub async fn execute(
        &self,
        eval: &str,
        sample: &str,
        target: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<ExecuteResult, RpcError> {
        let mut result = self
            .peer
            .execute(run_params(eval, sample, target, params, trial))
            .await?;
        // Normalize on receipt: a foreign study may return a trajectory-only
        // transcript (only `transcript.trajectory` set). Fill any flat fields
        // still at their defaults from the trajectory — never overwriting one
        // the study set — so persisted artifacts and deferred scoring see the
        // projections without any study-side cooperation.
        result.transcript.project_trajectory();
        Ok(result)
    }

    /// Score a previously-captured transcript without re-executing the subject
    /// (deferred scoring / re-scoring). Requires the study to advertise the
    /// `score` capability. Safe to call concurrently from clones.
    pub async fn score(&self, captured: &ExecuteResult) -> Result<RunResult, RpcError> {
        self.peer
            .score(ScoreParams {
                eval: captured.eval.clone(),
                sample: captured.sample.clone(),
                target: captured.target.clone(),
                params: captured.params.clone(),
                trial: captured.trial,
                trials: captured.trials,
                seed: captured.seed,
                transcript: captured.transcript.clone(),
            })
            .await
    }
}

/// Build the `run`/`execute` params for one case + trial. Trial fields ride
/// along so the study can echo the case's trial identity back (its key must match
/// the host's plan).
fn run_params(eval: &str, sample: &str, target: &str, params: &Params, trial: Trial) -> RunParams {
    RunParams {
        eval: eval.into(),
        sample: sample.into(),
        target: target.into(),
        params: params.clone(),
        trial: trial.index,
        trials: trial.count,
        seed: trial.seed,
    }
}

/// Forwards the study's notifications to the host's `on_event` callback.
///
/// This is the generated [`InitiatorHandler`]: the study is the responder, so
/// `event` and `log` are what it may send, and the dispatcher routes them here
/// with their params already typed. The callback stays untyped
/// ([`Notification`]) because that is the host's public surface; a malformed
/// notification is dropped by the dispatcher before it gets this far.
struct Events(Arc<std::sync::Mutex<EventCb>>);

impl Events {
    fn emit(&self, notification: Notification) {
        let cb = self.0.lock().expect("on_event mutex poisoned").clone();
        cb(&notification);
    }
}

impl InitiatorHandler for Events {
    fn event(&self, params: crate::protocol::EventParams) {
        self.emit(Notification::event(params));
    }

    fn log(&self, params: crate::protocol::LogParams) {
        self.emit(Notification::log(params.message, params.request_id));
    }
}

/// A study connection and the framed channel to it. Usually a spawned child
/// process ([`spawn`](Host::spawn)); also constructible over arbitrary pipes
/// ([`connect`](Host::connect)) for in-process tests.
pub struct Host {
    handle: HostHandle,
    /// Swappable progress callback, read by the notification handler per event.
    on_event: Arc<std::sync::Mutex<EventCb>>,
}

impl Host {
    /// Spawn `command` as the eval study. Its stderr is forwarded to the host's
    /// (build logs, tracing); only stdout carries protocol JSON. The peer starts
    /// reading immediately, demultiplexing responses and notifications.
    pub async fn spawn(command: Command) -> std::io::Result<Self> {
        // Forwarded rather than inherited, because the transport drains the
        // pipe: an unread stderr is what wedges a chatty study at ~8 KiB of
        // logging, and draining is also what puts a crashing study's last lines
        // in front of whoever is debugging it.
        let transport =
            ChildTransport::spawn_logging(command, Arc::new(|line: &str| eprintln!("{line}")))?;
        Ok(Self::with_transport(transport))
    }

    /// Connect to a study over arbitrary transports: `reader` carries the study's
    /// responses/notifications (host→study), `writer` carries the host's requests.
    /// The process-spawning [`spawn`](Host::spawn) is this over a child's stdio.
    pub fn connect<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::with_transport(NdjsonTransport::new(reader, writer))
    }

    /// Shared constructor: configure the peer over `transport` and wrap it in the
    /// cheaply-cloneable handle.
    fn with_transport(transport: impl Transport) -> Self {
        let on_event: Arc<std::sync::Mutex<EventCb>> =
            Arc::new(std::sync::Mutex::new(Arc::new(|_: &Notification| {})));

        let peer = Peer::builder()
            .handler(InitiatorDispatch::new(Events(on_event.clone())))
            // Cancel-on-drop. Lanok notices the abandonment and hands over the
            // id; what goes on the wire is mira's, and mira's cancel is an
            // acknowledged *request* rather than the usual fire-and-forget
            // notification. Gated on the study having said it can cancel, and on
            // the method being one worth cancelling.
            .on_abandon(Arc::new(|peer: &Peer, abandoned: &Abandoned| {
                if !peer.supports(capabilities::CANCEL)
                    || !CANCELABLE.contains(&abandoned.method.as_str())
                {
                    return;
                }
                let Some(id) = abandoned.id.as_number() else {
                    return;
                };
                // Needs a runtime to spawn the send; if there isn't one (a drop
                // during shutdown), skip it. No reply is awaited: the study's
                // ack arrives for a request nobody is waiting on, which is
                // exactly what best-effort cancellation wants.
                let peer = peer.clone();
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        let _ = peer.cancel(CancelParams { id }).await;
                    });
                }
            }))
            .connect(transport);

        Self {
            handle: HostHandle { peer },
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

    /// The full catalogue with every sample materialized (pages `list_samples`
    /// as needed). See [`HostHandle::list_complete`].
    pub async fn list_complete(&self) -> Result<ListResult, RpcError> {
        self.handle.list_complete().await
    }

    /// Run one matrix case (sequential convenience; see [`Host::handle`] for the
    /// concurrent path).
    pub async fn run(
        &self,
        eval: &str,
        sample: &str,
        target: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<RunResult, RpcError> {
        self.handle.run(eval, sample, target, params, trial).await
    }

    /// Execute one case's subject without scoring (sequential convenience; see
    /// [`HostHandle::execute`]).
    pub async fn execute(
        &self,
        eval: &str,
        sample: &str,
        target: &str,
        params: &Params,
        trial: Trial,
    ) -> Result<ExecuteResult, RpcError> {
        self.handle
            .execute(eval, sample, target, params, trial)
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

    /// Close the connection and wait for the study to exit. Closing stdin is the
    /// polite signal: the study's serve loop sees EOF and returns. Returns once
    /// the transport is released, which for a spawned study means the child has
    /// been reaped and its stderr drained.
    pub async fn shutdown(self) -> std::io::Result<()> {
        self.handle.peer.shutdown().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use lanok::Message;

    #[test]
    fn classifies_response_notification_and_reverse_request() {
        // The rule this host depends on, now lanok's. Kept as a test here
        // because the safety property is mira's to rely on: a reverse request
        // must never be read as a response. Pre-lanok, a study→host request
        // whose id collided with a host's in-flight id parsed as an "empty
        // response" and completed that unrelated request.

        // A response: id, no method.
        assert!(matches!(
            Message::from_line(r#"{"id":3,"result":{"ok":true}}"#),
            Ok(Message::Response { id, .. }) if id.as_number() == Some(3)
        ));
        // A notification: method, no id.
        assert!(matches!(
            Message::from_line(r#"{"method":"event","params":{"kind":"started"}}"#),
            Ok(Message::Notification { ref method, .. }) if method == "event"
        ));
        // A reverse request: id + method. Must NOT be seen as a response.
        assert!(matches!(
            Message::from_line(r#"{"id":1,"method":"broker_model","params":{}}"#),
            Ok(Message::Request { ref method, ref id, .. })
                if method == "broker_model" && id.as_number() == Some(1)
        ));
    }
}
