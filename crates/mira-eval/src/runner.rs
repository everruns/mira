//! [`Runner`]: the in-process run loop. Expands `evals × targets × samples` into
//! cases, applies selective filtering, runs each case through its subject +
//! scorers, and collects a [`RunReport`].
//!
//! Selection mirrors `cargo test`: a free-text `filter` is a substring match on
//! the case key `eval/sample@target`, and `tag` narrows by sample tag. The same
//! [`run_case`] is used by the protocol [`study`](crate::study), so in-process
//! and over-the-wire runs score identically.

use crate::content::{Message, Part, Role};
use crate::eval::{Eval, Responder};
use crate::target::Target;
use crate::{Params, RunCx, Sample, Score, Transcript, Trial, case_key, trial_suffix};

/// Execute a single matrix case's subject, returning the full [`Transcript`]
/// **without scoring**. The first half of [`run_case`]; used directly by the
/// protocol `execute` method so a long-running subject can be run once and
/// scored later from its stored transcript. `trial` carries this run's
/// repetition index and seed (see [`Trial`]).
///
/// When the eval has a [`responder`](crate::eval::EvalBuilder::responder), this
/// drives an **interactive** turn exchange (subject ⇄ simulated user) and
/// accumulates the dialog into one transcript; otherwise the subject runs once.
pub async fn execute_case(
    eval: &Eval,
    sample: &Sample,
    target: &Target,
    params: &Params,
    trial: Trial,
) -> Transcript {
    let mut cx = RunCx {
        target: target.clone(),
        max_turns: eval.max_turns,
        params: params.clone(),
        trial,
        conversation: Vec::new(),
    };
    match &eval.responder {
        None => eval.subject.run(sample, &cx).await,
        Some(responder) => drive_interactive(eval, sample, &mut cx, responder.as_ref()).await,
    }
}

/// Drive an interactive eval: invoke the subject once per turn (handing it the
/// running conversation via [`RunCx::conversation`]), append the simulated
/// user's reply, and repeat until the [`Responder`] ends it or `max_turns` is
/// reached. The turns are folded into one [`Transcript`] so scoring is unchanged.
async fn drive_interactive(
    eval: &Eval,
    sample: &Sample,
    cx: &mut RunCx,
    responder: &Responder,
) -> Transcript {
    // The opening user turn is the sample's (multimodal) prompt.
    let mut convo: Vec<Message> = vec![Message::new(Role::User, sample.prompt_parts())];
    let mut combined = Transcript::default();
    let cap = eval.max_turns.max(1);

    for turn in 1..=cap {
        cx.conversation = convo.clone();
        let t = eval.subject.run(sample, cx).await;
        merge_turn(&mut combined, &t);
        combined.iterations = turn;
        // Record the subject's turn so the responder (and transcript) see it.
        convo.push(Message::new(Role::Assistant, assistant_parts(&t)));

        // An error ends the exchange — carry it onto the combined transcript.
        if t.error.is_some() {
            combined.error = t.error.clone();
            combined.error_kind = t.error_kind;
            break;
        }
        if turn == cap {
            break;
        }
        // Ask the simulated user for the next turn; empty / None ends it.
        match responder(&convo) {
            Some(parts) if !parts.is_empty() => convo.push(Message::new(Role::User, parts)),
            _ => break,
        }
    }

    combined.tool_calls_count = combined.tool_calls.len();
    combined
}

/// Fold one turn's transcript into the accumulated interactive transcript:
/// last response wins, usage/duration/tools/events/files/metrics/metadata
/// accumulate.
fn merge_turn(combined: &mut Transcript, t: &Transcript) {
    combined.final_response = t.final_response.clone();
    combined.usage.add(&t.usage);
    combined.timing.duration_ms += t.timing.duration_ms;
    if combined.timing.time_to_first_token_ms.is_none() {
        combined.timing.time_to_first_token_ms = t.timing.time_to_first_token_ms;
    }
    combined.tool_calls.extend(t.tool_calls.iter().cloned());
    combined.events.extend(t.events.iter().cloned());
    for (k, v) in &t.files {
        combined.files.insert(k.clone(), v.clone());
    }
    for (k, v) in &t.metrics {
        combined.metrics.insert(k.clone(), *v);
    }
    for (k, v) in &t.metadata {
        combined.metadata.insert(k.clone(), v.clone());
    }
    combined.output = t.output.clone();
}

/// The subject's turn as conversation parts: its multimodal `output` when set,
/// else the text `final_response`.
fn assistant_parts(t: &Transcript) -> Vec<Part> {
    if !t.output.is_empty() {
        return t.output.clone();
    }
    vec![Part::text(t.final_response.clone())]
}

