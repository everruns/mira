//! Resumable evaluation sessions — the checkpoint as a first-class record.
//!
//! The host (the `mira` CLI) persists a [`Session`] as it drives a run, one
//! write per completed cell. A re-run loads it to:
//! * resume with **accurate progress** — the session carries the planned cell
//!   `total`, so a progress bar can show `done/total` immediately instead of
//!   only counting cells run in the current process; and
//! * **warn on stale results** — it fingerprints each eval's advertised
//!   definition (scorers, axes, models, samples, metadata, max_turns) at save
//!   time, so a resume can detect evals whose definition changed and flag their
//!   cached cells rather than silently reusing them.
//!
//! Design note: the session lives host-side on purpose. The study returns
//! per-cell [`RunResult`]s and knows nothing about checkpoints (see
//! `specs/architecture.md` §7); this module is the durable wrapper the host owns.
//!
//! Staleness is detected at eval granularity. Sample *content* is not visible in
//! the advertised `list` (only sample ids/tags are), so editing a prompt without
//! touching the definition won't be flagged — use `--fresh` when in doubt.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Metadata;
use crate::protocol::{EvalInfo, ListResult, RunResult};

/// On-disk format version for the session/checkpoint file. Bumped on a breaking
/// change to the layout; an unrecognised file is treated as "no session" and the
/// run starts fresh (pre-1.0: no migration of old checkpoints).
pub const SESSION_FORMAT: u32 = 1;

/// A resumable evaluation session: run metadata plus the per-cell results
/// gathered so far. This is the unit persisted to the `--checkpoint` path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// Layout version; see [`SESSION_FORMAT`].
    pub format: u32,
    /// Study name (from `initialize`), for diagnostics and stale-study warnings.
    pub study: String,
    /// Study version, when the study advertises one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study_version: Option<String>,
    /// Total planned cells for the run this session belongs to. Lets a resume
    /// report `done/total` without re-deriving the plan.
    pub total: usize,
    /// Unix seconds when the session was first created.
    pub created_unix: u64,
    /// Unix seconds of the last write (one per completed cell).
    pub updated_unix: u64,
    /// Per-eval fingerprint of the advertised definition at save time, keyed by
    /// eval name. A resume compares these against the current listing to flag
    /// stale cells. See [`fingerprints`].
    #[serde(default)]
    pub fingerprints: BTreeMap<String, String>,
    /// The per-cell results gathered so far.
    pub results: Vec<RunResult>,
}

impl Session {
    /// Start a fresh session for a planned run.
    pub fn new(
        study: impl Into<String>,
        study_version: Option<String>,
        total: usize,
        fingerprints: BTreeMap<String, String>,
    ) -> Self {
        let now = now_unix();
        Self {
            format: SESSION_FORMAT,
            study: study.into(),
            study_version,
            total,
            created_unix: now,
            updated_unix: now,
            fingerprints,
            results: Vec::new(),
        }
    }

    /// Load a session from `path`.
    ///
    /// `Ok(None)` means the file simply doesn't exist (a normal first run).
    /// `Err` means it exists but couldn't be used — unreadable, malformed, or a
    /// different format version — so the caller can warn before starting fresh
    /// rather than silently discarding a checkpoint.
    pub fn load(path: &str) -> std::io::Result<Option<Session>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let session: Session = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if session.format != SESSION_FORMAT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unsupported session format {} (expected {SESSION_FORMAT})",
                    session.format
                ),
            ));
        }
        Ok(Some(session))
    }

    /// Persist the session to `path` as pretty JSON, stamping `updated_unix`.
    pub fn save(&mut self, path: &str) -> std::io::Result<()> {
        self.updated_unix = now_unix();
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, text)
    }

    /// Replace the results and refresh `total`/`fingerprints` for the current
    /// plan, keeping `created_unix`. Used as the host records each cell.
    pub fn update(
        &mut self,
        total: usize,
        fingerprints: BTreeMap<String, String>,
        results: Vec<RunResult>,
    ) {
        self.total = total;
        self.fingerprints = fingerprints;
        self.results = results;
    }

    /// Keys of cached cells whose eval definition changed since they were
    /// recorded — i.e. the current listing's fingerprint differs from the one
    /// stored in this session. Cells for evals no longer in the plan are not
    /// reported (they're unused, not stale).
    pub fn stale_keys(&self, current: &BTreeMap<String, String>) -> Vec<String> {
        self.results
            .iter()
            .filter(
                |r| match (self.fingerprints.get(&r.eval), current.get(&r.eval)) {
                    (Some(stored), Some(now)) => stored != now,
                    // No definition on file but the eval is still planned: can't
                    // vouch for the cache, so treat as stale.
                    (None, Some(_)) => true,
                    // Eval dropped from the plan: unused, not stale.
                    (_, None) => false,
                },
            )
            .map(|r| r.key())
            .collect()
    }
}

/// Unix seconds now (0 if the clock is before the epoch).
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Per-eval fingerprint of the advertised definitions in `listing`, keyed by
/// eval name. Two listings with identical eval definitions produce identical
/// fingerprints; any change to scorers, axes, models, samples (ids/tags),
/// metadata, or `max_turns` changes them.
pub fn fingerprints(listing: &ListResult) -> BTreeMap<String, String> {
    listing
        .evals
        .iter()
        .map(|e| (e.name.clone(), fingerprint(e)))
        .collect()
}

