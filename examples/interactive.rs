#!/usr/bin/env -S cargo +nightly -Zscript
---
# Single-file Mira study (cargo-script frontmatter, RFC 3502). Run it with
# the host CLI — no per-study crate:
#
#   mira run --script examples/interactive.rs
#
# The host shims cargo-script on **stable** (it's otherwise nightly-only
# `cargo -Zscript`); set MIRA_SCRIPT_NATIVE=1 to run it natively on nightly.
# Outside this repo, depend on the published crates: mira-eval = "0.3".
[package]
edition = "2024"

[dependencies]
mira-eval = { path = "../crates/mira-eval" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
---
//! An **interactive** (multi-turn) eval: the subject and a *simulated user*
//! exchange turns until the conversation resolves. The subject answers from the
//! running conversation (`cx.conversation`); the `.responder(..)` closure plays
//! the user. Runs offline against `sim`:
//!
//! ```bash
//! mira run --script examples/interactive.rs
//! ```
//!
//! Here the subject asks a clarifying question; the simulated user answers it;
//! the subject then gives the final answer. The whole dialog is folded into one
//! transcript that the scorers grade. Interactive evals need no protocol feature
//! — the turn exchange is driven in-process.

use mira::scorer::{contains, succeeded, turns_within};
use mira::subject::subject_fn;
use mira::{Eval, Message, Part, Role, eval};

#[eval]
fn interactive() -> Eval {
    Eval::new("interactive")
        .describe("Asks a clarifying question, then answers once the user replies")
        .sample("weather", "What's the weather?")
        .max_turns(4)
        .subject(subject_fn(|_sample, cx| async move {
            // The subject sees the conversation so far and replies to the latest
            // user turn. Opening turn only? Ask for the missing detail.
            let said_city = cx
                .conversation
                .iter()
                .filter(|m| m.role == Role::User)
                .nth(1) // the user's *reply* to our clarifying question
                .map(Message::text);
            match said_city {
                Some(city) => mira::Transcript::response(format!("It's sunny in {city}.")),
                None => mira::Transcript::response("Which city?"),
            }
        }))
        // Simulated user: answer the clarifying question once, then stop.
        .responder(|convo: &[Message]| {
            let last = convo.last()?;
            if last.role == Role::Assistant && last.text().contains("Which city") {
                Some(vec![Part::text("Paris")])
            } else {
                None // the subject answered — end the conversation
            }
        })
        .scorer(succeeded())
        .scorer(contains("sunny"))
        .scorer(turns_within(4))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
