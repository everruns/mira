//! [`Sample`]s and [`Dataset`]s — the inputs to an eval.
//!
//! Datasets are language-agnostic JSON so the same files can drive Rust, CLI, or
//! polyglot subjects. Small evals skip the file entirely and inline samples in
//! Rust via [`Eval::case`](crate::eval::EvalBuilder::case).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Metadata;
use crate::content::{self, Part};

/// One dataset row: an input conversation plus optional target / metadata.
///
/// Multimodal note: `input` carries the *text* turns (the common case);
/// `attachments` carries any non-text input (images, audio, files, structured
/// JSON) accompanying the prompt. [`Sample::prompt_parts`] fuses both into one
/// ordered [`Part`] list for a multimodal subject. `Sample` is **not** a wire
/// type — the study owns the dataset and the host addresses samples by id — so
/// this stays a study-side concern with no protocol impact.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Stable identifier within the eval (used in case keys and selection).
    pub id: String,
    /// Sequence of user turns to send. Most samples have exactly one.
    pub input: Vec<String>,
    /// Non-text input accompanying the prompt: images, audio, files, or
    /// structured JSON. Empty for text-only samples.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Part>,
    /// Optional reference answer / expected value for target-based scorers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    /// Files to pre-seed into the subject's workspace before the run.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
    /// Free-form tags for selective evaluation (`--tag smoke`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Free-form metadata (provenance, difficulty, observability links).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: Metadata,
}

impl Sample {
    /// A single-turn sample.
    pub fn new(id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            input: vec![prompt.into()],
            ..Default::default()
        }
    }

    /// A multi-turn sample.
    pub fn turns(
        id: impl Into<String>,
        turns: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            input: turns.into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Add a selection tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Attach a non-text input [`Part`] (image, audio, file, JSON) to the prompt.
    pub fn attach(mut self, part: Part) -> Self {
        self.attachments.push(part);
        self
    }

    /// Attach an image input, referenced by a `uri` (a URL or `data:` URI).
    pub fn image(self, media_type: impl Into<String>, uri: impl Into<String>) -> Self {
        self.attach(Part::image_uri(media_type, uri))
    }

    /// The full multimodal prompt as one ordered [`Part`] list: the text turns
    /// (each an [`Part::Text`]) followed by the `attachments`. The single entry
    /// point a multimodal subject reads, regardless of how the sample was built.
    pub fn prompt_parts(&self) -> Vec<Part> {
        let mut parts: Vec<Part> = self.input.iter().map(|s| Part::text(s.as_str())).collect();
        parts.extend(self.attachments.iter().cloned());
        parts
    }

    /// The distinct input modalities present (`text`, `image`, …), in first-seen
    /// order — `text` first when there are text turns, then attachment kinds.
    pub fn modalities(&self) -> Vec<&'static str> {
        content::modalities(&self.prompt_parts())
    }

    /// Set the reference target.
    pub fn target(mut self, target: impl Into<serde_json::Value>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Seed a workspace file.
    pub fn file(mut self, path: impl Into<String>, contents: impl Into<String>) -> Self {
        self.files.insert(path.into(), contents.into());
        self
    }

    /// Attach a metadata key/value. The value is open-ended JSON.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// The reference target as a string, if present and string-typed.
    pub fn target_str(&self) -> Option<&str> {
        self.target.as_ref().and_then(|v| v.as_str())
    }
}

/// A dataset is a sequence of [`Sample`]s. Loaders are convenience constructors;
/// the engine only cares about the resulting `Vec<Sample>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dataset {
    pub samples: Vec<Sample>,
}

impl Dataset {
    pub fn new(samples: Vec<Sample>) -> Self {
        Self { samples }
    }

    /// Number of samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Load a JSONL dataset: one [`Sample`] object per line. Blank lines are
    /// skipped. This is the secondary, config-style on-ramp; code-first evals
    /// usually inline cases.
    pub fn jsonl(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_jsonl_str(&text)
    }

    /// Parse a JSONL string (one [`Sample`] per line).
    pub fn from_jsonl_str(text: &str) -> std::io::Result<Self> {
        let mut samples = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let sample: Sample = serde_json::from_str(line).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line {}: {e}", i + 1),
                )
            })?;
            samples.push(sample);
        }
        Ok(Self { samples })
    }

    /// Load a JSON dataset: a top-level array of [`Sample`] objects.
    pub fn json(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let samples: Vec<Sample> = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Self { samples })
    }
}

impl From<Vec<Sample>> for Dataset {
    fn from(samples: Vec<Sample>) -> Self {
        Self { samples }
    }
}

impl FromIterator<Sample> for Dataset {
    fn from_iter<T: IntoIterator<Item = Sample>>(iter: T) -> Self {
        Self {
            samples: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_builder() {
        let s = Sample::new("a", "hi")
            .tag("smoke")
            .target("42")
            .file("main.rs", "fn main() {}")
            .meta("trace", "https://obs/123");
        assert_eq!(s.input, vec!["hi"]);
        assert_eq!(s.tags, vec!["smoke"]);
        assert_eq!(s.target_str(), Some("42"));
        assert_eq!(s.files.get("main.rs").unwrap(), "fn main() {}");
        assert_eq!(s.metadata.get("trace").unwrap(), "https://obs/123");
    }

    #[test]
    fn jsonl_roundtrip_and_blank_lines() {
        let text = r#"
{"id":"a","input":["hello"]}

{"id":"b","input":["world"],"tags":["smoke"]}
"#;
        let ds = Dataset::from_jsonl_str(text).unwrap();
        assert_eq!(ds.len(), 2);
        assert_eq!(ds.samples[1].tags, vec!["smoke"]);
    }

    #[test]
    fn jsonl_reports_bad_line() {
        let err = Dataset::from_jsonl_str("{not json}").unwrap_err();
        assert!(err.to_string().contains("line 1"));
    }

    #[test]
    fn multi_turn() {
        let s = Sample::turns("c", ["a", "b", "c"]);
        assert_eq!(s.input.len(), 3);
    }

    #[test]
    fn multimodal_input_fuses_text_and_attachments() {
        let s = Sample::new("a", "what is in this image?")
            .image("image/png", "https://x/cat.png")
            .attach(Part::audio_uri("audio/wav", "https://x/clip.wav"));
        let parts = s.prompt_parts();
        assert_eq!(parts.len(), 3); // 1 text turn + 2 attachments
        assert_eq!(parts[0].as_text(), Some("what is in this image?"));
        assert_eq!(parts[1].kind(), "image");
        assert_eq!(s.modalities(), vec!["text", "image", "audio"]);
    }

    #[test]
    fn text_only_sample_has_no_attachments_on_the_wire() {
        // A plain sample omits `attachments` when serialized (back-compat with
        // existing JSONL datasets).
        let s = Sample::new("a", "hi");
        assert!(s.attachments.is_empty());
        let line = serde_json::to_string(&s).unwrap();
        assert!(!line.contains("attachments"));
        // …and a multimodal sample round-trips through JSONL.
        let m = Sample::new("b", "describe").image("image/png", "u");
        let back: Sample = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }
}
