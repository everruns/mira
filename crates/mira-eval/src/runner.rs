//! [`Runner`]: the in-process run loop. Expands `evals × models × samples` into
//! cells, applies selective filtering, runs each cell through its subject +
//! scorers, and collects a [`RunReport`].
//!
//! Selection mirrors `cargo test`: a free-text `filter` is a substring match on
//! the case key `eval/sample@model`, and `tag` narrows by sample tag. The same
//! [`run_cell`] is used by the protocol [`server`](crate::server), so in-process
//! and over-the-wire runs score identically.

use crate::eval::Eval;
use crate::model::ModelSpec;
use crate::{Metadata, RunCx, Sample, Score, Transcript, cell_key};

/// Run a single matrix cell: one sample, one model, one set of axis `params`.
/// Shared by the in-process [`Runner`] and the protocol server.
pub async fn run_cell(
    eval: &Eval,
    sample: &Sample,
    model: &ModelSpec,
    params: &Metadata,
) -> CaseOutcome {
    let cx = RunCx {
        model: model.clone(),
        max_turns: eval.max_turns,
        params: params.clone(),
    };
    let transcript = eval.subject.run(sample, &cx).await;

    let mut scores = Vec::with_capacity(eval.scorers.len());
    for scorer in &eval.scorers {
        scores.push(scorer.score(sample, &transcript).await);
    }

    let passed = !scores.is_empty() && scores.iter().all(|s| s.pass);
    let aggregate = aggregate_value(&scores);

    CaseOutcome {
        eval: eval.name.clone(),
        sample_id: sample.id.clone(),
        model: model.label.clone(),
        params: params.clone(),
        scores,
        passed,
        aggregate,
        transcript,
    }
}

/// Mean of score values, or 0.0 for an empty set.
pub fn aggregate_value(scores: &[Score]) -> f64 {
    if scores.is_empty() {
        return 0.0;
    }
    scores.iter().map(|s| s.value).sum::<f64>() / scores.len() as f64
}

/// The result of one matrix cell: one sample, one model, one axis combination.
#[derive(Clone, Debug)]
pub struct CaseOutcome {
    pub eval: String,
    pub sample_id: String,
    pub model: String,
    /// Extra matrix-axis values for this cell (empty for a model-only matrix).
    pub params: Metadata,
    pub scores: Vec<Score>,
    pub passed: bool,
    pub aggregate: f64,
    pub transcript: Transcript,
}

impl CaseOutcome {
    pub fn key(&self) -> String {
        cell_key(&self.eval, &self.sample_id, &self.model, &self.params)
    }
}

/// Aggregate result of an in-process run.
#[derive(Clone, Debug, Default)]
pub struct RunReport {
    pub outcomes: Vec<CaseOutcome>,
    pub skipped: Vec<String>,
}

impl RunReport {
    pub fn total(&self) -> usize {
        self.outcomes.len()
    }
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.passed).count()
    }
    pub fn failed(&self) -> usize {
        self.total() - self.passed()
    }
    pub fn all_passed(&self) -> bool {
        self.failed() == 0
    }
}

/// Runs a suite of [`Eval`]s in-process with optional selection.
#[derive(Default)]
pub struct Runner {
    evals: Vec<Eval>,
    filter: Option<String>,
    tag: Option<String>,
    models: Option<Vec<String>>,
}

impl Runner {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, eval: Eval) -> Self {
        self.evals.push(eval);
        self
    }

    /// Add many evals at once (e.g. from `registered_evals()`).
    pub fn extend(mut self, evals: impl IntoIterator<Item = Eval>) -> Self {
        self.evals.extend(evals);
        self
    }

    /// Substring filter on the case key `eval/sample@model` (like `cargo test PAT`).
    pub fn filter(mut self, filter: Option<String>) -> Self {
        self.filter = filter;
        self
    }

    /// Only run samples carrying this tag.
    pub fn tag(mut self, tag: Option<String>) -> Self {
        self.tag = tag;
        self
    }

    /// Restrict the matrix to these model labels.
    pub fn models(mut self, models: Option<Vec<String>>) -> Self {
        self.models = models;
        self
    }

    /// True if a cell passes the active selection (filter, tag, model labels).
    fn selected(&self, key: &str, sample: &Sample, model: &ModelSpec) -> bool {
        if let Some(f) = &self.filter
            && !key.contains(f.as_str())
        {
            return false;
        }
        if let Some(tag) = &self.tag
            && !sample.tags.iter().any(|t| t == tag)
        {
            return false;
        }
        if let Some(allow) = &self.models
            && !allow.iter().any(|m| m == &model.label)
        {
            return false;
        }
        true
    }

    pub async fn run(&self) -> RunReport {
        let mut report = RunReport::default();

        for eval in &self.evals {
            let combos = eval.axis_combinations();
            for model in &eval.models {
                for sample in &eval.dataset.samples {
                    for params in &combos {
                        let key = cell_key(&eval.name, &sample.id, &model.label, params);
                        if !self.selected(&key, sample, model) {
                            continue;
                        }
                        if !model.available {
                            report.skipped.push(format!("{key} (unavailable)"));
                            continue;
                        }
                        report
                            .outcomes
                            .push(run_cell(eval, sample, model, params).await);
                    }
                }
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::contains;
    use crate::subject::subject_fn;

    fn echo_eval(name: &str) -> Eval {
        Eval::new(name)
            .sample(Sample::new("hi", "say hi").tag("smoke"))
            .sample(Sample::new("bye", "say bye"))
            .subject(subject_fn(|s, _| async move {
                Transcript::response(s.input.join(" "))
            }))
            .scorer(contains("say"))
            .build()
    }

    #[tokio::test]
    async fn runs_all_cells() {
        let report = Runner::new().add(echo_eval("greet")).run().await;
        assert_eq!(report.total(), 2);
        assert!(report.all_passed());
    }

    #[tokio::test]
    async fn filter_selects_by_key() {
        let report = Runner::new()
            .add(echo_eval("greet"))
            .filter(Some("hi".into()))
            .run()
            .await;
        assert_eq!(report.total(), 1);
        assert_eq!(report.outcomes[0].sample_id, "hi");
    }

    #[tokio::test]
    async fn tag_narrows() {
        let report = Runner::new()
            .add(echo_eval("greet"))
            .tag(Some("smoke".into()))
            .run()
            .await;
        assert_eq!(report.total(), 1);
    }

    #[tokio::test]
    async fn unavailable_model_is_skipped_not_failed() {
        let eval = Eval::new("e")
            .case("a", "x")
            .models([ModelSpec::sim().available(false)])
            .subject(subject_fn(|_, _| async { Transcript::response("x") }))
            .scorer(contains("x"))
            .build();
        let report = Runner::new().add(eval).run().await;
        assert_eq!(report.total(), 0);
        assert_eq!(report.skipped.len(), 1);
        assert!(report.all_passed()); // no failures
    }

    #[tokio::test]
    async fn empty_scorers_means_not_passed() {
        let eval = Eval::new("e")
            .case("a", "x")
            .subject(subject_fn(|_, _| async { Transcript::response("x") }))
            .build();
        let report = Runner::new().add(eval).run().await;
        assert!(!report.outcomes[0].passed);
        assert_eq!(report.outcomes[0].aggregate, 0.0);
    }
}
