//! Diagnostic scorers consume explicit evidence. Retrieval IDs are unique,
//! ranked strings in `transcript.metadata.retrieved_ids`; references live in
//! `sample.expected.relevant_ids`. Missing/malformed evidence fails visibly.
use super::{Scorer, scorer};
use crate::{Sample, Score, Transcript};
use serde_json::Value;
use std::collections::BTreeSet;

/// Require arguments and a correlated observation on the SAME tool invocation.
pub fn tool_invocation(
    tool: impl Into<String>,
    pointer: impl Into<String>,
    expected: Value,
    observation: impl Into<String>,
) -> Box<dyn Scorer> {
    let (tool, pointer, observation) = (tool.into(), pointer.into(), observation.into());
    let name = format!("tool_invocation({tool},{pointer},{expected},{observation:?})");
    scorer(name.clone(), move |_, t| {
        let Some(tr) = &t.trajectory else {
            return Score::fail(&name, "subject reported no trajectory");
        };
        let calls: Vec<_> = tr.steps.iter().flat_map(|s| &s.tool_calls).collect();
        if calls
            .iter()
            .map(|c| &c.tool_call_id)
            .collect::<BTreeSet<_>>()
            .len()
            != calls.len()
        {
            return Score::fail(
                &name,
                "duplicate tool_call_id values make observation correlation ambiguous",
            );
        }
        let mut mismatch = None;
        for (index, step) in tr.steps.iter().enumerate() {
            for call in &step.tool_calls {
                if call.function_name == tool && call.arguments.pointer(&pointer) == Some(&expected)
                {
                    mismatch = Some(format!(
                        "step_id={} call_id={}: arguments match; correlated observation does not",
                        step.step_id, call.tool_call_id
                    ));
                    let found = tr.steps[index..]
                        .iter()
                        .filter_map(|s| s.observation.as_ref())
                        .flat_map(|o| &o.results)
                        .any(|r| {
                            r.source_call_id.as_deref() == Some(call.tool_call_id.as_str())
                                && r.content
                                    .as_ref()
                                    .is_some_and(|c| c.text().contains(&observation))
                        });
                    if found {
                        return Score::pass(
                            &name,
                            format!(
                                "step_id={} call_id={}: arguments and correlated observation match",
                                step.step_id, call.tool_call_id
                            ),
                        );
                    }
                }
            }
        }
        Score::fail(
            &name,
            mismatch.unwrap_or_else(|| {
                "no single invocation matches arguments and correlated observation".into()
            }),
        )
    })
}

/// Require a tool subsequence across strictly increasing trajectory steps.
/// Other calls may intervene. Calls in one step do not establish causal order.
pub fn tool_sequence(tools: impl IntoIterator<Item = impl Into<String>>) -> Box<dyn Scorer> {
    let tools: Vec<String> = tools.into_iter().map(Into::into).collect();
    let name = format!("tool_sequence({tools:?})");
    scorer(name.clone(), move |_, t| {
        let Some(tr) = &t.trajectory else {
            return Score::fail(&name, "subject reported no trajectory");
        };
        if tr
            .steps
            .windows(2)
            .any(|pair| pair[0].step_id >= pair[1].step_id)
        {
            return Score::fail(&name, "step_id values must increase strictly");
        }
        if tools.is_empty() {
            return Score::fail(&name, "tool sequence must not be empty");
        }
        let mut at = 0;
        let mut steps = Vec::new();
        for step in &tr.steps {
            if step.tool_calls.iter().any(|c| c.function_name == tools[at]) {
                steps.push(step.step_id);
                at += 1;
                if at == tools.len() {
                    return Score::pass(&name, format!("matched step_id values {steps:?}"));
                }
            }
        }
        Score::fail(
            &name,
            format!(
                "matched {at}/{} ordered tools; step_id values {steps:?}",
                tools.len()
            ),
        )
    })
}

fn ids(value: Option<&Value>, label: &str) -> Result<Vec<String>, String> {
    let list = value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {label} string array"))?;
    let ids: Vec<String> = list
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("invalid {label} ID"))
        })
        .collect::<Result<_, _>>()?;
    if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(format!("duplicate {label} IDs"));
    }
    Ok(ids)
}

fn retrieval(sample: &Sample, t: &Transcript) -> Result<(Vec<String>, Vec<String>), String> {
    let relevant = ids(
        sample.expected.as_ref().and_then(|v| v.get("relevant_ids")),
        "expected.relevant_ids",
    )?;
    if relevant.is_empty() {
        return Err("expected.relevant_ids must not be empty".into());
    }
    Ok((
        ids(t.metadata.get("retrieved_ids"), "retrieved_ids")?,
        relevant,
    ))
}

