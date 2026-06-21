//! [`Runner`]: the in-process run loop. Expands `evals × models × samples` into
//! cells, applies selective filtering, runs each cell through its subject +
//! scorers, and collects a [`RunReport`].
//!
//! Selection mirrors `cargo test`: a free-text `filter` is a substring match on
//! the case key `eval/sample@model`, and `tag` narrows by sample tag. The same
//! [`run_cell`] is used by the protocol [`study`](crate::study), so in-process
//! and over-the-wire runs score identically.

use crate::eval::Eval;
use crate::model::ModelSpec;
use crate::{Params, RunCx, Sample, Score, Transcript, Trial, cell_key, trial_suffix};

/// Execute a single matrix cell's subject, returning the full [`Transcript`]
/// **without scoring**. The first half of [`run_cell`]; used directly by the
/// protocol `execute` method so a long-running subject can be run once and
/// scored later from its stored transcript. `trial` carries this run's
/// repetition index and seed (see [`Trial`]).
pub async fn execute_cell(
    eval: &Eval,
    sample: &Sample,
    model: &ModelSpec,
    params: &Params,
    trial: Trial,
) -> Transcript {
    let cx = RunCx {
        model: model.clone(),
        max_turns: eval.max_turns,
        params: params.clone(),
        trial,
    };
    eval.subject.run(sample, &cx).await
}

/// Score a (possibly previously stored) `transcript` with an eval's scorers,
/// independent of how it was produced. The second half of [`run_cell`]; used by
/// the protocol `score` method to (re-)score without re-executing the subject.
pub async fn score_transcript(eval: &Eval, sample: &Sample, transcript: &Transcript) -> Vec<Score> {
    // An infrastructure failure (budget, rate limit, provider outage, timeout)
    // isn't the model's fault and didn't produce a transcript worth grading.
    // Short-circuit to a single N/A score so the cell is excluded from the
    // verdict and aggregate (neither pass nor fail) — the cell-level dual of a
    // scorer returning `Score::na`. The host retries such cells.
    if transcript.errored_infra() {
        let reason = transcript
            .error
            .clone()
            .unwrap_or_else(|| "infra error".into());
        return vec![Score::na("infra", reason)];
    }
    let mut scores = Vec::with_capacity(eval.scorers.len());
    for scorer in &eval.scorers {
        scores.push(scorer.score(sample, transcript).await);
    }
    scores
}

/// True iff at least one *applicable* scorer ran and all of them passed (the
/// cell verdict). N/A scores (e.g. an unreachable judge) are excluded: a cell
/// passes when every score that *could* be evaluated passed, and at least one
/// did.
pub fn verdict(scores: &[Score]) -> bool {
    scores.iter().any(|s| !s.na) && scores.iter().filter(|s| !s.na).all(|s| s.pass)
}

/// Run a single matrix cell: one sample, one model, one set of axis `params`.
/// Composes [`execute_cell`] + [`score_transcript`] so the fused path scores
/// identically to the split `execute`/`score` path. Shared by the in-process
/// [`Runner`] and the protocol study.
pub async fn run_cell(
    eval: &Eval,
    sample: &Sample,
    model: &ModelSpec,
    params: &Params,
    trial: Trial,
) -> CaseOutcome {
    let transcript = execute_cell(eval, sample, model, params, trial).await;
    let scores = score_transcript(eval, sample, &transcript).await;

    let passed = verdict(&scores);
    let aggregate = aggregate_value(&scores);

    CaseOutcome {
        eval: eval.name.clone(),
        sample_id: sample.id.clone(),
        model: model.label.clone(),
        params: params.clone(),
        trial,
        scores,
        passed,
        aggregate,
        transcript,
    }
}

