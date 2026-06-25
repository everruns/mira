//! Bounded, provider-aware, **adaptive** execution of a planned matrix.
//!
//! The host owns the run plan; this module decides *how many* cases run at once.
//! Three knobs, smallest-wins:
//!
//! 1. a **global** cap on total cases in flight;
//! 2. a **per-provider** cap, so a single provider (e.g. `anthropic`) can't be
//!    flooded even when the global budget is large;
//! 3. **adaptive reduction** — when a case comes back rate-limited (HTTP 429,
//!    "overloaded", quota; see [`crate::is_rate_limited`]), that provider's
//!    in-flight limit is halved (AIMD multiplicative decrease) and a growing
//!    backoff is applied before its next dispatch; sustained success grows the
//!    limit back, one slot at a time, up to its ceiling. The rate-limited case is
//!    re-queued (up to `max_retries`) rather than failed, so backing off actually
//!    rescues the run instead of dropping results.
//!
//! [`run_cases`] is generic over the per-case run function so the scheduling
//! policy is unit-testable without a live study; the `mira` CLI passes a closure
//! that drives a [`HostHandle`](crate::HostHandle).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;

use crate::protocol::{RpcError, RunResult, TranscriptSummary};
use crate::{Params, Trial};

/// Consecutive successes a provider needs before its limit grows by one.
const GROW_THRESHOLD: usize = 3;
/// Cap on the backoff exponent, so the delay can't grow without bound.
const MAX_BACKOFF_STEPS: u32 = 6;

/// One planned matrix case to execute, with the provider it routes to (so the
/// scheduler can bucket concurrency). Identity matches [`crate::case_key`].
#[derive(Clone, Debug)]
pub struct CaseSpec {
    pub eval: String,
    pub sample: String,
    pub target: String,
    /// Provider id used for per-provider concurrency bucketing. Empty groups all
    /// such cases together (e.g. a foreign study that omits provider in `list`).
    pub provider: String,
    pub params: Params,
    /// Which trial of this case to run (index, count, seed). [`Trial::single`]
    /// for an unrepeated case — its key then has no `#index` suffix.
    pub trial: Trial,
    /// Wall-clock budget for this case. When set and exceeded, the scheduler
    /// drops the case's future — which best-effort cancels the in-flight run (see
    /// [`HostHandle`](crate::HostHandle) cancel-on-drop) — and records the case
    /// failed with a timeout error. `None` ⇒ no time limit. Resolved per target by
    /// the host (CLI `--timeout` / `mira.toml` per-target / preset).
    pub timeout: Option<Duration>,
}

impl CaseSpec {
    /// Trial-aware case identity (a `#index` suffix when the case is repeated).
    pub fn key(&self) -> String {
        format!("{}{}", self.logical_key(), self.trial.key_suffix())
    }

    /// Case identity shared by all trials of this case (no `#index` suffix).
    pub fn logical_key(&self) -> String {
        crate::case_key(&self.eval, &self.sample, &self.target, &self.params)
    }
}

/// Concurrency policy for a matrix run.
#[derive(Clone, Debug)]
pub struct Concurrency {
    /// Max total cases in flight across all providers.
    pub global: usize,
    /// Explicit per-provider ceilings (provider id → max in flight).
    pub per_provider: BTreeMap<String, usize>,
    /// Ceiling for providers without an explicit entry.
    pub default_per_provider: usize,
    /// Whether to shrink/grow per-provider limits in response to rate limits.
    pub adaptive: bool,
    /// Max times a rate-limited case is re-queued before it is recorded failed.
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
    /// No new case for this provider starts before this instant.
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

    /// Can a case for `provider` start right now (global budget, provider limit,
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

    /// Record a finished case and adapt the provider's limit.
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
    /// Used to sleep when every pending case is blocked only by backoff.
    fn earliest_ready(&self, pending: &VecDeque<(CaseSpec, u32)>) -> Option<Instant> {
        pending
            .iter()
            .filter_map(|(c, _)| self.providers.get(&c.provider))
            .filter_map(|s| s.backoff_until)
            .min()
    }
}

/// Whether a case's outcome looks rate-limited — either an [`RpcError`] whose
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

/// Whether a case's outcome should be retried. For a protocol-level [`RpcError`]:
/// its structured `retryable` flag (set by the study/host for transient infra),
/// or a rate-limit phrase in the message. For a completed run: an
/// *infrastructure* transcript error (`error_kind = Infra` — budget, outage,
/// timeout) or a rate-limited transcript error. Not the target's fault either way,
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

