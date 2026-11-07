//! Live judge tests against the real provider APIs.
//!
//! They cost money, so they are a separate test target behind the
//! `live-tests` feature: `cargo test` never builds them, and the weekly
//! `Live judges` workflow runs them with keys from Doppler:
//!
//! ```bash
//! cargo test -p mira-judge --features live-tests --test live_judges
//! ```
//!
//! A missing key fails the test. Skipping instead would report green while
//! checking nothing.

use mira::{Sample, Transcript};
use mira_judge::LlmJudge;

async fn grades_correct_answer(judge: LlmJudge, key_var: &str) {
    assert!(
        judge.is_configured(),
        "{key_var} is not set; live judge tests need a real key"
    );
    let scorer = judge.scorer("Does the response correctly name the capital of France?");
    let sample = Sample::new("capital", "What is the capital of France?").expected("Paris");
    let t = Transcript::response("The capital of France is Paris.");
    let s = scorer.score(&sample, &t).await;
    assert!(!s.is_na(), "judge returned N/A: {}", s.reason);
    assert!(s.pass, "expected pass, got {} ({})", s.value, s.reason);
}

#[tokio::test]
async fn openai_completions_grades_correct_answer() {
    grades_correct_answer(LlmJudge::openai_completions("gpt-6-luna"), "OPENAI_API_KEY").await;
}

#[tokio::test]
async fn openai_responses_grades_correct_answer() {
    grades_correct_answer(LlmJudge::openai_responses("gpt-6-luna"), "OPENAI_API_KEY").await;
}

#[tokio::test]
async fn claude_grades_correct_answer() {
    grades_correct_answer(LlmJudge::claude("claude-haiku-5-5"), "ANTHROPIC_API_KEY").await;
}
