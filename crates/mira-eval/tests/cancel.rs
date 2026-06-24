//! End-to-end cancellation across the host↔study seam, wired over in-memory
//! pipes (no child process): a real [`Host`] talks to a real [`Study`] running a
//! deliberately slow case. Covers both levers — an explicit
//! [`HostHandle::cancel`] by request id, and cancel-on-drop, where abandoning a
//! `run` future (a per-case timeout / fail-fast) aborts the study-side run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mira::scorer::contains;
use mira::subject::subject_fn;
use mira::{Eval, Host, Sample, Study, Transcript, Trial};

/// Fires when a run aborts before completing (its future is dropped mid-sleep).
struct AbortFlag {
    aborted: Arc<AtomicBool>,
    completed: bool,
}

impl Drop for AbortFlag {
    fn drop(&mut self) {
        if !self.completed {
            self.aborted.store(true, Ordering::SeqCst);
        }
    }
}

/// A study whose only case signals when it starts, then sleeps far past the test
/// before returning — so a `run` stays observably in flight until cancelled, and
/// `aborted` flips iff the run was dropped rather than allowed to finish.
fn slow_study(started: Arc<tokio::sync::Notify>, aborted: Arc<AtomicBool>) -> Study {
    Study::new().eval(
        Eval::new("slow")
            .add_sample(Sample::new("s", "go"))
            .subject(subject_fn(move |_, _| {
                let started = started.clone();
                let aborted = aborted.clone();
                async move {
                    started.notify_one();
                    let mut guard = AbortFlag {
                        aborted,
                        completed: false,
                    };
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    guard.completed = true;
                    Transcript::response("done")
                }
            }))
            .scorer(contains("done"))
            .build(),
    )
}

/// Wire a host to an in-process study over two in-memory pipes.
fn wire(study: Study) -> Host {
    let (host_w, study_r) = tokio::io::duplex(8192);
    let (study_w, host_r) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        let _ = study.serve_io(study_r, study_w).await;
    });
    Host::connect(host_r, host_w)
}

/// Poll `flag` until set, or panic after `within`.
async fn await_flag(flag: &Arc<AtomicBool>, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    while !flag.load(Ordering::SeqCst) {
        if tokio::time::Instant::now() >= deadline {
            panic!("flag not set within {within:?}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn explicit_cancel_aborts_inflight_run() {
    let started = Arc::new(tokio::sync::Notify::new());
    let aborted = Arc::new(AtomicBool::new(false));
    let host = wire(slow_study(started.clone(), aborted.clone()));

    let info = host.initialize("test-host").await.expect("initialize"); // request id 1
    assert!(
        info.capabilities.iter().any(|c| c == "cancel"),
        "study should advertise the cancel capability",
    );
    assert!(host.handle().supports_cancel());

    // Fire the slow run on a task so it stays in flight; it is request id 2.
    let handle = host.handle();
    let run = tokio::spawn(async move {
        handle
            .run("slow", "s", "sim", &Default::default(), Trial::single())
            .await
    });

    // Wait until the study has actually begun the run, then cancel id 2.
    started.notified().await;
    let cancelled = host.cancel(2).await.expect("cancel");
    assert!(
        cancelled,
        "study should report it cancelled the in-flight run"
    );

    // The run resolves promptly with a cancellation error (not after 30s).
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("run did not resolve after cancel")
        .expect("join");
    let err = result.expect_err("a cancelled run is an error");
    assert!(err.message.contains("cancelled"), "run error was {err:?}");
    await_flag(&aborted, Duration::from_secs(5)).await;

    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dropping_a_run_future_cancels_it() {
    let started = Arc::new(tokio::sync::Notify::new());
    let aborted = Arc::new(AtomicBool::new(false));
    let host = wire(slow_study(started.clone(), aborted.clone()));

    host.initialize("test-host").await.expect("initialize");
    let handle = host.handle();

    // A per-case timeout drops the run future; cancel-on-drop must abort the
    // study-side run rather than leaving it to sleep out the full 30s.
    let timed = tokio::time::timeout(
        Duration::from_millis(300),
        handle.run("slow", "s", "sim", &Default::default(), Trial::single()),
    )
    .await;
    assert!(timed.is_err(), "the slow run should have timed out");

    // The study observes the abort shortly after the future was dropped.
    await_flag(&aborted, Duration::from_secs(5)).await;

    // Drop the last handle so the study sees EOF (else shutdown waits forever).
    drop(handle);
    host.shutdown().await.expect("shutdown");
}
