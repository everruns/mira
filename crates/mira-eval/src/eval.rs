//! [`Eval`]: one evaluation = a dataset + a subject + scorers + a model matrix.
//!
//! This mirrors Inspect AI's `Task`, but the matrix (which models to run) is a
//! first-class axis. The code-first builder is the primary authoring surface;
//! `Dataset::jsonl` / `Dataset::json` are the secondary, config-style on-ramps.

use std::sync::Arc;

use crate::model::ModelSpec;
use crate::scorer::Scorer;
use crate::subject::Subject;
use crate::{Dataset, Metadata, Sample};

/// A dataset row, in eval-authoring terms.
pub type Case = Sample;

/// One extra matrix axis beyond the model: a name and the discrete values it
/// takes (e.g. `("effort", ["low", "high"])`). The runner takes the
/// cross-product of all axes with the model matrix and the dataset, and the
/// chosen value for each axis is handed to the subject via [`RunCx::param`].
///
/// [`RunCx::param`]: crate::RunCx::param
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub name: String,
    pub values: Vec<String>,
}

impl Axis {
    pub fn new(
        name: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            values: values.into_iter().map(Into::into).collect(),
        }
    }
}

/// A single evaluation, ready to run across its model matrix.
pub struct Eval {
    pub name: String,
    pub description: String,
    pub dataset: Dataset,
    pub subject: Arc<dyn Subject>,
    pub scorers: Vec<Box<dyn Scorer>>,
    pub models: Vec<ModelSpec>,
    /// Extra matrix axes beyond the model (empty for a model-only matrix).
    pub axes: Vec<Axis>,
    pub max_turns: usize,
    pub metadata: Metadata,
}

impl Eval {
    /// Every combination of axis values, as `params` maps, in cross-product
    /// order. Always yields at least one (empty) map, so a no-axis eval runs a
    /// single cell per `(sample, model)`.
    pub fn axis_combinations(&self) -> Vec<Metadata> {
        let mut combos = vec![Metadata::new()];
        for axis in &self.axes {
            let mut next = Vec::new();
            for combo in &combos {
                for value in &axis.values {
                    let mut c = combo.clone();
                    c.insert(axis.name.clone(), value.clone());
                    next.push(c);
                }
            }
            if !next.is_empty() {
                combos = next;
            }
        }
        combos
    }
}

impl Eval {
    /// Starts the builder. Returns an [`EvalBuilder`] so the fluent API can
    /// require a subject before producing an [`Eval`].
    #[allow(clippy::new_ret_no_self)]
    pub fn new(name: impl Into<String>) -> EvalBuilder {
        EvalBuilder {
            name: name.into(),
            description: String::new(),
            dataset: Dataset::default(),
            subject: None,
            scorers: Vec::new(),
            models: Vec::new(),
            axes: Vec::new(),
            max_turns: 12,
            metadata: Metadata::new(),
        }
    }
}

pub struct EvalBuilder {
    name: String,
    description: String,
    dataset: Dataset,
    subject: Option<Arc<dyn Subject>>,
    scorers: Vec<Box<dyn Scorer>>,
    models: Vec<ModelSpec>,
    axes: Vec<Axis>,
    max_turns: usize,
    metadata: Metadata,
}

impl EvalBuilder {
    /// A one-line human description (shown in `list`).
    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Provide the dataset wholesale (e.g. `Dataset::jsonl(...)`).
    pub fn dataset(mut self, dataset: impl Into<Dataset>) -> Self {
        self.dataset = dataset.into();
        self
    }

    /// Inline one single-turn case — no dataset file needed for small evals.
    pub fn case(mut self, id: impl Into<String>, prompt: impl Into<String>) -> Self {
        self.dataset.samples.push(Sample::new(id, prompt));
        self
    }

    /// Inline a fully-built [`Sample`] (with tags, target, seeded files).
    pub fn sample(mut self, sample: Sample) -> Self {
        self.dataset.samples.push(sample);
        self
    }

    /// The thing under evaluation.
    pub fn subject(mut self, subject: impl Subject + 'static) -> Self {
        self.subject = Some(Arc::new(subject));
        self
    }

    /// The thing under evaluation, already in an `Arc` (shared across evals).
    pub fn subject_arc(mut self, subject: Arc<dyn Subject>) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Add a scorer. Every scorer runs against every sample × model cell.
    pub fn scorer(mut self, scorer: Box<dyn Scorer>) -> Self {
        self.scorers.push(scorer);
        self
    }

    /// Add one matrix cell (a model). Omit entirely to default to `sim`.
    pub fn model(mut self, model: ModelSpec) -> Self {
        self.models.push(model);
        self
    }

    /// Replace the matrix with `models`.
    pub fn models(mut self, models: impl IntoIterator<Item = ModelSpec>) -> Self {
        self.models = models.into_iter().collect();
        self
    }

    /// Add an extra matrix axis (beyond the model): a name and the discrete
    /// values it takes. The runner crosses every axis with the model matrix, and
    /// the subject reads the chosen value via [`RunCx::param`](crate::RunCx::param).
    ///
    /// ```
    /// # use mira::{Eval, Transcript, subject::subject_fn, scorer::succeeded};
    /// let eval = Eval::new("e")
    ///     .case("a", "x")
    ///     .axis("effort", ["low", "high"])
    ///     .subject(subject_fn(|_, cx| async move {
    ///         Transcript::response(cx.param("effort").unwrap_or("?").to_string())
    ///     }))
    ///     .scorer(succeeded())
    ///     .build();
    /// // One sample × one (default sim) model × two effort values = 2 cells.
    /// assert_eq!(eval.axis_combinations().len(), 2);
    /// ```
    pub fn axis(
        mut self,
        name: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.axes.push(Axis::new(name, values));
        self
    }

    /// Cap reasoning iterations per run.
    pub fn max_turns(mut self, max_turns: usize) -> Self {
        self.max_turns = max_turns;
        self
    }

    /// Attach a metadata key/value (provenance, suite, observability links).
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Finish the eval. Panics if no subject was provided.
    pub fn build(self) -> Eval {
        Eval {
            name: self.name,
            description: self.description,
            dataset: self.dataset,
            subject: self.subject.expect("eval requires a subject"),
            scorers: self.scorers,
            models: if self.models.is_empty() {
                vec![ModelSpec::sim()]
            } else {
                self.models
            },
            axes: self.axes,
            max_turns: self.max_turns,
            metadata: self.metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subject::subject_fn;
    use crate::{Transcript, scorer::contains};

    #[test]
    fn builder_defaults_to_sim() {
        let eval = Eval::new("greet")
            .case("hi", "say hi")
            .subject(subject_fn(|_, _| async { Transcript::response("hi") }))
            .scorer(contains("hi"))
            .build();
        assert_eq!(eval.models.len(), 1);
        assert!(eval.models[0].is_sim());
        assert_eq!(eval.dataset.len(), 1);
    }

    #[test]
    fn builder_keeps_metadata_and_matrix() {
        let eval = Eval::new("e")
            .describe("desc")
            .meta("suite", "smoke")
            .models([ModelSpec::sim(), ModelSpec::anthropic("opus")])
            .subject(subject_fn(|_, _| async { Transcript::default() }))
            .build();
        assert_eq!(eval.description, "desc");
        assert_eq!(eval.metadata.get("suite").unwrap(), "smoke");
        assert_eq!(eval.models.len(), 2);
    }
}
