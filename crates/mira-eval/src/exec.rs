//! Bounded, provider-aware, **adaptive** execution of a planned matrix.
//!
//! The host owns the run plan; this module decides *how many* cells run at once.
//! Three knobs, smallest-wins:
//!
//! 1. a **global** cap on total cells in flight;
//! 2. a **per-provider** cap, so a single provider (e.g. `anthropic`) can't be
//!    flooded even when the global budget is large;
//! 3. **adaptive reduction** — when a cell comes back rate-limited (HTTP 429,
//!    "overloaded", quota; see [`crate::is_rate_limited`]), that provider's
//!    in-flight limit is halved (AIMD multiplicative decrease) and a growing
//!    backoff is applied before its next dispatch; sustained success grows the
//!    limit back, one slot at a time, up to its ceiling. The rate-limited cell is
//!    re-queued (up to `max_retries`) rather than failed, so backing off actually
//!    rescues the run instead of dropping results.
//!
//! [`run_cells`] is generic over the per-cell run function so the scheduling
//! policy is unit-testable without a live study; the `mira` CLI passes a closure
//! that drives a [`HostHandle`](crate::HostHandle).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;

use crate::Params;
use crate::protocol::{RpcError, RunResult, TranscriptSummary};

/// Consecutive successes a provider needs before its limit grows by one.
const GROW_THRESHOLD: usize = 3;
/// Cap on the backoff exponent, so the delay can't grow without bound.
const MAX_BACKOFF_STEPS: u32 = 6;

/// One planned matrix cell to execute, with the provider it routes to (so the
/// scheduler can bucket concurrency). Identity matches [`crate::cell_key`].
#[derive(Clone, Debug)]
pub struct CellSpec {
    pub eval: String,
    pub sample: String,
    pub model: String,
    /// Provider id used for per-provider concurrency bucketing. Empty groups all
    /// such cells together (e.g. a foreign study that omits provider in `list`).
    pub provider: String,
    pub params: Params,
}

impl CellSpec {
    pub fn key(&self) -> String {
        crate::cell_key(&self.eval, &self.sample, &self.model, &self.params)
    }
}

/// Concurrency policy for a matrix run.
#[derive(Clone, Debug)]
pub struct Concurrency {
    /// Max total cells in flight across all providers.
    pub global: usize,
    /// Explicit per-provider ceilings (provider id → max in flight).
    pub per_provider: BTreeMap<String, usize>,
    /// Ceiling for providers without an explicit entry.
    pub default_per_provider: usize,
    /// Whether to shrink/grow per-provider limits in response to rate limits.
    pub adaptive: bool,
    /// Max times a rate-limited cell is re-queued before it is recorded failed.
    pub max_retries: u32,
    /// Base backoff applied after a rate limit (doubled per consecutive hit).
    pub base_backoff: Duration,
}

impl Concurrency {
    /// A policy with a global cap and, by default, the same ceiling per provider
    /// (so adaptive backoff — not a static per-provider cap — does the throttling
    /// until the caller sets explicit per-provider limits).
    pub fn new(global: usize) -> Self {
        let global = global.max(1);
        Self {
            global,
            per_provider: BTreeMap::new(),
            default_per_provider: global,
            adaptive: true,
            max_retries: 4,
            base_backoff: Duration::from_millis(500),
        }
    }

    /// Set the ceiling for one provider.
    pub fn provider(mut self, provider: impl Into<String>, limit: usize) -> Self {
        self.per_provider.insert(provider.into(), limit.max(1));
        self
    }
}

impl Default for Concurrency {
    fn default() -> Self {
        Self::new(8)
    }
}

/// Per-provider dynamic state for the AIMD controller.
#[derive(Debug)]
struct ProviderState {
    in_flight: usize,
    /// Current dynamic cap (`1..=ceiling`).
    limit: usize,
    /// Configured maximum the limit may grow back to.
    ceiling: usize,
    ok_streak: usize,
    /// Exponent for the next backoff window.
    backoff_steps: u32,
    /// No new cell for this provider starts before this instant.
    backoff_until: Option<Instant>,
}

/// The scheduling controller: tracks global + per-provider in-flight counts and
/// adapts per-provider limits to rate-limit feedback.
struct Limiter {
    providers: HashMap<String, ProviderState>,
    per_provider: BTreeMap<String, usize>,
    default_per_provider: usize,
    global_in_flight: usize,
    global_max: usize,
    adaptive: bool,
    base_backoff: Duration,
}