/// Score a (possibly previously stored) `transcript` with an eval's scorers,
/// independent of how it was produced. The second half of [`run_case`]; used by
/// the protocol `score` method to (re-)score without re-executing the subject.
pub async fn score_transcript(eval: &Eval, sample: &Sample, transcript: &Transcript) -> Vec<Score> {
    // An infrastructure failure (budget, rate limit, provider outage, timeout)
    // isn't the target's fault and didn't produce a transcript worth grading.
    // Short-circuit to a single N/A score so the case is excluded from the
    // verdict and aggregate (neither pass nor fail) — the case-level dual of a
    // scorer returning `Score::na`. The host retries such cases.
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
/// case verdict). N/A scores (e.g. an unreachable judge) are excluded: a case
/// passes when every score that *could* be evaluated passed, and at least one
/// did.
pub fn verdict(scores: &[Score]) -> bool {
    scores.iter().any(|s| !s.na) && scores.iter().filter(|s| !s.na).all(|s| s.pass)
}

/// Run a single matrix case: one sample, one target, one set of axis `params`.
/// Composes [`execute_case`] + [`score_transcript`] so the fused path scores
/// identically to the split `execute`/`score` path. Shared by the in-process
/// [`Runner`] and the protocol study.
pub async fn run_case(
    eval: &Eval,
    sample: &Sample,
    target: &Target,
    params: &Params,
    trial: Trial,
) -> CaseOutcome {
    let transcript = execute_case(eval, sample, target, params, trial).await;
    let scores = score_transcript(eval, sample, &transcript).await;

    let passed = verdict(&scores);
    let aggregate = aggregate_value(&scores);

    CaseOutcome {
        eval: eval.name.clone(),
        sample_id: sample.id.clone(),
        target: target.label.clone(),
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

/// The result of one matrix case: one sample, one target, one axis combination.
#[derive(Clone, Debug)]
pub struct CaseOutcome {
    pub eval: String,
    pub sample_id: String,
    pub target: String,
    /// Extra matrix-axis values for this case (empty for a target-only matrix).
    pub params: Params,
    /// This run's trial (repetition index, count, and seed).
    pub trial: Trial,
    pub scores: Vec<Score>,
    pub passed: bool,
    pub aggregate: f64,
    pub transcript: Transcript,
}

impl CaseOutcome {
    /// Trial-aware case identity (a `#index` suffix when the case is repeated).
    pub fn key(&self) -> String {
        format!(
            "{}{}",
            self.logical_key(),
            trial_suffix(self.trial.index, self.trial.count)
        )
    }

    /// Case identity shared by all trials of this case (no `#index` suffix).
    pub fn logical_key(&self) -> String {
        case_key(&self.eval, &self.sample_id, &self.target, &self.params)
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
    targets: Option<Vec<String>>,
    samples: Option<Vec<String>>,
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

    /// Substring filter on the case key `eval/sample@target` (like `cargo test PAT`).
    pub fn filter(mut self, filter: Option<String>) -> Self {
        self.filter = filter;
        self
    }

    /// Only run samples carrying this tag.
    pub fn tag(mut self, tag: Option<String>) -> Self {
        self.tag = tag;
        self
    }

    /// Restrict the matrix to target labels matching these glob patterns
    /// (`anthropic/*`, `sim`, …). A literal pattern is an exact label.
    pub fn targets(mut self, targets: Option<Vec<String>>) -> Self {
        self.targets = targets;
        self
    }

    /// Restrict the matrix to sample ids matching these glob patterns. A literal
    /// pattern is an exact id; `france*`, `geo/{a,b}` select by shape.
    pub fn samples(mut self, samples: Option<Vec<String>>) -> Self {
        self.samples = samples;
        self
    }

    /// True if a case passes the active selection. `filter` is a cross-cutting
    /// substring on the whole case key (the `cargo test PAT` convenience);
    /// `samples`/`targets` are per-dimension glob selectors.
    fn selected(&self, key: &str, sample: &Sample, target: &Target) -> bool {
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
        if let Some(allow) = &self.samples
            && !allow.iter().any(|p| crate::glob_match(p, &sample.id))
        {
            return false;
        }
        if let Some(allow) = &self.targets
            && !allow.iter().any(|p| crate::glob_match(p, &target.label))
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
            for target in &eval.targets {
                for sample in &eval.dataset.samples {
                    for params in &combos {
                        let key = case_key(&eval.name, &sample.id, &target.label, params);
                        if !self.selected(&key, sample, target) {
                            continue;
                        }
                        if !target.available {
                            report.skipped.push(format!("{key} (unavailable)"));
                            continue;
                        }
                        // Repeat the case `trials` times (1 = a single run),
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
                                .push(run_case(eval, sample, target, params, trial).await);
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
            .add_sample(Sample::new("hi", "say hi").tag("smoke"))
            .add_sample(Sample::new("bye", "say bye"))
            .subject(subject_fn(|s, _| async move {
                Transcript::response(s.input.join(" "))
            }))
            .scorer(contains("say"))
            .build()
    }

    #[tokio::test]
    async fn runs_all_cases() {
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
    async fn samples_select_by_glob() {
        // `b*` matches sample id `bye` but not `hi`.
        let report = Runner::new()
            .add(echo_eval("greet"))
            .samples(Some(vec!["b*".into()]))
            .run()
            .await;
        assert_eq!(report.total(), 1);
        assert_eq!(report.outcomes[0].sample_id, "bye");
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
            .sample("a", "x")
            .targets([Target::sim().available(false)])
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
        // A passing deterministic scorer plus an N/A judge (infra down): the case
        // passes on the applicable score, and the aggregate ignores the N/A one.
        let eval = Eval::new("e")
            .sample("a", "x")
            .subject(subject_fn(|_, _| async { Transcript::response("x") }))
            .scorer(contains("x"))
            .scorer(scorer("judge", |_, _| Score::na("judge", "unreachable")))
            .build();
        let report = Runner::new().add(eval).run().await;
        let out = &report.outcomes[0];
        assert!(out.passed);
        assert_eq!(out.aggregate, 1.0); // mean over applicable scores only

        // A case whose every scorer is N/A does not pass (nothing was evaluated).
        let all_na = Eval::new("e2")
            .sample("a", "x")
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
        // an infra error isn't the target's fault, so the case is N/A, not failed.
        let eval = Eval::new("e")
            .sample("a", "x")
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
    async fn trials_repeat_the_case_with_seeded_reproducibility() {
        // A "stochastic" subject that just echoes its seed, so we can assert each
        // trial ran with a distinct, deterministic seed (base + index).
        let eval = Eval::new("e")
            .sample("a", "x")
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
            .sample("a", "x")
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
    async fn interactive_eval_exchanges_turns() {
        use crate::scorer::{succeeded, turns_within};
        // The subject answers from the running conversation; the simulated user
        // pushes back twice, then a third reply is suppressed by max_turns.
        let eval = Eval::new("chat")
            .sample("open", "hello")
            .max_turns(3)
            .subject(subject_fn(|_, cx| async move {
                // One assistant reply per turn, numbered by conversation length.
                Transcript::response(format!("reply to {} msgs", cx.conversation.len()))
                    .with_metric("turn_len", cx.conversation.len() as f64)
            }))
            .responder(|convo: &[Message]| {
                // Keep going while under the cap; the driver enforces max_turns.
                Some(vec![Part::text(format!("more ({})", convo.len()))])
            })
            .scorer(succeeded())
            .scorer(turns_within(3))
            .build();
        let report = Runner::new().add(eval).run().await;
        let out = &report.outcomes[0];
        assert!(out.passed);
        // Exactly max_turns subject invocations.
        assert_eq!(out.transcript.iterations, 3);
        // The conversation grew: turn 1 saw 1 msg (opening user), turn 3 saw 5
        // (user, asst, user, asst, user).
        assert!(out.transcript.final_response.contains("5 msgs"));
    }

    #[tokio::test]
    async fn interactive_responder_can_end_early() {
        use crate::scorer::succeeded;
        let eval = Eval::new("chat")
            .sample("open", "hi")
            .max_turns(10)
            .subject(subject_fn(|_, _| async { Transcript::response("ok") }))
            // End immediately after the first assistant turn.
            .responder(|_: &[Message]| None)
            .scorer(succeeded())
            .build();
        let report = Runner::new().add(eval).run().await;
        // Responder ended it after one turn despite a generous max_turns.
        assert_eq!(report.outcomes[0].transcript.iterations, 1);
    }

    #[tokio::test]
    async fn empty_scorers_means_not_passed() {
        let eval = Eval::new("e")
            .sample("a", "x")
            .subject(subject_fn(|_, _| async { Transcript::response("x") }))
            .build();
        let report = Runner::new().add(eval).run().await;
        assert!(!report.outcomes[0].passed);
        assert_eq!(report.outcomes[0].aggregate, 0.0);
    }
}
