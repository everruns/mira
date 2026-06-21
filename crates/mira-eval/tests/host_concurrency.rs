//! Host ↔ study integration: the host multiplexes many in-flight requests over
//! one stdio pipe to a single study process, demultiplexing responses by id.
//!
//! Uses the bundled polyglot Python study (no Mira dep, protocol 1.0) so the test
//! also pins backward compatibility: a 1.0 study with no `provider` field talks to
//! the concurrent 1.1 host. Skips cleanly if `python3` isn't on PATH.

use std::path::PathBuf;

use mira::Host;
use tokio::process::Command;

fn python_study() -> Option<Command> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/greet-python/study.py")
        .canonicalize()
        .ok()?;
    // Cheap PATH probe for python3.
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()?;
    let mut cmd = Command::new("python3");
    cmd.arg(script);
    Some(cmd)
}

#[tokio::test]
async fn host_handles_many_concurrent_runs() {
    let Some(cmd) = python_study() else {
        eprintln!("skipping: python3 not available");
        return;
    };

    let host = Host::spawn(cmd).await.expect("spawn study");
    let info = host.initialize("test-host").await.expect("initialize");
    assert_eq!(info.study, "greet-python");
    // A 1.0 study with no provider field still lists fine through the 1.1 host.
    let listing = host.list().await.expect("list");
    assert_eq!(listing.evals[0].models[0].label, "sim");

    // Fire many runs of the same cell concurrently over the one pipe; every one
    // must come back correctly correlated and passing.
    let handle = host.handle();
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let h = handle.clone();
        tasks.push(tokio::spawn(async move {
            h.run("greet", "hi", "sim", &Default::default()).await
        }));
    }
    for task in tasks {
        let result = task.await.expect("join").expect("run ok");
        assert!(result.passed, "cell should pass");
        assert_eq!(result.sample, "hi");
    }

    drop(handle);
    host.shutdown().await.expect("shutdown");
}
