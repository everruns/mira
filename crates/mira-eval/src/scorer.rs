//! [`Scorer`]s grade a [`Transcript`]. They compose freely: an [`Eval`] holds a
//! `Vec<Box<dyn Scorer>>` and every scorer runs against every transcript. This
//! is one open, code-first vocabulary — deterministic built-ins, an
//! arbitrary-closure escape hatch, and LLM-as-judge — rather than a closed enum.
//!
//! [`Eval`]: crate::eval::Eval

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::{Sample, Score, Transcript};

/// Grades a [`Transcript`] for one [`Sample`] into a [`Score`].
#[async_trait]
pub trait Scorer: Send + Sync {
    /// A stable, human-readable name (shown in reports).
    fn name(&self) -> String;
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score;
}

// ----- closure scorer -------------------------------------------------------

/// Wraps a plain closure as a scorer — the escape hatch for one-off checks. The
/// bar for bespoke logic is a closure, not a new type.
pub struct FnScorer {
    name: String,
    f: Box<dyn Fn(&Sample, &Transcript) -> Score + Send + Sync>,
}

#[async_trait]
impl Scorer for FnScorer {
    fn name(&self) -> String {
        self.name.clone()
    }
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score {
        (self.f)(sample, transcript)
    }
}

/// Build a scorer from a closure: `scorer("nonempty", |_, t| ...)`.
pub fn scorer(
    name: impl Into<String>,
    f: impl Fn(&Sample, &Transcript) -> Score + Send + Sync + 'static,
) -> Box<dyn Scorer> {
    Box::new(FnScorer {
        name: name.into(),
        f: Box::new(f),
    })
}

// ----- deterministic built-ins ----------------------------------------------

/// Passes if the final response contains `needle`.
pub fn contains(needle: impl Into<String>) -> Box<dyn Scorer> {
    let needle = needle.into();
    scorer(format!("contains({needle:?})"), move |_, t| {
        if t.final_response.contains(&needle) {
            Score::pass("contains", format!("found {needle:?}"))
        } else {
            Score::fail("contains", format!("missing {needle:?}"))
        }
    })
}

/// Passes if the final response does NOT contain `needle`.
pub fn not_contains(needle: impl Into<String>) -> Box<dyn Scorer> {
    let needle = needle.into();
    scorer(format!("not_contains({needle:?})"), move |_, t| {
        if t.final_response.contains(&needle) {
            Score::fail("not_contains", format!("unexpectedly found {needle:?}"))
        } else {
            Score::pass("not_contains", format!("absent {needle:?}"))
        }
    })
}

/// Passes if the final response (case-insensitively) equals `expected`.
pub fn equals(expected: impl Into<String>) -> Box<dyn Scorer> {
    let expected = expected.into();
    scorer(format!("equals({expected:?})"), move |_, t| {
        if t.final_response
            .trim()
            .eq_ignore_ascii_case(expected.trim())
        {
            Score::pass("equals", "exact match")
        } else {
            Score::fail(
                "equals",
                format!("expected {expected:?}, got {:?}", t.final_response),
            )
        }
    })
}

/// Passes if the final response matches the regex `pattern`.
pub fn regex(pattern: impl Into<String>) -> Box<dyn Scorer> {
    let pattern = pattern.into();
    let compiled = regex::Regex::new(&pattern);
    scorer(format!("regex({pattern:?})"), move |_, t| match &compiled {
        Ok(re) if re.is_match(&t.final_response) => {
            Score::pass("regex", format!("matched {pattern:?}"))
        }
        Ok(_) => Score::fail("regex", format!("no match for {pattern:?}")),
        Err(e) => Score::fail("regex", format!("bad pattern: {e}")),
    })
}

/// Passes if the final response matches the sample's string `target`. Samples
/// without a string target fail (the eval is misconfigured).
pub fn matches_target() -> Box<dyn Scorer> {
    scorer("matches_target", move |s, t| match s.target_str() {
        Some(target) if t.final_response.trim() == target.trim() => {
            Score::pass("matches_target", "matched target")
        }
        Some(target) => Score::fail("matches_target", format!("expected {target:?}")),
        None => Score::fail("matches_target", "sample has no string target"),
    })
}

/// Passes if a file at `path` exists in the subject's captured workspace, and
/// (optionally) contains `needle`.
pub fn file_contains(path: impl Into<String>, needle: impl Into<String>) -> Box<dyn Scorer> {
    let path = path.into();
    let needle = needle.into();
    scorer(
        format!("file_contains({path}, {needle:?})"),
        move |_, t| match t.files.get(&path) {
            Some(contents) if contents.contains(&needle) => {
                Score::pass("file_contains", format!("{path} contains {needle:?}"))
            }
            Some(_) => Score::fail("file_contains", format!("{path} missing {needle:?}")),
            None => Score::fail("file_contains", format!("no such file: {path}")),
        },
    )
}

/// Passes if a file at `path` exists in the subject's captured workspace.
pub fn file_exists(path: impl Into<String>) -> Box<dyn Scorer> {
    let path = path.into();
    scorer(format!("file_exists({path})"), move |_, t| {
        if t.files.contains_key(&path) {
            Score::pass("file_exists", format!("{path} exists"))
        } else {
            Score::fail("file_exists", format!("no such file: {path}"))
        }
    })
}

/// Passes if a tool named `tool` was invoked at least once.
pub fn tool_called(tool: impl Into<String>) -> Box<dyn Scorer> {
    let tool = tool.into();
    scorer(format!("tool_called({tool})"), move |_, t| {
        if t.tool_calls.iter().any(|n| n == &tool) {
            Score::pass("tool_called", format!("{tool} was called"))
        } else {
            Score::fail("tool_called", format!("{tool} never called"))
        }
    })
}

