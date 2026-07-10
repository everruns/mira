#!/usr/bin/env -S cargo +nightly -Zscript
---
# Single-file Mira study (cargo-script frontmatter, RFC 3502). Run it with
# the host CLI — no per-study crate:
#
#   mira run --script examples/multimodal.rs
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
//! A multimodal eval study: the sample carries an **image attachment** next to
//! its text prompt, and the subject reads the full multimodal prompt via
//! [`Sample::prompt_parts`]. Runs offline against `sim` (no API key):
//!
//! ```bash
//! mira list --script examples/multimodal.rs
//! mira run --script examples/multimodal.rs
//! ```
//!
//! Multimodal *input* is study-side and needs no protocol feature. The subject
//! also returns multimodal *output* (`Transcript::output`) — typed [`Part`]s on
//! the committed protocol `1.0` wire — graded by `produced_modality`.

use mira::scorer::{contains, produced_modality, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Part, Sample, Transcript, eval};

/// A 1x1 transparent PNG as a `data:` URI — a stand-in for a real image so the
/// example stays self-contained and offline.
const PIXEL_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M8AAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

#[eval]
fn multimodal() -> Eval {
    Eval::new("multimodal")
        .describe("Answers a question about an attached image")
        .add_sample(
            Sample::new("describe-image", "What format is the attached image?")
                .attach(Part::image_uri("image/png", PIXEL_PNG))
                .tag("smoke"),
        )
        .subject(subject_fn(|sample, _cx| async move {
            // A real subject would send `sample.prompt_parts()` (text + media) to
            // a multimodal model. This stand-in inspects the attached modalities.
            let parts = sample.prompt_parts();
            let kinds = mira::content::modalities(&parts).join(", ");
            let media = parts.iter().find_map(Part::media_type).unwrap_or("none");
            let text = format!(
                "Received {} parts ({kinds}); the image is {media}.",
                parts.len()
            );
            // Return a multimodal response: the canonical text plus a thumbnail
            // the model "produced". `final_response` stays the text projection.
            Transcript::response(text.clone())
                .with_output([Part::text(text), Part::image_uri("image/png", PIXEL_PNG)])
        }))
        .scorer(succeeded())
        .scorer(contains("image/png"))
        .scorer(produced_modality("image"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
