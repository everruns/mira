//! Evaluate an **external program** — the polyglot path. Here the "agent under
//! test" is a small shell pipeline, but it could be any binary in any language,
//! including an everruns coding CLI that emits the canonical JSONL transcript.
//!
//! ```bash
//! mira run --bin cli_subject
//! ```

use mira::scorer::{contains, succeeded};
use mira::subject::CliSubject;
use mira::{Eval, eval};

#[eval]
fn shell() -> Eval {
    Eval::new("shell")
        .describe("Runs an external CLI and scores its stdout")
        // The subject is a real external program, `subject.sh`, sitting next to
        // this example. Mira sends it the prompt as argv[1] and captures stdout.
        .subject(
            CliSubject::new(concat!(env!("CARGO_MANIFEST_DIR"), "/subject.sh")).arg("{prompt}"),
        )
        .sample("greet", "hello world")
        .scorer(succeeded())
        .scorer(contains("HELLO WORLD"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
