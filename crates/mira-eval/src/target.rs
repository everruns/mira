//! [`Target`] — one case of the **target** matrix (the privileged comparison
//! axis): the configured thing under evaluation. For an LLM eval a target *is* a
//! model; for an agent eval it is a harness (e.g. `yolop`, `codex`), optionally
//! wrapping a model.
//!
//! Mira's core is deliberately **provider-agnostic**: a [`Target`] is just a
//! `(label, provider, model)` descriptor plus an availability flag and
//! metadata. It carries no API keys and no provider SDK types. A
//! [`Subject`](crate::subject::Subject) interprets it — e.g. the
//! `mira-everruns` `RuntimeSubject` maps `provider`/`model` onto an everruns
//! `ResolvedModel`, while a [`CliSubject`](crate::subject::CliSubject) routes on
//! the label (so a single CLI subject dispatches `yolop` vs `codex`).
//!
//! Availability is decided in the *study* process (where keys live): a named
//! provider is available only when its API-key env var is set. Unavailable
//! cases are **skipped**, never failed, so a key-free run stays green offline.

use std::collections::BTreeMap;

use crate::Metadata;

/// The conventional offline simulator provider id.
pub const SIM_PROVIDER: &str = "sim";

/// The conventional provider id for a [`Target::cli`] harness target.
pub const CLI_PROVIDER: &str = "cli";

/// One case of the target matrix — the configured thing under evaluation (a
/// model, or a harness optionally wrapping one).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Stable label used in case keys and selection (e.g. `anthropic/opus`,
    /// `yolop`). This is the name `--targets`/`--axis target=…` matches.
    pub label: String,
    /// Provider id (e.g. `sim`, `anthropic`, `openai`, `cli`). Subjects route on
    /// this; the host buckets per-provider concurrency on it.
    pub provider: String,
    /// Underlying model id passed to the provider (e.g. `claude-opus-4-8`).
    /// Empty for a pure harness target whose model is irrelevant or implicit.
    pub model: String,
    /// Whether this case can run. `false` (e.g. missing API key) ⇒ skipped.
    pub available: bool,
    /// Free-form metadata: cost tier, region, observability links, etc.
    pub metadata: Metadata,
}

impl Target {
    /// A fully-explicit, always-available case. Use the provider-specific
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
    /// matrix case, so a fresh `mira run` is green without credentials.
    pub fn sim() -> Self {
        Self::new("sim", SIM_PROVIDER, "sim")
    }

    /// A **harness / agent** target identified by `label` (e.g. `yolop`,
    /// `codex`) — not a provider model. Always available; provider is
    /// [`CLI_PROVIDER`], model empty. A [`CliSubject`](crate::subject::CliSubject)
    /// (or any subject) dispatches on `cx.target.label`. Wrap an underlying model
    /// for cost attribution via [`with_model`](Target::with_model) when it matters.
    pub fn cli(label: impl Into<String>) -> Self {
        Self::new(label, CLI_PROVIDER, "")
    }

    /// Set the underlying model id (e.g. attach the model a harness target wraps).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
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

    /// A cloud case labelled `provider/model`, available iff `key_env` is set in
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

    /// Attach a metadata key/value. The value is open-ended JSON.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// True if this is the offline simulator case.
    pub fn is_sim(&self) -> bool {
        self.provider == SIM_PROVIDER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_is_available() {
        let m = Target::sim();
        assert!(m.available);
        assert!(m.is_sim());
        assert_eq!(m.label, "sim");
    }

    #[test]
    fn cli_is_a_harness_target() {
        // A harness/agent target (yolop, codex): always available, CLI provider,
        // empty underlying model unless one is attached for cost attribution.
        let t = Target::cli("yolop");
        assert!(t.available);
        assert!(!t.is_sim());
        assert_eq!(t.label, "yolop");
        assert_eq!(t.provider, CLI_PROVIDER);
        assert_eq!(t.model, "");
        let wrapped = Target::cli("codex").with_model("claude-opus-4-8");
        assert_eq!(wrapped.model, "claude-opus-4-8");
    }

    #[test]
    fn cloud_gates_on_env() {
        // A key env that is (almost certainly) unset ⇒ unavailable.
        let m = Target::cloud("acme", "x", "MIRA_TEST_DEFINITELY_UNSET_KEY");
        assert!(!m.available);
        assert_eq!(m.label, "acme/x");
    }

    #[test]
    fn builder_overrides() {
        let m = Target::new("a", "p", "m")
            .label("alias")
            .available(false)
            .meta("region", "us");
        assert_eq!(m.label, "alias");
        assert!(!m.available);
        assert_eq!(m.metadata.get("region").unwrap(), "us");
    }
}
