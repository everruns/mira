//! [`Sample`]s and [`Dataset`]s — the inputs to an eval.
//!
//! Datasets are language-agnostic JSON so the same files can drive Rust, CLI, or
//! polyglot subjects. Small evals skip the file entirely and inline samples in
//! Rust via [`Eval::case`](crate::eval::EvalBuilder::case).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Metadata;

/// One dataset row: an input conversation plus optional target / metadata.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Stable identifier within the eval (used in case keys and selection).
    pub id: String,
    /// Sequence of user turns to send. Most samples have exactly one.
    pub input: Vec<String>,
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

    /// Attach a metadata key/value.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
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
}
