//! Environment capture for saved runs.
//!
//! Collects the context a result needs to be interpreted and compared later —
//! the git checkout, the box (OS/arch/host/CPU/memory), the host version, and
//! any labels configured in `mira.toml` or detected from CI. Enabled by default;
//! see [`crate::config::EnvironmentConfig`].
//!
//! Everything here is best-effort: capture must never fail or slow a run, so any
//! probe that errors, times out, or isn't available is silently dropped. The
//! serialized shape lives in the core ([`mira::run::Environment`]); this module
//! only does the gathering, which is why it lives in the host, not the library.

use std::collections::BTreeMap;
use std::process::Command;

use mira::run::{Environment, GitInfo};

/// CI environment variables surfaced as labels when present, mapped to stable
/// label keys. Keeps a saved run traceable back to the pipeline that produced
/// it without depending on any one CI provider.
const CI_LABELS: &[(&str, &str)] = &[
    ("ci", "CI"),
    ("ci.provider", "CI_PROVIDER"),
    // GitHub Actions.
    ("ci.workflow", "GITHUB_WORKFLOW"),
    ("ci.run_id", "GITHUB_RUN_ID"),
    ("ci.run_attempt", "GITHUB_RUN_ATTEMPT"),
    ("ci.actor", "GITHUB_ACTOR"),
    // GitLab CI.
    ("ci.pipeline_id", "CI_PIPELINE_ID"),
    // Generic / Buildkite / CircleCI job url.
    ("ci.job_url", "CI_JOB_URL"),
];

/// Collect the current environment, merging `labels` (from config) on top of the
/// auto-detected fields. Returns `None` when nothing could be captured, so the
/// caller stores no `environment` block rather than an empty one.
pub fn collect(extra_labels: &BTreeMap<String, String>) -> Option<Environment> {
    let mut env = Environment {
        git: git_info(),
        os: non_empty(std::env::consts::OS),
        arch: non_empty(std::env::consts::ARCH),
        hostname: hostname(),
        cpus: std::thread::available_parallelism().ok().map(|n| n.get()),
        mem_total_mib: mem_total_mib(),
        mira_version: non_empty(env!("CARGO_PKG_VERSION")),
        labels: BTreeMap::new(),
    };

    // Detected CI context first, then config labels — config wins on conflict so
    // an operator can override a noisy auto-detected value.
    for (key, var) in CI_LABELS {
        if let Ok(val) = std::env::var(var)
            && !val.is_empty()
        {
            env.labels.insert((*key).to_string(), val);
        }
    }
    for (k, v) in extra_labels {
        env.labels.insert(k.clone(), v.clone());
    }

    if env.is_empty() { None } else { Some(env) }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Resolve `HEAD`, the branch, and a dirty flag by shelling out to `git`. Any
/// failure (not a repo, no git on PATH) yields `None` — environment capture is
/// never allowed to break a run.
fn git_info() -> Option<GitInfo> {
    let commit = git(&["rev-parse", "HEAD"])?;
    // `--abbrev-ref HEAD` prints "HEAD" when detached; treat that as "no branch".
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    // Porcelain is empty on a clean tree; any line means uncommitted changes.
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    Some(GitInfo {
        commit,
        branch,
        dirty,
    })
}

/// Run `git ARGS` and return trimmed stdout, or `None` on any non-zero/error.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Best-effort hostname: the `HOSTNAME`/`COMPUTERNAME` env var, else the
/// `hostname` command. `None` when neither is available.
fn hostname() -> Option<String> {
    if let Ok(h) = std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME"))
        && !h.is_empty()
    {
        return Some(h);
    }
    let out = Command::new("hostname").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!h.is_empty()).then_some(h)
}

/// Total physical memory in MiB from `/proc/meminfo` (Linux). `None` elsewhere
/// or if the file isn't readable — no cross-platform memory probe without a
/// dependency, and a missing value is fine.
fn mem_total_mib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    // Line shape: `MemTotal:       16384256 kB`.
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_in_repo_has_git_and_box() {
        // Runs inside the mira git checkout, so git + os/arch/cpus are present.
        let env = collect(&BTreeMap::new()).expect("something is always captured");
        assert!(env.os.is_some());
        assert!(env.arch.is_some());
        assert!(env.cpus.unwrap() >= 1);
        assert!(env.mira_version.is_some());
        let git = env.git.expect("captured in a git checkout");
        assert_eq!(git.commit.len(), 40, "full sha: {}", git.commit);
    }

    #[test]
    fn config_labels_win_over_detected() {
        let mut labels = BTreeMap::new();
        labels.insert("team".to_string(), "search".to_string());
        let env = collect(&labels).unwrap();
        assert_eq!(env.labels.get("team").map(String::as_str), Some("search"));
    }
}