impl Limiter {
    fn new(cfg: &Concurrency) -> Self {
        Self {
            providers: HashMap::new(),
            per_provider: cfg.per_provider.clone(),
            default_per_provider: cfg.default_per_provider.max(1),
            global_in_flight: 0,
            global_max: cfg.global.max(1),
            adaptive: cfg.adaptive,
            base_backoff: cfg.base_backoff,
        }
    }

    fn ceiling_for(&self, provider: &str) -> usize {
        self.per_provider
            .get(provider)
            .copied()
            .unwrap_or(self.default_per_provider)
            .clamp(1, self.global_max)
    }

    fn state(&mut self, provider: &str) -> &mut ProviderState {
        let ceiling = self.ceiling_for(provider);
        self.providers
            .entry(provider.to_string())
            .or_insert_with(|| ProviderState {
                in_flight: 0,
                limit: ceiling,
                ceiling,
                ok_streak: 0,
                backoff_steps: 0,
                backoff_until: None,
            })
    }

    /// Can a cell for `provider` start right now (global budget, provider limit,
    /// and backoff window all permitting)?
    fn can_start(&mut self, provider: &str, now: Instant) -> bool {
        if self.global_in_flight >= self.global_max {
            return false;
        }
        let st = self.state(provider);
        st.in_flight < st.limit && st.backoff_until.is_none_or(|t| now >= t)
    }

    fn start(&mut self, provider: &str) {
        self.global_in_flight += 1;
        self.state(provider).in_flight += 1;
    }

    /// Record a finished cell and adapt the provider's limit.
    fn finish(&mut self, provider: &str, rate_limited: bool, now: Instant) {
        let adaptive = self.adaptive;
        let base = self.base_backoff;
        self.global_in_flight = self.global_in_flight.saturating_sub(1);
        let st = self.state(provider);
        st.in_flight = st.in_flight.saturating_sub(1);
        if !adaptive {
            return;
        }
        if rate_limited {
            // Multiplicative decrease + exponential backoff.
            st.limit = (st.limit / 2).max(1);
            st.backoff_steps = (st.backoff_steps + 1).min(MAX_BACKOFF_STEPS);
            let mult = 1u32 << (st.backoff_steps - 1);
            st.backoff_until = Some(now + base * mult);
            st.ok_streak = 0;
        } else {
            // Additive increase after a streak; relax the backoff exponent.
            st.ok_streak += 1;
            if st.ok_streak >= GROW_THRESHOLD {
                st.ok_streak = 0;
                st.backoff_steps = st.backoff_steps.saturating_sub(1);
                if st.limit < st.ceiling {
                    st.limit += 1;
                }
            }
        }
    }

    /// Earliest instant any still-pending provider leaves its backoff window.
    /// Used to sleep when every pending cell is blocked only by backoff.
    fn earliest_ready(&self, pending: &VecDeque<(CellSpec, u32)>) -> Option<Instant> {
        pending
            .iter()
            .filter_map(|(c, _)| self.providers.get(&c.provider))
            .filter_map(|s| s.backoff_until)
            .min()
    }
}

/// Whether a cell's outcome looks rate-limited — either an [`RpcError`] whose
/// message carries a known rate-limit phrase, or a transcript error with one.
fn outcome_rate_limited(res: &Result<RunResult, RpcError>) -> bool {
    match res {
        Err(e) => crate::is_rate_limited(&e.message),
        Ok(r) => r
            .transcript
            .error
            .as_deref()
            .is_some_and(crate::is_rate_limited),
    }
}

/// Whether a cell's outcome should be retried. For a protocol-level [`RpcError`]:
/// its structured `retryable` flag (set by the study/host for transient infra),
/// or a rate-limit phrase in the message. For a completed run: an
/// *infrastructure* transcript error (`error_kind = Infra` — budget, outage,
/// timeout) or a rate-limited transcript error. Not the model's fault either way,
/// so re-running may succeed. A non-retryable RPC error (bad params, unknown
/// method) is left alone — re-running won't help.
fn outcome_retryable(res: &Result<RunResult, RpcError>) -> bool {
    match res {
        Err(e) => e.retryable || crate::is_rate_limited(&e.message),
        Ok(r) => {
            r.transcript.error_kind == crate::ErrorKind::Infra
                || r.transcript
                    .error
                    .as_deref()
                    .is_some_and(crate::is_rate_limited)
        }
    }
}

