//! Grading resources belong to scores, never to subject usage. Unknown token
//! counts or prices remain unknown; zero means explicitly measured free work.
use serde::{Deserialize, Serialize};

/// Resources consumed by a scorer, including unsuccessful grading requests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GradingUsage {
    /// Number of outbound grading requests.
    pub requests: u64,
    /// Requests that failed transport, HTTP, response-shape, or verdict parsing.
    pub errors: u64,
    /// Measured scorer wall time.
    pub duration_ms: u64,
    /// Provider-reported input tokens; absent when unavailable.
    pub input_tokens: Option<u64>,
    /// Provider-reported output tokens; absent when unavailable.
    pub output_tokens: Option<u64>,
    /// Cost using explicit configured prices; absent when unpriced/unmeasured.
    pub cost_usd: Option<f64>,
}

impl GradingUsage {
    /// Combine every child scorer, preserving unknown totals conservatively.
    pub fn combine(&mut self, other: &Self) {
        self.requests += other.requests;
        self.errors += other.errors;
        self.duration_ms += other.duration_ms;
        self.input_tokens = self
            .input_tokens
            .zip(other.input_tokens)
            .map(|(a, b)| a + b);
        self.output_tokens = self
            .output_tokens
            .zip(other.output_tokens)
            .map(|(a, b)| a + b);
        self.cost_usd = self.cost_usd.zip(other.cost_usd).map(|(a, b)| a + b);
    }
}

/// Summarize reported grading work; absent score usage is counted explicitly.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    /// Number of scores reporting grading usage.
    pub reported_scores: usize,
    /// Number of scores with no grading usage declaration.
    pub unreported_scores: usize,
    /// Total resources over reported scores; unknown prices remain unknown.
    pub usage: Option<GradingUsage>,
}
impl Summary {
    /// Fold score usage without counting target usage or inventing missing data.
    pub fn of<'a>(scores: impl IntoIterator<Item = &'a crate::Score>) -> Self {
        let mut out = Self::default();
        for score in scores {
            if let Some(usage) = &score.grading {
                out.reported_scores += 1;
                match &mut out.usage {
                    Some(total) => total.combine(usage),
                    None => out.usage = Some(usage.clone()),
                }
            } else {
                out.unreported_scores += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rust_combinators_preserve_usage_on_na_and_unknown_children() {
        use crate::scorer::*;
        let child = || {
            scorer("judge", |_, _| {
                crate::Score::na("judge", "invalid").with_grading(GradingUsage {
                    requests: 1,
                    errors: 1,
                    cost_usd: Some(0.1),
                    ..Default::default()
                })
            })
        };
        let sample = crate::Sample::new("s", "q");
        let transcript = crate::Transcript::response("a");
        let score = all_of("combined", vec![child()])
            .score(&sample, &transcript)
            .await;
        assert!(score.na);
        assert_eq!(score.grading.unwrap().cost_usd, Some(0.1));
        let score = not(child()).score(&sample, &transcript).await;
        assert_eq!(score.grading.unwrap().errors, 1);
        let score = all_of("combined", vec![child(), succeeded()])
            .score(&sample, &transcript)
            .await;
        assert_eq!(score.grading.unwrap().cost_usd, None);
    }
    #[test]
    fn unknown_prices_and_unreported_scores_remain_visible() {
        let scores = [
            crate::Score::pass("local", "ok"),
            crate::Score::na("judge", "malformed").with_grading(GradingUsage {
                requests: 1,
                errors: 1,
                cost_usd: Some(0.2),
                ..Default::default()
            }),
            crate::Score::pass("judge2", "ok").with_grading(GradingUsage {
                requests: 1,
                ..Default::default()
            }),
        ];
        let total = Summary::of(&scores);
        assert_eq!((total.reported_scores, total.unreported_scores), (2, 1));
        let usage = total.usage.unwrap();
        assert_eq!(usage.requests, 2);
        assert_eq!(usage.errors, 1);
        assert_eq!(usage.cost_usd, None);
    }
}
