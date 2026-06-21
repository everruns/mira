//! A multimodal eval study: the sample carries an **image attachment** next to
//! its text prompt, and the subject reads the full multimodal prompt via
//! [`Sample::prompt_parts`]. Runs offline against `sim` (no API key):
//!
//! ```bash
//! mira --bin multimodal list
//! mira --bin multimodal run
//! ```
//!
//! Multimodal *input* is study-side and needs no protocol feature. Multimodal
//! *output* (`Transcript::output`) is staged behind `protocol-unstable` — see
//! `specs/architecture.md`.

use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Part, Sample, Transcript, eval};

/// A 1x1 transparent PNG as a `data:` URI — a stand-in for a real image so the
/// example stays self-contained and offline.
const PIXEL_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M8AAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

#[eval]
fn multimodal() -> Eval {
    Eval::new("multimodal")
        .describe("Answers a question about an attached image")
        .sample(
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
            Transcript::response(format!(
                "Received {} parts ({kinds}); the image is {media}.",
                parts.len()
            ))
        }))
        .scorer(succeeded())
        .scorer(contains("image/png"))
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