/// Synthesize a failed result for a case whose run errored at the protocol level
/// (so one case's failure is recorded, not fatal to the whole matrix). A
/// retryable or rate-limited RPC error is infrastructure, not the target's fault.
fn failed_result(case: &CaseSpec, error: RpcError) -> RunResult {
    let infra = error.retryable || crate::is_rate_limited(&error.message);
    RunResult {
        eval: case.eval.clone(),
        sample: case.sample.clone(),
        target: case.target.clone(),
        params: case.params.clone(),
        trial: case.trial.index,
        trials: case.trial.count,
        seed: case.trial.seed,
        input: Vec::new(),
        expected: None,
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

/// Execute `cases` under the concurrency policy `cfg`, invoking `run` per case and
/// reporting each finished case to `on_done` (in completion order). `run` returns
/// the case's [`RunResult`] or a transport error string; rate-limited outcomes are
/// re-queued up to `cfg.max_retries`.
///
/// `run` must be cheap to call and produce a `Send + 'static` future (the `mira`
/// CLI hands it a closure that clones a [`HostHandle`](crate::HostHandle)).
pub async fn run_cases<F, Fut>(
    cases: Vec<CaseSpec>,
    cfg: &Concurrency,
    run: F,
    mut on_done: impl FnMut(&CaseSpec, RunResult),
) where
    F: Fn(CaseSpec) -> Fut,
    Fut: Future<Output = Result<RunResult, RpcError>> + Send + 'static,
{
    let mut limiter = Limiter::new(cfg);
    let mut pending: VecDeque<(CaseSpec, u32)> = cases.into_iter().map(|c| (c, 0)).collect();
    let mut tasks: JoinSet<Result<RunResult, RpcError>> = JoinSet::new();
    // Side table so a finished (or panicked) task can be attributed back to its
    // case: a JoinError carries only the task id, not the case.
    let mut inflight: HashMap<tokio::task::Id, (CaseSpec, u32)> = HashMap::new();

    loop {
        // Start as many cases as the global + per-provider budgets allow.
        loop {
            let now = Instant::now();
            let idx = pending
                .iter()
                .position(|(c, _)| limiter.can_start(&c.provider, now));
            let Some(idx) = idx else { break };
            let (case, attempts) = pending.remove(idx).expect("index in bounds");
            limiter.start(&case.provider);
            let task_case = case.clone();
            let timeout = case.timeout;
            let fut = run(case);
            // Per-case wall-clock budget: dropping the timed-out future best-effort
            // cancels the in-flight run (HostHandle cancel-on-drop), so an
            // over-budget case stops burning cost instead of running unobserved.
            // A timeout is recorded as a non-retryable failure — retrying would
            // just burn the same budget again — so it isn't re-queued below.
            let task = async move {
                match timeout {
                    Some(dur) => match tokio::time::timeout(dur, fut).await {
                        Ok(res) => res,
                        Err(_) => Err(RpcError::new(format!(
                            "timed out after {}s (target timeout)",
                            dur.as_secs()
                        ))),
                    },
                    None => fut.await,
                }
            };
            let id = tasks.spawn(task).id();
            inflight.insert(id, (task_case, attempts));
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
        // Map the task back to its case either way, so the limiter's in-flight
        // counts are always released — even when the case's future panicked.
        let (case, attempts, res) = match joined {
            Ok((id, res)) => {
                let (case, attempts) = inflight.remove(&id).expect("task id tracked");
                (case, attempts, res)
            }
            Err(join_err) => {
                let (case, attempts) = inflight.remove(&join_err.id()).expect("task id tracked");
                (
                    case,
                    attempts,
                    Err(RpcError::new(format!("task panicked: {join_err}"))),
                )
            }
        };

        let rate_limited = outcome_rate_limited(&res);
        limiter.finish(&case.provider, rate_limited, Instant::now());

        // Re-queue rate-limited *and* other infrastructure-errored cases (outage,
        // budget, timeout — not the target's fault) up to max_retries. Only rate
        // limits drive the AIMD throttle/backoff above; other infra errors get a
        // plain bounded retry.
        if attempts < cfg.max_retries && outcome_retryable(&res) {
            pending.push_back((case, attempts + 1));
            continue;
        }

        let result = match res {
            Ok(result) => result,
            Err(error) => failed_result(&case, error),
        };
        on_done(&case, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    fn case(provider: &str, id: &str) -> CaseSpec {
        CaseSpec {
            eval: "e".into(),
            sample: id.into(),
            target: format!("{provider}/m"),
            provider: provider.into(),
            params: Params::new(),
            trial: Trial::single(),
            timeout: None,
        }
    }

    fn ok_result(case: &CaseSpec) -> RunResult {
        RunResult {
            eval: case.eval.clone(),
            sample: case.sample.clone(),
            target: case.target.clone(),
            params: case.params.clone(),
            trial: case.trial.index,
            trials: case.trial.count,
            seed: case.trial.seed,
            input: Vec::new(),
            expected: None,
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
    async fn runs_every_case_once() {
        let cases: Vec<CaseSpec> = (0..20).map(|i| case("sim", &i.to_string())).collect();
        let cfg = Concurrency::new(4);
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = seen.clone();
        let mut done = Vec::new();
        run_cases(
            cases,
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
        let cases: Vec<CaseSpec> = (0..30).map(|i| case("sim", &i.to_string())).collect();
        let cfg = Concurrency::new(3);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (a, p) = (active.clone(), peak.clone());
        let mut done = 0usize;
        run_cases(
            cases,
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
    async fn retries_rate_limited_case_then_succeeds() {
        // Fail with a 429 on the first attempt, succeed after.
        let cfg = Concurrency {
            base_backoff: Duration::from_millis(1),
            ..Concurrency::new(2)
        };
        let attempts = Arc::new(Mutex::new(0usize));
        let a = attempts.clone();
        let mut results = Vec::new();
        run_cases(
            vec![case("anthropic", "x")],
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
    async fn retries_infra_errored_case_then_succeeds() {
        let cfg = Concurrency::new(2);
        let attempts = Arc::new(Mutex::new(0usize));
        let a = attempts.clone();
        let mut results = Vec::new();
        run_cases(
            vec![case("sim", "x")],
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
        run_cases(
            vec![case("sim", "x")],
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
    /// case once and is recorded as a subject error.
    #[tokio::test]
    async fn non_retryable_rpc_error_is_not_requeued() {
        let cfg = Concurrency::new(2);
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        let mut results = Vec::new();
        run_cases(
            vec![case("sim", "x")],
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
    async fn panicking_case_is_recorded_and_frees_its_slot() {
        // Global cap of 1: if a panic leaked the in-flight count, the second
        // case could never start and this test would hang.
        let cfg = Concurrency::new(1);
        let cases = vec![case("sim", "boom"), case("sim", "ok")];
        let mut results = Vec::new();
        run_cases(
            cases,
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
        run_cases(
            vec![case("anthropic", "x")],
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

    /// A case that outruns its per-case timeout is given up on: recorded once as a
    /// failure with a timeout error, and *not* retried (retrying would burn the
    /// same budget again). The run-count proves it ran a single attempt.
    #[tokio::test]
    async fn times_out_slow_case_without_retrying() {
        let cfg = Concurrency {
            base_backoff: Duration::from_millis(1),
            max_retries: 4,
            ..Concurrency::new(2)
        };
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        let slow = CaseSpec {
            timeout: Some(Duration::from_millis(10)),
            ..case("sim", "slow")
        };
        let mut results = Vec::new();
        run_cases(
            vec![slow],
            &cfg,
            move |c| {
                let c2 = c2.clone();
                async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                    // Outlast the 10ms budget.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(ok_result(&c))
                }
            },
            |_, r| results.push(r),
        )
        .await;
        assert_eq!(count.load(Ordering::SeqCst), 1, "no retry after timeout");
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        let err = results[0].transcript.error.as_deref().unwrap();
        assert!(err.contains("timed out"), "{err}");
        // A timeout is the target's fault, not infra: it fails the case (red CI).
        assert_eq!(results[0].transcript.error_kind, crate::ErrorKind::Subject);
    }

    /// A case that finishes within its timeout is unaffected.
    #[tokio::test]
    async fn fast_case_under_timeout_passes() {
        let cfg = Concurrency::new(2);
        let fast = CaseSpec {
            timeout: Some(Duration::from_secs(30)),
            ..case("sim", "fast")
        };
        let mut results = Vec::new();
        run_cases(
            vec![fast],
            &cfg,
            move |c| async move { Ok(ok_result(&c)) },
            |_, r| results.push(r),
        )
        .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }
}