/// Synthesize a failed result for a cell whose run errored at the protocol level
/// (so one cell's failure is recorded, not fatal to the whole matrix). A
/// retryable or rate-limited RPC error is infrastructure, not the model's fault.
fn failed_result(cell: &CellSpec, error: RpcError) -> RunResult {
    let infra = error.retryable || crate::is_rate_limited(&error.message);
    RunResult {
        eval: cell.eval.clone(),
        sample: cell.sample.clone(),
        model: cell.model.clone(),
        params: cell.params.clone(),
        passed: false,
        aggregate: 0.0,
        scores: Vec::new(),
        transcript: TranscriptSummary {
            error: Some(error.message),
            error_kind: if infra {
                crate::ErrorKind::Infra
            } else {
                crate::ErrorKind::Subject
            },
            ..Default::default()
        },
        skipped: false,
    }
}

/// Execute `cells` under the concurrency policy `cfg`, invoking `run` per cell and
/// reporting each finished cell to `on_done` (in completion order). `run` returns
/// the cell's [`RunResult`] or a transport error string; rate-limited outcomes are
/// re-queued up to `cfg.max_retries`.
///
/// `run` must be cheap to call and produce a `Send + 'static` future (the `mira`
/// CLI hands it a closure that clones a [`HostHandle`](crate::HostHandle)).
pub async fn run_cells<F, Fut>(
    cells: Vec<CellSpec>,
    cfg: &Concurrency,
    run: F,
    mut on_done: impl FnMut(&CellSpec, RunResult),
) where
    F: Fn(CellSpec) -> Fut,
    Fut: Future<Output = Result<RunResult, RpcError>> + Send + 'static,
{
    let mut limiter = Limiter::new(cfg);
    let mut pending: VecDeque<(CellSpec, u32)> = cells.into_iter().map(|c| (c, 0)).collect();
    let mut tasks: JoinSet<Result<RunResult, RpcError>> = JoinSet::new();
    // Side table so a finished (or panicked) task can be attributed back to its
    // cell: a JoinError carries only the task id, not the cell.
    let mut inflight: HashMap<tokio::task::Id, (CellSpec, u32)> = HashMap::new();

    loop {
        // Start as many cells as the global + per-provider budgets allow.
        loop {
            let now = Instant::now();
            let idx = pending
                .iter()
                .position(|(c, _)| limiter.can_start(&c.provider, now));
            let Some(idx) = idx else { break };
            let (cell, attempts) = pending.remove(idx).expect("index in bounds");
            limiter.start(&cell.provider);
            let task_cell = cell.clone();
            let fut = run(cell);
            let id = tasks.spawn(fut).id();
            inflight.insert(id, (task_cell, attempts));
        }

        if tasks.is_empty() {
            if pending.is_empty() {
                break;
            }
            // Everything left is blocked by a backoff window; wait it out.
            match limiter.earliest_ready(&pending) {
                Some(t) => {
                    tokio::time::sleep_until(t.into()).await;
                    continue;
                }
                // No backoff and still can't start ⇒ would spin; bail defensively.
                None => break,
            }
        }

        let Some(joined) = tasks.join_next_with_id().await else {
            continue;
        };
        // Map the task back to its cell either way, so the limiter's in-flight
        // counts are always released — even when the cell's future panicked.
        let (cell, attempts, res) = match joined {
            Ok((id, res)) => {
                let (cell, attempts) = inflight.remove(&id).expect("task id tracked");
                (cell, attempts, res)
            }
            Err(join_err) => {
                let (cell, attempts) = inflight.remove(&join_err.id()).expect("task id tracked");
                (
                    cell,
                    attempts,
                    Err(RpcError::new(format!("task panicked: {join_err}"))),
                )
            }
        };

        let rate_limited = outcome_rate_limited(&res);
        limiter.finish(&cell.provider, rate_limited, Instant::now());

        // Re-queue rate-limited *and* other infrastructure-errored cells (outage,
        // budget, timeout — not the model's fault) up to max_retries. Only rate
        // limits drive the AIMD throttle/backoff above; other infra errors get a
        // plain bounded retry.
        if attempts < cfg.max_retries && outcome_retryable(&res) {
            pending.push_back((cell, attempts + 1));
            continue;
        }

        let result = match res {
            Ok(result) => result,
            Err(error) => failed_result(&cell, error),
        };
        on_done(&cell, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    fn cell(provider: &str, id: &str) -> CellSpec {
        CellSpec {
            eval: "e".into(),
            sample: id.into(),
            model: format!("{provider}/m"),
            provider: provider.into(),
            params: Params::new(),
        }
    }

    fn ok_result(cell: &CellSpec) -> RunResult {
        RunResult {
            eval: cell.eval.clone(),
            sample: cell.sample.clone(),
            model: cell.model.clone(),
            params: cell.params.clone(),
            passed: true,
            aggregate: 1.0,
            scores: Vec::new(),
            transcript: TranscriptSummary::default(),
            skipped: false,
        }
    }

    #[test]
    fn limiter_buckets_per_provider() {
        let cfg = Concurrency::new(10).provider("anthropic", 2);
        let mut lim = Limiter::new(&cfg);
        let now = Instant::now();
        assert!(lim.can_start("anthropic", now));
        lim.start("anthropic");
        lim.start("anthropic");
        // Provider ceiling of 2 reached even though the global budget is 10.
        assert!(!lim.can_start("anthropic", now));
        // A different provider is unaffected.
        assert!(lim.can_start("openai", now));
    }

    #[test]
    fn rate_limit_halves_and_backs_off() {
        let cfg = Concurrency::new(8); // default per-provider ceiling = 8
        let mut lim = Limiter::new(&cfg);
        let now = Instant::now();
        lim.start("anthropic");
        lim.finish("anthropic", true, now);
        let st = &lim.providers["anthropic"];
        assert_eq!(st.limit, 4); // 8 -> 4
        assert!(st.backoff_until.is_some());
        // Within the backoff window, no new start.
        assert!(!lim.can_start("anthropic", now));
    }

    #[test]
    fn sustained_success_grows_back() {
        let cfg = Concurrency::new(8);
        let mut lim = Limiter::new(&cfg);
        let now = Instant::now();
        // Knock the limit down first.
        lim.start("anthropic");
        lim.finish("anthropic", true, now); // limit -> 4
        assert_eq!(lim.providers["anthropic"].limit, 4);
        // GROW_THRESHOLD clean finishes bump it by one.
        for _ in 0..GROW_THRESHOLD {
            lim.start("anthropic");
            lim.finish("anthropic", false, now);
        }
        assert_eq!(lim.providers["anthropic"].limit, 5);
    }

    #[tokio::test]
    async fn runs_every_cell_once() {
        let cells: Vec<CellSpec> = (0..20).map(|i| cell("sim", &i.to_string())).collect();
        let cfg = Concurrency::new(4);
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = seen.clone();
        let mut done = Vec::new();
        run_cells(
            cells,
            &cfg,
            move |c| {
                let seen = seen2.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    Ok(ok_result(&c))
                }
            },
            |_, r| done.push(r),
        )
        .await;
        assert_eq!(seen.load(Ordering::SeqCst), 20);
        assert_eq!(done.len(), 20);
        assert!(done.iter().all(|r| r.passed));
    }

    #[tokio::test]
    async fn respects_global_concurrency_cap() {
        // Track peak concurrency; it must never exceed the global cap.
        let cells: Vec<CellSpec> = (0..30).map(|i| cell("sim", &i.to_string())).collect();
        let cfg = Concurrency::new(3);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (a, p) = (active.clone(), peak.clone());
        let mut done = 0usize;
        run_cells(
            cells,
            &cfg,
            move |c| {
                let (a, p) = (a.clone(), p.clone());
                async move {
                    let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    a.fetch_sub(1, Ordering::SeqCst);
                    Ok(ok_result(&c))
                }
            },
            |_, _| done += 1,
        )
        .await;
        assert_eq!(done, 30);
        assert!(peak.load(Ordering::SeqCst) <= 3, "peak exceeded global cap");
    }

    #[tokio::test]
    async fn retries_rate_limited_cell_then_succeeds() {
        // Fail with a 429 on the first attempt, succeed after.
        let cfg = Concurrency {
            base_backoff: Duration::from_millis(1),
            ..Concurrency::new(2)
        };
        let attempts = Arc::new(Mutex::new(0usize));
        let a = attempts.clone();
        let mut results = Vec::new();
        run_cells(
            vec![cell("anthropic", "x")],
            &cfg,
            move |c| {
                let a = a.clone();
                async move {
                    let mut n = a.lock().await;
                    *n += 1;
                    if *n == 1 {
                        Err(RpcError::new("HTTP 429 rate limit"))
                    } else {
                        Ok(ok_result(&c))
                    }
                }
            },
            |_, r| results.push(r),
        )
        .await;
        assert_eq!(*attempts.lock().await, 2);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    /// An infra-errored result (`error_kind = Infra`) that is *not* a rate limit
    /// is still re-queued, then succeeds.
    #[tokio::test]
    async fn retries_infra_errored_cell_then_succeeds() {
        let cfg = Concurrency::new(2);
        let attempts = Arc::new(Mutex::new(0usize));
        let a = attempts.clone();
        let mut results = Vec::new();
        run_cells(
            vec![cell("sim", "x")],
            &cfg,
            move |c| {
                let a = a.clone();
                async move {
                    let mut n = a.lock().await;
                    *n += 1;
                    if *n == 1 {
                        let mut r = ok_result(&c);
                        r.passed = false;
                        r.transcript.error = Some("provider 503 unavailable".into());
                        r.transcript.error_kind = crate::ErrorKind::Infra;
                        Ok(r)
                    } else {
                        Ok(ok_result(&c))
                    }
                }
            },
            |_, r| results.push(r),
        )
        .await;
        assert_eq!(*attempts.lock().await, 2); // re-queued once
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    /// A protocol-level `RpcError` flagged `retryable` is re-queued even when its
    /// message carries no rate-limit phrase — classification comes from the
    /// structured flag, not string-matching. The give-up path also records it as
    /// an infra error.
    #[tokio::test]
    async fn retries_retryable_rpc_error_then_succeeds() {
        let cfg = Concurrency::new(2);
        let attempts = Arc::new(Mutex::new(0usize));
        let a = attempts.clone();
        let mut results = Vec::new();
        run_cells(
            vec![cell("sim", "x")],
            &cfg,
            move |c| {
                let a = a.clone();
                async move {
                    let mut n = a.lock().await;
                    *n += 1;
                    if *n == 1 {
                        Err(RpcError::new("provider outage").retryable())
                    } else {
                        Ok(ok_result(&c))
                    }
                }
            },
            |_, r| results.push(r),
        )
        .await;
        assert_eq!(*attempts.lock().await, 2); // re-queued once
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    /// A non-retryable `RpcError` (e.g. bad params) is not re-queued; it fails the
    /// cell once and is recorded as a subject error.
    #[tokio::test]
    async fn non_retryable_rpc_error_is_not_requeued() {
        let cfg = Concurrency::new(2);
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        let mut results = Vec::new();
        run_cells(
            vec![cell("sim", "x")],
            &cfg,
            move |_| {
                let c2 = c2.clone();
                async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                    Err(RpcError::new("bad run params"))
                }
            },
            |_, r| results.push(r),
        )
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 1); // no retry
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert_eq!(results[0].transcript.error_kind, crate::ErrorKind::Subject);
    }

    #[tokio::test]
    async fn panicking_cell_is_recorded_and_frees_its_slot() {
        // Global cap of 1: if a panic leaked the in-flight count, the second
        // cell could never start and this test would hang.
        let cfg = Concurrency::new(1);
        let cells = vec![cell("sim", "boom"), cell("sim", "ok")];
        let mut results = Vec::new();
        run_cells(
            cells,
            &cfg,
            move |c| async move {
                if c.sample == "boom" {
                    panic!("subject blew up");
                }
                Ok(ok_result(&c))
            },
            |c, r| results.push((c.sample.clone(), r)),
        )
        .await;
        assert_eq!(results.len(), 2);
        let boom = results.iter().find(|(s, _)| s == "boom").unwrap();
        assert!(!boom.1.passed);
        assert!(
            boom.1
                .transcript
                .error
                .as_deref()
                .unwrap()
                .contains("panic")
        );
        let ok = results.iter().find(|(s, _)| s == "ok").unwrap();
        assert!(ok.1.passed);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let cfg = Concurrency {
            base_backoff: Duration::from_millis(1),
            max_retries: 2,
            ..Concurrency::new(1)
        };
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        let mut results = Vec::new();
        run_cells(
            vec![cell("anthropic", "x")],
            &cfg,
            move |_| {
                let c2 = c2.clone();
                async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                    Err(RpcError::new("429 too many requests"))
                }
            },
            |_, r| results.push(r),
        )
        .await;
        // Initial attempt + max_retries re-queues.
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].transcript.error.is_some());
    }
}