/// Mean of the *applicable* score values, or 0.0 when none apply. N/A scores are
/// excluded so an unreachable judge doesn't drag the aggregate toward zero.
pub fn aggregate_value(scores: &[Score]) -> f64 {
    let mut sum = 0.0;
    let mut count = 0usize;
    for s in scores {
        if !s.na {
            sum += s.value;
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// The result of one matrix cell: one sample, one model, one axis combination.
#[derive(Clone, Debug)]
pub struct CaseOutcome {
    pub eval: String,
    pub sample_id: String,
    pub model: String,
    /// Extra matrix-axis values for this cell (empty for a model-only matrix).
    pub params: Params,
    /// This run's trial (repetition index, count, and seed).
    pub trial: Trial,
    pub scores: Vec<Score>,
    pub passed: bool,
    pub aggregate: f64,
    pub transcript: Transcript,
}

impl CaseOutcome {
    /// Trial-aware cell identity (a `#index` suffix when the cell is repeated).
    pub fn key(&self) -> String {
        format!(
            "{}{}",
            self.logical_key(),
            trial_suffix(self.trial.index, self.trial.count)
        )
    }

    /// Cell identity shared by all trials of this cell (no `#index` suffix).
    pub fn logical_key(&self) -> String {
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
            let trials = eval.trials.max(1);
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
                        // Repeat the cell `trials` times (1 = a single run),
                        // seeding each trial deterministically when a base seed
                        // is set, so the repetitions are reproducible.
                        for index in 0..trials {
                            let trial = Trial {
                                index,
                                count: trials,
                                // wrapping_add: a huge base seed must not panic
                                // (debug) or differ by build mode (release).
                                seed: eval.seed.map(|s| s.wrapping_add(index as u64)),
                            };
                            report
                                .outcomes
                                .push(run_cell(eval, sample, model, params, trial).await);
                        }
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
    async fn na_scores_are_excluded_from_verdict_and_aggregate() {
        use crate::scorer::scorer;
        // A passing deterministic scorer plus an N/A judge (infra down): the cell
        // passes on the applicable score, and the aggregate ignores the N/A one.
        let eval = Eval::new("e")
            .case("a", "x")
            .subject(subject_fn(|_, _| async { Transcript::response("x") }))
            .scorer(contains("x"))
            .scorer(scorer("judge", |_, _| Score::na("judge", "unreachable")))
            .build();
        let report = Runner::new().add(eval).run().await;
        let out = &report.outcomes[0];
        assert!(out.passed);
        assert_eq!(out.aggregate, 1.0); // mean over applicable scores only

        // A cell whose every scorer is N/A does not pass (nothing was evaluated).
        let all_na = Eval::new("e2")
            .case("a", "x")
            .subject(subject_fn(|_, _| async { Transcript::response("x") }))
            .scorer(scorer("judge", |_, _| Score::na("judge", "unreachable")))
            .build();
        let report = Runner::new().add(all_na).run().await;
        assert!(!report.outcomes[0].passed);
        assert_eq!(report.outcomes[0].aggregate, 0.0);
    }

    #[tokio::test]
    async fn infra_error_short_circuits_scoring_to_na() {
        // A scorer that would FAIL on the errored transcript must never run —
        // an infra error isn't the model's fault, so the cell is N/A, not failed.
        let eval = Eval::new("e")
            .case("a", "x")
            .subject(subject_fn(|_, _| async {
                Transcript::infra_error("provider 503: service unavailable")
            }))
            .scorer(contains("x"))
            .build();
        let report = Runner::new().add(eval).run().await;
        let out = &report.outcomes[0];
        assert_eq!(out.scores.len(), 1);
        assert!(out.scores[0].na); // single N/A score, real scorer skipped
        assert_eq!(out.scores[0].scorer, "infra");
        assert!(!out.passed); // N/A ⇒ not passed …
        assert_eq!(out.aggregate, 0.0); // … and excluded from the aggregate
    }

    #[tokio::test]
    async fn trials_repeat_the_cell_with_seeded_reproducibility() {
        // A "stochastic" subject that just echoes its seed, so we can assert each
        // trial ran with a distinct, deterministic seed (base + index).
        let eval = Eval::new("e")
            .case("a", "x")
            .trials(3)
            .seed(100)
            .subject(subject_fn(|_, cx| async move {
                Transcript::response(format!("seed={:?}", cx.seed()))
            }))
            .scorer(contains("seed="))
            .build();
        let report = Runner::new().add(eval).run().await;
        assert_eq!(report.total(), 3, "one outcome per trial");

        // Trials share a logical key but carry distinct trial keys + seeds.
        let mut keys: Vec<String> = report.outcomes.iter().map(|o| o.key()).collect();
        keys.sort();
        assert_eq!(keys, vec!["e/a@sim#0", "e/a@sim#1", "e/a@sim#2"]);
        for o in &report.outcomes {
            assert_eq!(o.logical_key(), "e/a@sim");
            let expected = 100 + o.trial.index as u64;
            assert_eq!(o.trial.seed, Some(expected));
            assert!(o.transcript.final_response.contains(&expected.to_string()));
        }
    }

    #[tokio::test]
    async fn single_trial_keeps_plain_key() {
        // The default (trials = 1) adds no trial dimension or `#index` suffix.
        let eval = Eval::new("e")
            .case("a", "x")
            .subject(subject_fn(|_, cx| async move {
                assert_eq!(cx.seed(), None);
                Transcript::response("x")
            }))
            .scorer(contains("x"))
            .build();
        let report = Runner::new().add(eval).run().await;
        assert_eq!(report.total(), 1);
        assert_eq!(report.outcomes[0].key(), "e/a@sim");
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
