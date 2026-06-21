//! Multimodal content: a typed vocabulary for non-text inputs and outputs.
//!
//! Mira's text fields — [`Sample::input`](crate::Sample::input) and
//! [`Transcript::final_response`](crate::Transcript::final_response) — stay the
//! canonical path for the common, text-only case. A [`Part`] generalizes a turn
//! or a response into one typed piece of content — text, an image, audio, a
//! file, or a structured JSON value — so multimodal subjects fit a first-class
//! shape rather than smuggling media through `files`/`metadata`/`events`.
//!
//! Design decisions (kept here, on the type):
//! * **Media is referenced, not embedded as bytes.** A media [`Part`] carries a
//!   `media_type` (an IANA type like `image/png`) plus *either* a `uri` (an
//!   `http(s)://` URL or a `data:` URI) *or* inline base64 `data`. Never raw
//!   bytes — so a `Part` is plain JSON that serializes on the wire and into
//!   JSONL datasets unchanged, and the core stays dependency-free (no image/
//!   audio codecs).
//! * **Open by construction, closed in shape.** The variant set is small and
//!   typed (scorers can match on it), while `Json` is the escape hatch for any
//!   structured output the typed variants don't cover.

use serde::{Deserialize, Serialize};

/// One piece of multimodal content. The discriminant serializes as a `kind`
/// tag (`text` / `image` / `audio` / `file` / `json`), so a part is a single
/// self-describing JSON object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    /// Plain text — the same content `final_response` / `input` carry, but as a
    /// part so it can sit alongside media in one ordered list.
    Text { text: String },
    /// An image, referenced by `uri` or inline base64 `data`.
    Image {
        media_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    /// Audio, referenced by `uri` or inline base64 `data`.
    Audio {
        media_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    /// An arbitrary file (a document, an archive, …), referenced by `uri` or
    /// inline base64 `data`, with an optional display `name`.
    File {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
        media_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    /// A structured JSON value — for tool-style / structured outputs that aren't
    /// free text and aren't a media blob.
    Json { json: serde_json::Value },
}

impl Part {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        Part::Text { text: text.into() }
    }

    /// An image referenced by a `uri` (a URL or a `data:` URI).
    pub fn image_uri(media_type: impl Into<String>, uri: impl Into<String>) -> Self {
        Part::Image {
            media_type: media_type.into(),
            uri: Some(uri.into()),
            data: None,
        }
    }

    /// An image carried inline as base64.
    pub fn image_data(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Part::Image {
            media_type: media_type.into(),
            uri: None,
            data: Some(data.into()),
        }
    }

    /// Audio referenced by a `uri`.
    pub fn audio_uri(media_type: impl Into<String>, uri: impl Into<String>) -> Self {
        Part::Audio {
            media_type: media_type.into(),
            uri: Some(uri.into()),
            data: None,
        }
    }

    /// Audio carried inline as base64.
    pub fn audio_data(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Part::Audio {
            media_type: media_type.into(),
            uri: None,
            data: Some(data.into()),
        }
    }

    /// A file referenced by a `uri`, with a display `name`.
    pub fn file_uri(
        name: impl Into<String>,
        media_type: impl Into<String>,
        uri: impl Into<String>,
    ) -> Self {
        Part::File {
            name: name.into(),
            media_type: media_type.into(),
            uri: Some(uri.into()),
            data: None,
        }
    }

    /// A structured JSON part.
    pub fn json(value: impl Into<serde_json::Value>) -> Self {
        Part::Json { json: value.into() }
    }

    /// The discriminant as a stable string: `text` / `image` / `audio` /
    /// `file` / `json`. Matches the serialized `kind` tag, so scorers and
    /// reports can name a modality without re-deriving it.
    pub fn kind(&self) -> &'static str {
        match self {
            Part::Text { .. } => "text",
            Part::Image { .. } => "image",
            Part::Audio { .. } => "audio",
            Part::File { .. } => "file",
            Part::Json { .. } => "json",
        }
    }

    /// True for a text part.
    pub fn is_text(&self) -> bool {
        matches!(self, Part::Text { .. })
    }

    /// The text of a [`Part::Text`], else `None`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Part::Text { text } => Some(text),
            _ => None,
        }
    }

    /// The IANA `media_type` of a media part (image/audio/file), else `None`.
    pub fn media_type(&self) -> Option<&str> {
        match self {
            Part::Image { media_type, .. }
            | Part::Audio { media_type, .. }
            | Part::File { media_type, .. } => Some(media_type),
            _ => None,
        }
    }
}

/// The concatenated text of all [`Part::Text`] parts, joined by newlines.
/// Non-text parts are skipped — the text projection a text-only scorer sees.
pub fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(Part::as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The distinct modalities present, in first-seen order (e.g.
/// `["text", "image"]`). Drives modality-aware scorers and reporting.
pub fn modalities(parts: &[Part]) -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for p in parts {
        let k = p.kind();
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_modalities() {
        let parts = vec![
            Part::text("describe this"),
            Part::image_uri("image/png", "https://x/y.png"),
            Part::text("and this"),
            Part::image_data("image/jpeg", "QUJD"),
        ];
        assert_eq!(text_of(&parts), "describe this\nand this");
        // Distinct, first-seen order — `image` not repeated.
        assert_eq!(modalities(&parts), vec!["text", "image"]);
    }

    #[test]
    fn accessors() {
        let img = Part::image_uri("image/png", "u");
        assert_eq!(img.kind(), "image");
        assert!(!img.is_text());
        assert_eq!(img.as_text(), None);
        assert_eq!(img.media_type(), Some("image/png"));

        let txt = Part::text("hi");
        assert_eq!(txt.kind(), "text");
        assert!(txt.is_text());
        assert_eq!(txt.as_text(), Some("hi"));
        assert_eq!(txt.media_type(), None);
    }

    #[test]
    fn serializes_with_kind_tag_and_round_trips() {
        let parts = vec![
            Part::text("hello"),
            Part::image_data("image/png", "QkFTRTY0"),
            Part::file_uri("notes.pdf", "application/pdf", "https://x/n.pdf"),
            Part::json(serde_json::json!({"label": "cat", "p": 0.9})),
        ];
        let line = serde_json::to_string(&parts).unwrap();
        // The discriminant rides as `kind`; empty optionals are omitted.
        assert!(line.contains(r#""kind":"text""#));
        assert!(line.contains(r#""kind":"image""#));
        assert!(!line.contains(r#""uri":null"#));
        let back: Vec<Part> = serde_json::from_str(&line).unwrap();
        assert_eq!(back, parts);
    }
}