#[derive(Clone, Copy, Debug)]
enum Retrieval {
    Precision,
    Recall,
    AveragePrecision,
}
fn retrieval_scorer(kind: Retrieval, k: usize, threshold: f64) -> Box<dyn Scorer> {
    let label = match kind {
        Retrieval::Precision => "retrieval_precision_at",
        Retrieval::Recall => "retrieval_recall_at",
        Retrieval::AveragePrecision => "retrieval_average_precision_at",
    };
    let name = format!("{label}({k},{threshold})");
    scorer(name.clone(), move |s, t| {
        if k == 0 || !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Score::fail(&name, "k must be positive and threshold in 0..1");
        }
        let (ranked, relevant) = match retrieval(s, t) {
            Ok(v) => v,
            Err(e) => return Score::fail(&name, e),
        };
        let mut hits = 0;
        let mut precision_sum = 0.0;
        for (i, id) in ranked.iter().take(k).enumerate() {
            if relevant.contains(id) {
                hits += 1;
                precision_sum += hits as f64 / (i + 1) as f64;
            }
        }
        let value = match kind {
            Retrieval::Precision => hits as f64 / k as f64,
            Retrieval::Recall => hits as f64 / relevant.len() as f64,
            Retrieval::AveragePrecision => precision_sum / relevant.len().min(k) as f64,
        };
        Score::graded(
            &name,
            value,
            threshold,
            format!(
                "{hits} relevant in top {k}; {} relevant references",
                relevant.len()
            ),
        )
    })
}
/// Precision@k: relevant hits divided by k (missing ranks count as misses).
pub fn retrieval_precision_at(k: usize, threshold: f64) -> Box<dyn Scorer> {
    retrieval_scorer(Retrieval::Precision, k, threshold)
}
/// Recall@k: relevant hits divided by the total relevant reference IDs.
pub fn retrieval_recall_at(k: usize, threshold: f64) -> Box<dyn Scorer> {
    retrieval_scorer(Retrieval::Recall, k, threshold)
}
/// AP@k: sum of precision at relevant ranks / min(k, reference count).
pub fn retrieval_average_precision_at(k: usize, threshold: f64) -> Box<dyn Scorer> {
    retrieval_scorer(Retrieval::AveragePrecision, k, threshold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn arguments_and_result_cannot_match_different_invocations() {
        let t = Transcript::from_trajectory(serde_json::from_value(json!({"schema_version":"ATIF-v1.7","agent":{"name":"test","version":"1"},"steps":[
            {"step_id":1,"source":"agent","message":"","tool_calls":[{"tool_call_id":"a","function_name":"lookup","arguments":{"id":42}},{"tool_call_id":"b","function_name":"lookup","arguments":{"id":7}}]},
            {"step_id":2,"source":"environment","message":"","observation":{"results":[{"source_call_id":"a","content":"pending"},{"source_call_id":"b","content":"shipped"}]}}
        ]})).unwrap());
        let s = Sample::new("s", "q");
        assert!(
            !tool_invocation("lookup", "/id", json!(42), "shipped")
                .score(&s, &t)
                .await
                .pass
        );
        let score = tool_invocation("lookup", "/id", json!(42), "pending")
            .score(&s, &t)
            .await;
        assert!(score.pass && score.reason.contains("call_id=a"));
        assert!(!tool_sequence(["lookup", "lookup"]).score(&s, &t).await.pass);
        assert!(
            !tool_invocation("lookup", "/id", json!(42), "pending")
                .score(&s, &Transcript::default())
                .await
                .pass
        );
    }
    #[tokio::test]
    async fn ranking_duplicates_missing_fields_and_short_lists() {
        let s = Sample::new("s", "q").expected(json!({"relevant_ids":["a","b"]}));
        let mut t = Transcript::response("answer");
        t.metadata
            .insert("retrieved_ids".into(), json!(["a", "noise", "b"]));
        let good = retrieval_average_precision_at(3, 0.8).score(&s, &t).await;
        assert!((good.value - 5.0 / 6.0).abs() < 1e-9);
        assert_eq!(
            retrieval_precision_at(3, 0.5).score(&s, &t).await.value,
            2.0 / 3.0
        );
        assert_eq!(retrieval_recall_at(3, 1.0).score(&s, &t).await.value, 1.0);
        t.metadata
            .insert("retrieved_ids".into(), json!(["noise", "a", "b"]));
        assert!(
            retrieval_average_precision_at(3, 0.8)
                .score(&s, &t)
                .await
                .value
                < good.value
        );
        t.metadata.insert("retrieved_ids".into(), json!(["a", "a"]));
        assert!(!retrieval_precision_at(2, 0.5).score(&s, &t).await.pass);
        t.metadata.insert("retrieved_ids".into(), json!(["a"]));
        assert_eq!(
            retrieval_precision_at(4, 0.0).score(&s, &t).await.value,
            0.25
        );
        assert!(!retrieval_precision_at(0, 0.5).score(&s, &t).await.pass);
        assert!(
            !retrieval_recall_at(1, 0.5)
                .score(&Sample::new("x", "q"), &t)
                .await
                .pass
        );
    }
}
