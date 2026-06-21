//! [`Eval`]: one evaluation = a dataset + a subject + scorers + a model matrix.
//!
//! This mirrors Inspect AI's `Task`, but the matrix (which models to run) is a
//! first-class axis. The code-first builder is the primary authoring surface;
//! `Dataset::jsonl` / `Dataset::json` are the secondary, config-style on-ramps.

use std::sync::Arc;

use crate::content::{Message, Part};
use crate::model::ModelSpec;
use crate::scorer::Scorer;
use crate::subject::Subject;
use crate::{Dataset, Metadata, Params, Sample};

/// A dataset row, in eval-authoring terms.
pub type Case = Sample;

/// A simulated user for an **interactive** (multi-turn) eval. Given the
/// conversation so far (ending with the subject's latest `Assistant` turn), it
/// returns the next user [`Part`]s to send, or `None` to end the exchange.
///
/// The interactive driver (in [`runner`](crate::runner)) invokes the subject
/// once per turn — passing the running conversation via
/// [`RunCx::conversation`](crate::RunCx::conversation) — and calls the responder
/// between turns until it returns `None` or `max_turns` is reached. Scoring is
/// unchanged: scorers grade the final accumulated [`Transcript`](crate::Transcript).
pub type Responder = dyn Fn(&[Message]) -> Option<Vec<Part>> + Send + Sync;

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
    /// How many times to run each cell (trials/repetitions). `1` = a single run.
    /// `> 1` repeats the cell so the host can report pass@k / pass-rate /
    /// variance over a stochastic subject (see [`crate::aggregate`]). The host
    /// may override this with `--trials`.
    pub trials: usize,
    /// Base seed for reproducible trials. When set, trial `t` runs with seed
    /// `seed + t`, so the whole repetition set replays deterministically; the
    /// subject reads it via [`RunCx::seed`](crate::RunCx::seed). `None` leaves
    /// seeding to the subject.
    pub seed: Option<u64>,
    /// Optional simulated user for an interactive (multi-turn) eval. When set,
    /// the runner drives a turn exchange (subject ⇄ responder) up to `max_turns`
    /// instead of a single subject call. `None` for the common single-shot case.
    pub responder: Option<Arc<Responder>>,
    pub metadata: Metadata,
}

impl Eval {
    /// Every combination of axis values, as `params` maps, in cross-product
    /// order. Always yields at least one (empty) map, so a no-axis eval runs a
    /// single cell per `(sample, model)`.
    pub fn axis_combinations(&self) -> Vec<Params> {
        let mut combos = vec![Params::new()];
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
            trials: 1,
            seed: None,
            responder: None,
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
    trials: usize,
    seed: Option<u64>,
    responder: Option<Arc<Responder>>,
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

    /// Run each cell `n` times (trials/repetitions) instead of once, so the host
    /// can report pass@k, pass-rate, and score variance over a stochastic
    /// subject. Trials are repetitions of the *same* cell (unlike an [`axis`],
    /// which forms new cells), grouped back for aggregation. `n` is clamped to at
    /// least 1. The host may override this with `--trials`.
    ///
    /// Pair with [`seed`](EvalBuilder::seed) for reproducible repetitions.
    ///
    /// [`axis`]: EvalBuilder::axis
    pub fn trials(mut self, n: usize) -> Self {
        self.trials = n.max(1);
        self
    }

    /// Set a base seed so trials are reproducible: trial `t` runs with seed
    /// `seed + t`. The subject reads it via [`RunCx::seed`](crate::RunCx::seed)
    /// to seed its RNG / sampling. Without a seed, trials still repeat but the
    /// subject decides its own (non-)determinism.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Make this an **interactive** (multi-turn) eval driven by a simulated
    /// user. The runner exchanges turns with the subject — sending the opening
    /// sample, then each [`Responder`] reply — until the responder returns
    /// `None` or `max_turns` is hit, accumulating the dialog into one transcript
    /// that the scorers grade.
    ///
    /// ```
    /// # use mira::{Eval, Part, Transcript, subject::subject_fn, scorer::succeeded};
    /// let eval = Eval::new("haggle")
    ///     .case("open", "I'd like a discount.")
    ///     .max_turns(3)
    ///     // The subject answers from the running conversation (cx.conversation).
    ///     .subject(subject_fn(|_, cx| async move {
    ///         Transcript::response(format!("turn {}", cx.conversation.len()))
    ///     }))
    ///     // The simulated user pushes back once, then stops.
    ///     .responder(|convo: &[mira::Message]| {
    ///         (convo.len() < 3).then(|| vec![Part::text("still too high")])
    ///     })
    ///     .scorer(succeeded())
    ///     .build();
    /// assert!(eval.responder.is_some());
    /// ```
    pub fn responder(
        mut self,
        responder: impl Fn(&[Message]) -> Option<Vec<Part>> + Send + Sync + 'static,
    ) -> Self {
        self.responder = Some(Arc::new(responder));
        self
    }

    /// Attach a metadata key/value (provenance, suite, observability links).
    /// The value is open-ended JSON, so `"smoke"`, `3`, or a nested object all
    /// work.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
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
            trials: self.trials.max(1),
            seed: self.seed,
            responder: self.responder,
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

    #[test]
    fn trials_default_to_one_and_are_clamped() {
        let eval = Eval::new("e")
            .case("a", "x")
            .subject(subject_fn(|_, _| async { Transcript::default() }))
            .build();
        assert_eq!(eval.trials, 1);
        assert_eq!(eval.seed, None);

        let repeated = Eval::new("e")
            .case("a", "x")
            .trials(0) // clamped up to 1
            .subject(subject_fn(|_, _| async { Transcript::default() }))
            .build();
        assert_eq!(repeated.trials, 1);

        let seeded = Eval::new("e")
            .case("a", "x")
            .trials(8)
            .seed(123)
            .subject(subject_fn(|_, _| async { Transcript::default() }))
            .build();
        assert_eq!(seeded.trials, 8);
        assert_eq!(seeded.seed, Some(123));
    }
}