/// Passes if the run used no more than `max` tool calls.
pub fn tool_calls_within(max: usize) -> Box<dyn Scorer> {
    scorer(format!("tool_calls_within({max})"), move |_, t| {
        if t.tool_calls_count <= max {
            Score::pass(
                "tool_calls_within",
                format!("{} <= {max}", t.tool_calls_count),
            )
        } else {
            Score::fail(
                "tool_calls_within",
                format!("{} > {max}", t.tool_calls_count),
            )
        }
    })
}

/// Passes if the run took no more than `max` reasoning iterations.
pub fn turns_within(max: usize) -> Box<dyn Scorer> {
    scorer(format!("turns_within({max})"), move |_, t| {
        if t.iterations <= max {
            Score::pass("turns_within", format!("{} <= {max}", t.iterations))
        } else {
            Score::fail("turns_within", format!("{} > {max}", t.iterations))
        }
    })
}

/// Passes if the run stayed at or under `max_usd` total cost.
pub fn cost_within(max_usd: f64) -> Box<dyn Scorer> {
    scorer(format!("cost_within(${max_usd})"), move |_, t| {
        if t.usage.cost_usd <= max_usd {
            Score::pass(
                "cost_within",
                format!("${:.4} <= ${max_usd}", t.usage.cost_usd),
            )
        } else {
            Score::fail(
                "cost_within",
                format!("${:.4} > ${max_usd}", t.usage.cost_usd),
            )
        }
    })
}

/// Passes if the subject completed without an error.
pub fn succeeded() -> Box<dyn Scorer> {
    scorer("succeeded", move |_, t| match &t.error {
        None => Score::pass("succeeded", "no error"),
        Some(e) => Score::fail("succeeded", format!("error: {e}")),
    })
}

// ----- model-graded scorer --------------------------------------------------

/// An async judge: given a rubric and the transcript, return a [`Score`].
/// Backed by a separate model — keeping the judge independent of the model
/// under test is the standard guidance, though the framework does not enforce
/// it.
pub type JudgeFn =
    Box<dyn Fn(String, Transcript) -> Pin<Box<dyn Future<Output = Score> + Send>> + Send + Sync>;

pub struct ModelGraded {
    rubric: String,
    judge: JudgeFn,
}

#[async_trait]
impl Scorer for ModelGraded {
    fn name(&self) -> String {
        format!("model_graded({:?})", self.rubric)
    }
    async fn score(&self, _sample: &Sample, transcript: &Transcript) -> Score {
        (self.judge)(self.rubric.clone(), transcript.clone()).await
    }
}

/// Grade a transcript against a natural-language `rubric` using a `judge`
/// (typically a cheaper model). LLM-as-judge is just another [`Scorer`], not a
/// special case in the engine.
pub fn model_graded(rubric: impl Into<String>, judge: JudgeFn) -> Box<dyn Scorer> {
    Box::new(ModelGraded {
        rubric: rubric.into(),
        judge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn transcript() -> Transcript {
        Transcript {
            final_response: "The answer is 42.".into(),
            iterations: 2,
            tool_calls_count: 1,
            tool_calls: vec!["calc".into()],
            usage: crate::Usage {
                input_tokens: 5,
                output_tokens: 3,
                cost_usd: 0.01,
            },
            files: BTreeMap::from([("out.txt".into(), "hello world".into())]),
            ..Default::default()
        }
    }

    async fn run(scorer: Box<dyn Scorer>, sample: &Sample) -> Score {
        scorer.score(sample, &transcript()).await
    }

    #[tokio::test]
    async fn text_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(contains("42"), &s).await.pass);
        assert!(!run(contains("99"), &s).await.pass);
        assert!(run(not_contains("99"), &s).await.pass);
        assert!(run(regex(r"answer is \d+"), &s).await.pass);
        assert!(!run(regex(r"^\d+$"), &s).await.pass);
        assert!(run(equals("the answer is 42."), &s).await.pass); // case-insensitive
    }

    #[tokio::test]
    async fn target_scorer() {
        let s = Sample::new("a", "q").target("The answer is 42.");
        assert!(run(matches_target(), &s).await.pass);
        let s2 = Sample::new("b", "q"); // no target
        assert!(!run(matches_target(), &s2).await.pass);
    }

    #[tokio::test]
    async fn structural_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(tool_called("calc"), &s).await.pass);
        assert!(!run(tool_called("grep"), &s).await.pass);
        assert!(run(tool_calls_within(1), &s).await.pass);
        assert!(!run(tool_calls_within(0), &s).await.pass);
        assert!(run(turns_within(2), &s).await.pass);
        assert!(run(cost_within(0.05), &s).await.pass);
        assert!(!run(cost_within(0.001), &s).await.pass);
        assert!(run(succeeded(), &s).await.pass);
    }

    #[tokio::test]
    async fn file_scorers() {
        let s = Sample::new("a", "q");
        assert!(run(file_exists("out.txt"), &s).await.pass);
        assert!(!run(file_exists("nope.txt"), &s).await.pass);
        assert!(run(file_contains("out.txt", "hello"), &s).await.pass);
        assert!(!run(file_contains("out.txt", "bye"), &s).await.pass);
    }

    #[tokio::test]
    async fn model_graded_is_just_a_scorer() {
        let judge: JudgeFn = Box::new(|rubric, t| {
            Box::pin(async move {
                let pass = t.final_response.contains("42");
                Score::graded("judge", if pass { 1.0 } else { 0.0 }, 0.5, rubric)
            })
        });
        let s = run(
            model_graded("is it answered?", judge),
            &Sample::new("a", "q"),
        )
        .await;
        assert!(s.pass);
    }
}
