//! Evaluate an **external program** — the polyglot path. Here the "agent under
//! test" is a small shell pipeline, but it could be any binary in any language,
//! including an everruns coding CLI that emits the canonical JSONL transcript.
//!
//! ```bash
//! mira --package mira-examples --example cli_subject run
//! ```

use mira::scorer::{contains, succeeded};
use mira::subject::CliSubject;
use mira::{Eval, eval};

#[eval]
fn shell() -> Eval {
    Eval::new("shell")
        .describe("Runs an external CLI and scores its stdout")
        // The subject runs `sh -c 'echo <prompt> | tr a-z A-Z'`, i.e. it
        // upper-cases whatever prompt we send it.
        .subject(
            CliSubject::new("sh")
                .arg("-c")
                .arg("printf '%s' \"{prompt}\" | tr 'a-z' 'A-Z'"),
        )
        .case("greet", "hello world")
        .scorer(succeeded())
        .scorer(contains("HELLO WORLD"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::serve_registered().await
}
