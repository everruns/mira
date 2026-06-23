//! Demonstrates Mira's distinction between a **failure** (the model under test
//! got it wrong) and an **infrastructure error** (budget/quota, rate limit,
//! provider outage, timeout — not the model's fault).
//!
//! ```bash
//! mira --bin infra run
//! ```
//!
//! Three samples show the three outcomes:
//! * `good`  — the subject answers correctly → **PASS**.
//! * `wrong` — the subject answers incorrectly → **FAIL** (a real, scored miss).
//! * `broke` — the subject hits an outage via [`Transcript::infra_error`] →
//!   **N/A**: scoring short-circuits to a single N/A score (the case-level dual
//!   of a scorer returning [`mira::Score::na`]), so it's excluded from the
//!   pass-rate — neither passed nor failed, like a skip. The host re-queues such
//!   cases up to `--max-retries`; one that stays broken is reported N/A, never
//!   counted against the model.
//!
//! Everything runs offline against the `sim` model — no API key needed.

use mira::scorer::contains;
use mira::subject::subject_fn;
use mira::{Eval, Transcript, eval};

#[eval]
fn infra() -> Eval {
    Eval::new("infra")
        .describe("Failure vs. infrastructure error: scored miss vs. N/A outage")
        .sample("good", "answer 42")
        .sample("wrong", "answer 42")
        .sample("broke", "answer 42")
        .subject(subject_fn(|sample, _cx| async move {
            match sample.id.as_str() {
                // A correct answer: passes the `contains("42")` scorer.
                "good" => Transcript::response("the answer is 42"),
                // A wrong answer: a genuine, scoreable model failure.
                "wrong" => Transcript::response("the answer is 7"),
                // Infrastructure broke mid-run. Not the model's fault: scoring
                // short-circuits to N/A, so the case is excluded from pass/fail
                // and the host retries it.
                _ => Transcript::infra_error("provider 503: service temporarily unavailable"),
            }
        }))
        .scorer(contains("42"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
