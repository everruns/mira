//! [`ModelSpec`] — one cell of the model matrix.
//!
//! Mira's core is deliberately **provider-agnostic**: a [`ModelSpec`] is just a
//! `(label, provider, model)` descriptor plus an availability flag and
//! metadata. It carries no API keys and no provider SDK types. A
//! [`Subject`](crate::subject::Subject) interprets the spec — e.g. the
//! `mira-everruns` `RuntimeSubject` maps `provider`/`model` onto an everruns
//! `ResolvedModel`, while a [`CliSubject`](crate::subject::CliSubject) passes
//! the label to a subprocess.
//!
//! Availability is decided in the *study* process (where keys live): a named
//! provider is available only when its API-key env var is set. Unavailable
//! cells are **skipped**, never failed, so a key-free run stays green offline.

use std::collections::BTreeMap;

use crate::Metadata;

/// The conventional offline simulator provider id.
pub const SIM_PROVIDER: &str = "sim";

/// One cell of the model matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelSpec {
    /// Stable label used in case keys and selection (e.g. `anthropic/opus`).
    pub label: String,
    /// Provider id (e.g. `sim`, `anthropic`, `openai`). Subjects route on this.
    pub provider: String,
    /// Model id passed to the provider (e.g. `claude-opus-4-8`).
    pub model: String,
    /// Whether this cell can run. `false` (e.g. missing API key) ⇒ skipped.
    pub available: bool,
    /// Free-form metadata: cost tier, region, observability links, etc.
    pub metadata: Metadata,
}

impl ModelSpec {
    /// A fully-explicit, always-available cell. Use the provider-specific
    /// constructors below for key-gated cloud models.
    pub fn new(
        label: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            provider: provider.into(),
            model: model.into(),
            available: true,
            metadata: BTreeMap::new(),
        }
    }

    /// The offline simulator — runs end-to-end with no API key. The default
    /// matrix cell, so a fresh `mira run` is green without credentials.
    pub fn sim() -> Self {
        Self::new("sim", SIM_PROVIDER, "sim")
    }

    /// An Anthropic model. Available only when `ANTHROPIC_API_KEY` is set.
    pub fn anthropic(model: impl Into<String>) -> Self {
        Self::cloud("anthropic", model, "ANTHROPIC_API_KEY")
    }

    /// An OpenAI model. Available only when `OPENAI_API_KEY` is set.
    pub fn openai(model: impl Into<String>) -> Self {
        Self::cloud("openai", model, "OPENAI_API_KEY")
    }

    /// A Google Gemini model. Available only when `GEMINI_API_KEY` is set.
    pub fn gemini(model: impl Into<String>) -> Self {
        Self::cloud("gemini", model, "GEMINI_API_KEY")
    }

    /// A cloud cell labelled `provider/model`, available iff `key_env` is set in
    /// the study's environment.
    pub fn cloud(
        provider: impl Into<String>,
        model: impl Into<String>,
        key_env: impl AsRef<str>,
    ) -> Self {
        let provider = provider.into();
        let model = model.into();
        let available = std::env::var(key_env.as_ref())
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        Self {
            label: format!("{provider}/{model}"),
            provider,
            model,
            available,
            metadata: BTreeMap::new(),
        }
    }

    /// Override the label (e.g. a short alias for a long model id).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Force availability (e.g. a custom provider with no env gating).
    pub fn available(mut self, available: bool) -> Self {
        self.available = available;
        self
    }

    /// Attach a metadata key/value.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// True if this is the offline simulator cell.
    pub fn is_sim(&self) -> bool {
        self.provider == SIM_PROVIDER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_is_available() {
        let m = ModelSpec::sim();
        assert!(m.available);
        assert!(m.is_sim());
        assert_eq!(m.label, "sim");
    }

    #[test]
    fn cloud_gates_on_env() {
        // A key env that is (almost certainly) unset ⇒ unavailable.
        let m = ModelSpec::cloud("acme", "x", "MIRA_TEST_DEFINITELY_UNSET_KEY");
        assert!(!m.available);
        assert_eq!(m.label, "acme/x");
    }

    #[test]
    fn builder_overrides() {
        let m = ModelSpec::new("a", "p", "m")
            .label("alias")
            .available(false)
            .meta("region", "us");
        assert_eq!(m.label, "alias");
        assert!(!m.available);
        assert_eq!(m.metadata.get("region").unwrap(), "us");
    }
}