/// Stable fingerprint of one eval's advertised definition.
///
/// Deliberately *not* `DefaultHasher`: std's SipHash algorithm is not contracted
/// to be stable across Rust releases, so a toolchain bump could silently change
/// fingerprints and flag every cached cell as stale. Instead we serialize the
/// relevant fields to canonical JSON (deterministic field/`BTreeMap` order) and
/// hash the bytes with FNV-1a — a fixed algorithm that won't drift. Not
/// cryptographic; it only needs to change when the definition changes. The
/// scheme is tied to [`SESSION_FORMAT`]: bump that if this changes.
fn fingerprint(eval: &EvalInfo) -> String {
    /// The subset of an eval's definition that, if changed, invalidates cached
    /// results. `available` is intentionally excluded (it tracks env, not the
    /// definition). Field/iteration order is deterministic, so JSON is canonical.
    #[derive(Serialize)]
    struct Fingerprintable<'a> {
        scorers: &'a [String],
        max_turns: usize,
        axes: Vec<(&'a str, &'a [String])>,
        models: Vec<&'a str>,
        samples: Vec<(&'a str, &'a [String])>,
        metadata: &'a Metadata,
    }

    let fp = Fingerprintable {
        scorers: &eval.scorers,
        max_turns: eval.max_turns,
        axes: eval
            .axes
            .iter()
            .map(|a| (a.name.as_str(), a.values.as_slice()))
            .collect(),
        models: eval.models.iter().map(|m| m.label.as_str()).collect(),
        samples: eval
            .samples
            .iter()
            .map(|s| (s.id.as_str(), s.tags.as_slice()))
            .collect(),
        metadata: &eval.metadata,
    };
    let bytes = serde_json::to_vec(&fp).expect("fingerprint serialization is infallible");
    format!("{:016x}", fnv1a(&bytes))
}

/// 64-bit FNV-1a. A small, dependency-free, version-stable hash.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AxisInfo, ModelInfo, SampleInfo, TranscriptSummary};

    fn eval_info(scorers: &[&str]) -> EvalInfo {
        EvalInfo {
            name: "greet".into(),
            description: String::new(),
            samples: vec![SampleInfo {
                id: "hi".into(),
                tags: vec![],
            }],
            scorers: scorers.iter().map(|s| s.to_string()).collect(),
            models: vec![ModelInfo {
                label: "sim".into(),
                provider: "sim".into(),
                available: true,
            }],
            axes: vec![AxisInfo {
                name: "effort".into(),
                values: vec!["low".into(), "high".into()],
            }],
            max_turns: 4,
            metadata: Default::default(),
        }
    }

    fn result(eval: &str) -> RunResult {
        RunResult {
            eval: eval.into(),
            sample: "hi".into(),
            model: "sim".into(),
            params: Default::default(),
            passed: true,
            aggregate: 1.0,
            scores: vec![],
            transcript: TranscriptSummary::default(),
            skipped: false,
        }
    }

    #[test]
    fn fingerprint_is_stable_and_change_sensitive() {
        let a = fingerprint(&eval_info(&["contains"]));
        let b = fingerprint(&eval_info(&["contains"]));
        assert_eq!(a, b, "same definition ⇒ same fingerprint");

        let c = fingerprint(&eval_info(&["contains", "regex"]));
        assert_ne!(a, c, "added scorer ⇒ different fingerprint");
    }

    #[test]
    fn stale_keys_flags_changed_evals_only() {
        let listing = ListResult {
            evals: vec![eval_info(&["contains"])],
        };
        let mut session = Session::new("study", None, 1, fingerprints(&listing));
        session.results = vec![result("greet")];

        // Unchanged listing ⇒ nothing stale.
        assert!(session.stale_keys(&fingerprints(&listing)).is_empty());

        // Changed scorers ⇒ the greet cell is stale.
        let changed = ListResult {
            evals: vec![eval_info(&["contains", "regex"])],
        };
        let stale = session.stale_keys(&fingerprints(&changed));
        assert_eq!(stale, vec!["greet/hi@sim".to_string()]);
    }

    #[test]
    fn stale_keys_ignores_dropped_evals() {
        let listing = ListResult {
            evals: vec![eval_info(&["contains"])],
        };
        let mut session = Session::new("study", None, 1, fingerprints(&listing));
        session.results = vec![result("gone")]; // eval not in any current listing
        assert!(session.stale_keys(&fingerprints(&listing)).is_empty());
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let path = path.to_str().unwrap();

        let listing = ListResult {
            evals: vec![eval_info(&["contains"])],
        };
        let mut session = Session::new("study", Some("0.1.0".into()), 3, fingerprints(&listing));
        session.results = vec![result("greet")];
        session.save(path).unwrap();

        let back = Session::load(path).unwrap().expect("loads");
        assert_eq!(back.study, "study");
        assert_eq!(back.total, 3);
        assert_eq!(back.results.len(), 1);
        assert_eq!(back.results[0].key(), "greet/hi@sim");
    }

    #[test]
    fn load_missing_is_ok_none() {
        assert!(Session::load("/no/such/session.json").unwrap().is_none());
    }

    #[test]
    fn load_corrupt_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        let path = path.to_str().unwrap();
        std::fs::write(path, "{ not valid json").unwrap();
        assert!(Session::load(path).is_err());

        // Recognisable JSON but a future format version is also an error.
        std::fs::write(path, r#"{"format":999,"study":"x","total":0,"created_unix":0,"updated_unix":0,"results":[]}"#).unwrap();
        assert!(Session::load(path).is_err());
    }
}
