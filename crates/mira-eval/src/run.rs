//! Run identity, timing, and rolled-up summary for a single host invocation.
//!
//! A *run* is one `mira run`/`mira score` invocation. Its [`RunMeta`] (a unique,
//! sortable id, start/finish timestamps, and a result summary) is the record the
//! host's `--save` writes next to the report, so past runs can later be listed
//! and compared. This is the data foundation for the "historical trend
//! aggregation across runs" seam in `specs/architecture.md` §12 — the query
//! commands consume these records; they don't change this shape.
//!
//! Design note: a run id is per *invocation*, not per checkpoint. Resuming a
//! `--checkpoint` continues the same [`Session`](crate::session::Session) but is
//! a fresh run with its own id/timestamps — exactly what you want when comparing
//! the same suite over time.

use serde::{Deserialize, Serialize};

use crate::protocol::RunResult;
use crate::session::now_unix;

/// On-disk format version for the run meta file. Bumped on a breaking layout
/// change; readers treat an unrecognised version as unusable.
pub const RUN_META_FORMAT: u32 = 1;

/// Identity, timing, and summary for one host run — the `meta.json` written by
/// `--save` alongside the report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunMeta {
    /// Layout version; see [`RUN_META_FORMAT`].
    pub format: u32,
    /// Sortable, unique run id: `YYYYMMDDThhmmssZ-xxxx` (UTC second + suffix).
    pub run_id: String,
    /// Study name (from `initialize`).
    pub study: String,
    /// Study version, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study_version: Option<String>,
    /// Unix seconds when this run started.
    pub started_unix: u64,
    /// Unix seconds when this run finished.
    pub finished_unix: u64,
    /// Rolled-up result summary (the same shape the report JSON carries).
    pub summary: RunSummary,
}

/// Rolled-up counts and totals over a run's results. The single source of truth
/// for the report JSON `summary` block and the saved run `meta.json`.
///
/// A cell is one of three states (see [`report::is_na`](crate::report::is_na)):
/// *scored* (a real verdict — counts toward `passed`/`failed`), *N/A* (ran but
/// nothing could be evaluated — an unreachable judge or infra failure; excluded
/// from the verdict like a skip), or *skipped* (never executed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    /// Cells with a real verdict (ran, not N/A): `passed + failed`.
    pub scored: usize,
    pub passed: usize,
    pub failed: usize,
    /// Cells that ran but were all-N/A — neither passed nor failed.
    pub na: usize,
    /// Cells that never executed.
    pub skipped: usize,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub total_tool_calls: usize,
    pub total_duration_ms: u64,
}

impl RunSummary {
    /// Aggregate a slice of results. Usage/timing totals cover every cell that
    /// actually ran (including N/A ones, which may have burned tokens before
    /// failing); only never-run skips drop out.
    pub fn of(results: &[RunResult]) -> Self {
        let mut s = RunSummary::default();
        for r in results {
            if r.skipped {
                s.skipped += 1;
                continue;
            }
            if crate::report::is_na(r) {
                s.na += 1;
            } else {
                s.scored += 1;
                if r.passed {
                    s.passed += 1;
                }
            }
            s.total_tokens += r.transcript.usage.total_tokens();
            s.total_cost_usd += r.transcript.usage.cost_usd;
            s.total_tool_calls += r.transcript.tool_calls_count;
            s.total_duration_ms += r.transcript.timing.duration_ms;
        }
        s.failed = s.scored - s.passed;
        s
    }
}

/// A fresh, sortable, collision-resistant run id: `YYYYMMDDThhmmssZ-xxxx`. The
/// timestamp prefix sorts lexically by time; the 4-hex suffix disambiguates two
/// runs started within the same second.
pub fn new_run_id() -> String {
    format!(
        "{}-{}",
        format_compact_utc(now_unix()),
        Hex4(short_suffix())
    )
}

/// `YYYYMMDDThhmmssZ` for `secs` (Unix seconds, UTC). Dependency-free on
/// purpose: the core avoids a date crate, and this only needs whole-second UTC.
pub fn format_compact_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's proleptic
/// Gregorian algorithm; correct across leap years without a calendar crate.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A short, non-cryptographic suffix from sub-second time and the pid — enough
/// to keep ids distinct when several runs land in the same wall-clock second.
fn short_suffix() -> u16 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    ((nanos ^ pid.wrapping_mul(2_654_435_761)) & 0xffff) as u16
}

/// Renders a `u16` as fixed 4-hex so the run-id suffix is constant width.
struct Hex4(u16);
impl std::fmt::Display for Hex4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TranscriptSummary;
    use crate::{Score, Usage};

    #[test]
    fn compact_utc_known_epochs() {
        assert_eq!(format_compact_utc(0), "19700101T000000Z");
        assert_eq!(format_compact_utc(1_609_459_200), "20210101T000000Z");
        // 2023-11-14 22:13:20 UTC.
        assert_eq!(format_compact_utc(1_700_000_000), "20231114T221320Z");
    }

    #[test]
    fn run_id_is_sortable_and_shaped() {
        let id = new_run_id();
        // YYYYMMDDThhmmssZ-xxxx → 16 + 1 + 4.
        assert_eq!(id.len(), 21, "{id}");
        assert_eq!(&id[8..9], "T");
        assert_eq!(&id[15..17], "Z-");
        assert!(id[17..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn run_result(passed: bool, skipped: bool, tokens: u64) -> RunResult {
        let t = TranscriptSummary {
            usage: Usage {
                output_tokens: tokens,
                ..Default::default()
            },
            tool_calls_count: 1,
            timing: crate::Timing {
                duration_ms: 10,
                ..Default::default()
            },
            ..Default::default()
        };
        RunResult {
            eval: "e".into(),
            sample: "s".into(),
            model: "m".into(),
            params: Default::default(),
            passed,
            aggregate: if passed { 1.0 } else { 0.0 },
            scores: vec![Score::pass("x", "ok")],
            transcript: t,
            skipped,
        }
    }

    #[test]
    fn summary_counts_and_totals() {
        // A pass, a fail, an all-N/A cell (ran but unevaluated), and a skip.
        let na = {
            let mut r = run_result(false, false, 7);
            r.scores = vec![Score::na("judge", "unreachable")];
            r
        };
        let results = vec![
            run_result(true, false, 5),
            run_result(false, false, 3),
            na,
            run_result(false, true, 99), // skipped: ignored in totals
        ];
        let s = RunSummary::of(&results);
        assert_eq!(s.scored, 2);
        assert_eq!(s.passed, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.na, 1, "all-N/A cell is N/A, not failed");
        assert_eq!(s.skipped, 1);
        // Totals include the N/A cell (it ran), exclude only the skip.
        assert_eq!(s.total_tokens, 15);
        assert_eq!(s.total_tool_calls, 3);
        assert_eq!(s.total_duration_ms, 30);
    }
}
