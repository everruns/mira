//! LLM-as-judge [`Scorer`]s backed by real providers.
//!
//! Mira's core stays provider-agnostic and dependency-light (`mira-eval` carries
//! no HTTP stack). This crate is the integration layer: it wires an
//! [`LlmJudge`] onto an actual model endpoint and exposes it as an ordinary
//! [`Scorer`], so it composes with the deterministic built-ins and combinators
//! unchanged.
//!
//! Three provider transports are supported:
//!
//! * [`LlmJudge::openai_completions`] — OpenAI **Chat Completions** (`/v1/chat/completions`).
//! * [`LlmJudge::openai_responses`] — OpenAI **Responses** (`/v1/responses`).
//! * [`LlmJudge::claude`] — Anthropic **Messages** (`/v1/messages`).
//!
//! # N/A, not crash
//!
//! A judge depends on a network call, so it *will* sometimes fail for reasons
//! that have nothing to do with the subject (no API key, rate limit, 5xx,
//! timeout). In those cases the scorer returns [`Score::na`] — neither pass nor
//! fail — rather than crashing the run or scoring a spurious `fail`. A run with
//! no credentials therefore stays green: every judge case is simply N/A.
//!
//! # What the judge sees
//!
//! [`Include`] selects the surface graded: just the agent's final response, the
//! response plus its tool calls, or the full picture including operational
//! metrics (tokens, cost, latency). Pick the narrowest surface the rubric needs.
//!
//! ```no_run
//! use mira::Eval;
//! use mira::scorer::succeeded;
//! use mira_judge::{Include, LlmJudge};
//!
//! let eval = Eval::new("qa")
//!     .sample("capital", "What is the capital of France?")
//!     // ... .subject(...) ...
//!     .scorer(succeeded())
//!     .scorer(
//!         LlmJudge::claude("claude-haiku-4-5")
//!             .include(Include::Transcript)
//!             .scorer("Is the answer correct, concise, and free of tool misuse?"),
//!     );
//! ```

use async_trait::async_trait;
use mira::scorer::Scorer;
use mira::{Sample, Score, Transcript};

/// Which provider transport the judge speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// OpenAI Chat Completions (`POST /v1/chat/completions`).
    OpenAiCompletions,
    /// OpenAI Responses (`POST /v1/responses`).
    OpenAiResponses,
    /// Anthropic Messages (`POST /v1/messages`).
    Claude,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Provider::OpenAiCompletions => "openai_completions",
            Provider::OpenAiResponses => "openai_responses",
            Provider::Claude => "claude",
        }
    }
}

/// How much of the run to put in front of the judge.
///
/// Scorers grade against one of three surfaces — the agent's result, the
/// transcript (result + tool calls), or the full run including prebuilt metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Include {
    /// Only the agent's final response text.
    Response,
    /// Final response plus the ordered list of tool calls (the default).
    #[default]
    Transcript,
    /// Everything in [`Include::Transcript`] plus operational metrics: tokens,
    /// cost, latency, and iteration count.
    Full,
}

/// A configured LLM judge. Build one with a provider constructor, tune it with
/// the builder methods, then turn it into a [`Scorer`] for a specific rubric via
/// [`LlmJudge::scorer`].
pub struct LlmJudge {
    provider: Provider,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    threshold: f64,
    include: Include,
    client: reqwest::Client,
}

impl LlmJudge {
    fn from_env(provider: Provider, model: impl Into<String>, key_env: &str) -> Self {
        let api_key = std::env::var(key_env).ok().filter(|v| !v.trim().is_empty());
        Self {
            provider,
            model: model.into(),
            api_key,
            base_url: None,
            threshold: 0.5,
            include: Include::default(),
            // Bound every call so a stalled connection degrades to N/A in finite
            // time rather than hanging a run (or CI) indefinitely.
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    /// A judge over OpenAI Chat Completions. Reads `OPENAI_API_KEY`.
    pub fn openai_completions(model: impl Into<String>) -> Self {
        Self::from_env(Provider::OpenAiCompletions, model, "OPENAI_API_KEY")
    }

    /// A judge over the OpenAI Responses API. Reads `OPENAI_API_KEY`.
    pub fn openai_responses(model: impl Into<String>) -> Self {
        Self::from_env(Provider::OpenAiResponses, model, "OPENAI_API_KEY")
    }

    /// A judge over Anthropic's Messages API. Reads `ANTHROPIC_API_KEY`.
    pub fn claude(model: impl Into<String>) -> Self {
        Self::from_env(Provider::Claude, model, "ANTHROPIC_API_KEY")
    }

    /// Override the API key (otherwise read from the provider's env var).
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.api_key = (!key.trim().is_empty()).then_some(key);
        self
    }

    /// Override the API base URL (e.g. a proxy or a compatible gateway). A
    /// trailing slash is trimmed so paths join cleanly (no `//v1/...`).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into().trim_end_matches('/').to_string());
        self
    }

    /// Pass threshold on the judge's `0.0..=1.0` score (default `0.5`).
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    /// Choose how much of the run the judge grades against (default
    /// [`Include::Transcript`]).
    pub fn include(mut self, include: Include) -> Self {
        self.include = include;
        self
    }

    /// True when a usable API key is configured. A judge without a key produces
    /// N/A scores rather than failing.
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// Build a [`Scorer`] that grades transcripts against `rubric`.
    pub fn scorer(self, rubric: impl Into<String>) -> Box<dyn Scorer> {
        Box::new(LlmJudgeScorer {
            judge: self,
            rubric: rubric.into(),
        })
    }

    fn name(&self) -> String {
        format!("llm_judge:{}:{}", self.provider.label(), self.model)
    }

    fn base(&self) -> &str {
        self.base_url.as_deref().unwrap_or(match self.provider {
            Provider::Claude => "https://api.anthropic.com",
            _ => "https://api.openai.com",
        })
    }

    /// Call the configured model with `system`/`user` prompts and return the raw
    /// assistant text. Any transport, status, or shape error becomes `Err`.
    async fn call(&self, system: &str, user: &str) -> Result<String, String> {
        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| "no API key".to_string())?;
        match self.provider {
            Provider::OpenAiCompletions => self.call_openai_completions(key, system, user).await,
            Provider::OpenAiResponses => self.call_openai_responses(key, system, user).await,
            Provider::Claude => self.call_claude(key, system, user).await,
        }
    }

    async fn call_openai_completions(
        &self,
        key: &str,
        system: &str,
        user: &str,
    ) -> Result<String, String> {
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        let v = self
            .post(
                &format!("{}/v1/chat/completions", self.base()),
                body,
                |req| req.bearer_auth(key),
            )
            .await?;
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "no choices[0].message.content in response".to_string())
    }

    async fn call_openai_responses(
        &self,
        key: &str,
        system: &str,
        user: &str,
    ) -> Result<String, String> {
        // The Responses API requires the word "json" to appear in the *input*
        // messages (not `instructions`) to use the `json_object` text format, so
        // the system prompt rides along as an input message like Completions.
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "input": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "text": {"format": {"type": "json_object"}},
        });
        let v = self
            .post(&format!("{}/v1/responses", self.base()), body, |req| {
                req.bearer_auth(key)
            })
            .await?;
        // Concatenate every `output_text` block across the output items.
        let mut text = String::new();
        if let Some(items) = v["output"].as_array() {
            for item in items {
                if let Some(content) = item["content"].as_array() {
                    for c in content {
                        if c["type"] == "output_text" {
                            if let Some(t) = c["text"].as_str() {
                                text.push_str(t);
                            }
                        }
                    }
                }
            }
        }
        if text.is_empty() {
            return Err("no output_text in responses payload".to_string());
        }
        Ok(text)
    }

    async fn call_claude(&self, key: &str, system: &str, user: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "temperature": 0,
            "system": system,
            "messages": [{"role": "user", "content": user}],
        });
        let v = self
            .post(&format!("{}/v1/messages", self.base()), body, |req| {
                req.header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01")
            })
            .await?;
        let mut text = String::new();
        if let Some(blocks) = v["content"].as_array() {
            for b in blocks {
                if b["type"] == "text" {
                    if let Some(t) = b["text"].as_str() {
                        text.push_str(t);
                    }
                }
            }
        }
        if text.is_empty() {
            return Err("no text block in messages payload".to_string());
        }
        Ok(text)
    }

    /// POST `body` as JSON, apply auth headers, and parse the JSON response.
    /// Surfaces non-2xx status (with a snippet of the body) as `Err`.
    async fn post(
        &self,
        url: &str,
        body: serde_json::Value,
        auth: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<serde_json::Value, String> {
        let req = auth(self.client.post(url)).json(&body);
        let resp = req
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("reading body failed: {e}"))?;
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            return Err(format!("HTTP {status}: {snippet}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("invalid JSON response: {e}"))
    }
}

/// The judge's structured verdict, parsed out of the model's reply.
#[derive(Debug, PartialEq)]
struct Verdict {
    score: f64,
    reason: String,
}

/// The instruction shared by every provider: grade against the rubric and reply
/// with strict JSON we can parse.
const SYSTEM: &str = "You are a strict, fair evaluator grading an AI agent's run \
against a rubric. Reply with ONLY a JSON object and nothing else: \
{\"score\": <number 0.0-1.0>, \"reason\": \"<one short sentence>\"}. \
1.0 means the rubric is fully satisfied, 0.0 means it is not satisfied at all.";

/// Render the chosen [`Include`] surface of a run as the user prompt body.
fn build_context(include: Include, rubric: &str, sample: &Sample, t: &Transcript) -> String {
    let mut s = String::new();
    s.push_str("# Rubric\n");
    s.push_str(rubric);
    s.push_str("\n\n# Agent input\n");
    s.push_str(&sample.input.join("\n"));
    if let Some(expected) = sample.expected_str() {
        s.push_str("\n\n# Reference / expected answer\n");
        s.push_str(expected);
    }
    s.push_str("\n\n# Agent final response\n");
    s.push_str(&t.final_response);

    if matches!(include, Include::Transcript | Include::Full) && !t.tool_calls.is_empty() {
        s.push_str("\n\n# Tool calls (in order)\n");
        s.push_str(&t.tool_calls.join(", "));
    }
    if matches!(include, Include::Full) {
        s.push_str(&format!(
            "\n\n# Metrics\ntokens: {} (output {}); cost: ${:.4}; latency: {}ms; iterations: {}",
            t.usage.total_tokens(),
            t.usage.output_tokens,
            t.usage.cost_usd,
            t.timing.duration_ms,
            t.iterations,
        ));
    }
    if let Some(err) = &t.error {
        s.push_str("\n\n# Run error\n");
        s.push_str(err);
    }
    s
}

/// Parse the judge's reply into a [`Verdict`]. Tolerates the model wrapping the
/// JSON in prose by extracting the first balanced-looking object.
fn parse_verdict(text: &str) -> Result<Verdict, String> {
    let value: serde_json::Value = serde_json::from_str(text.trim())
        .or_else(|_| {
            extract_json(text)
                .ok_or_else(|| "no JSON object found".to_string())
                .and_then(|j| serde_json::from_str(&j).map_err(|e| e.to_string()))
        })
        .map_err(|e| format!("unparseable judge reply: {e}"))?;

    let score = value
        .get("score")
        .and_then(|s| {
            s.as_f64()
                .or_else(|| s.as_str().and_then(|s| s.parse().ok()))
        })
        .ok_or_else(|| "verdict missing numeric `score`".to_string())?;
    let reason = value
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    Ok(Verdict {
        score: score.clamp(0.0, 1.0),
        reason,
    })
}

/// Extract the substring from the first `{` to the last `}`, inclusive.
fn extract_json(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}

struct LlmJudgeScorer {
    judge: LlmJudge,
    rubric: String,
}

#[async_trait]
impl Scorer for LlmJudgeScorer {
    fn name(&self) -> String {
        self.judge.name()
    }

    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score {
        let name = self.judge.name();
        // No credentials → N/A, so a key-free run stays green instead of failing.
        if !self.judge.is_configured() {
            return Score::na(name, "no API key configured — judge skipped");
        }
        let user = build_context(self.judge.include, &self.rubric, sample, transcript);
        match self.judge.call(SYSTEM, &user).await {
            Ok(reply) => match parse_verdict(&reply) {
                Ok(v) => Score::graded(name, v.score, self.judge.threshold, v.reason),
                // The judge answered but malformed — an infra/judge problem, not
                // a subject failure, so N/A rather than a spurious fail.
                Err(e) => Score::na(name, e),
            },
            Err(e) => Score::na(name, format!("judge unavailable: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript() -> Transcript {
        let mut t = Transcript::response("Paris is the capital of France.");
        t.tool_calls = vec!["search".into(), "read".into()];
        t.usage.input_tokens = 100;
        t.usage.output_tokens = 20;
        t.usage.cost_usd = 0.002;
        t.timing.duration_ms = 850;
        t.iterations = 2;
        t
    }

    #[test]
    fn context_includes_selected_surface() {
        let s = Sample::new("capital", "What is the capital of France?").expected("Paris");
        let t = transcript();

        let response_only = build_context(Include::Response, "is it right?", &s, &t);
        assert!(response_only.contains("Paris is the capital"));
        assert!(response_only.contains("Reference / expected answer"));
        assert!(!response_only.contains("Tool calls"));
        assert!(!response_only.contains("Metrics"));

        let with_tools = build_context(Include::Transcript, "r", &s, &t);
        assert!(with_tools.contains("Tool calls"));
        assert!(with_tools.contains("search, read"));
        assert!(!with_tools.contains("Metrics"));

        let full = build_context(Include::Full, "r", &s, &t);
        assert!(full.contains("Metrics"));
        assert!(full.contains("cost: $0.0020"));
        assert!(full.contains("iterations: 2"));
    }

    #[test]
    fn parse_clean_and_wrapped_json() {
        let v = parse_verdict(r#"{"score": 0.8, "reason": "good"}"#).unwrap();
        assert_eq!(
            v,
            Verdict {
                score: 0.8,
                reason: "good".into()
            }
        );

        // Wrapped in prose / code fence.
        let v = parse_verdict("Sure!\n```json\n{\"score\": 1, \"reason\": \"ok\"}\n```").unwrap();
        assert_eq!(v.score, 1.0);

        // Score as a string, out-of-range clamps.
        let v = parse_verdict(r#"{"score": "1.5", "reason": "x"}"#).unwrap();
        assert_eq!(v.score, 1.0);
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let judge = LlmJudge::openai_completions("m").base_url("https://gw.example.com/");
        assert_eq!(judge.base(), "https://gw.example.com");
        // Default base has no trailing slash either.
        assert_eq!(LlmJudge::claude("m").base(), "https://api.anthropic.com");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_verdict("not json at all").is_err());
        assert!(parse_verdict(r#"{"reason": "no score"}"#).is_err());
    }

    #[tokio::test]
    async fn unconfigured_judge_scores_na_not_fail() {
        // Force no key regardless of the environment.
        let judge = LlmJudge::openai_completions("gpt-4o-mini").api_key("");
        assert!(!judge.is_configured());
        let scorer = judge.scorer("anything");
        let s = scorer
            .score(&Sample::new("a", "q"), &Transcript::response("hi"))
            .await;
        assert!(s.is_na());
        assert!(!s.pass);
    }

    // Integration tests hit real provider endpoints and cost money, so they are
    // `#[ignore]`d in the normal suite. CI runs them with `--ignored` after
    // injecting keys from Doppler. Without a key they no-op (the case is N/A),
    // so running them locally is always safe.
    async fn integration_smoke(judge: LlmJudge) {
        if !judge.is_configured() {
            eprintln!("skipping: no API key for {}", judge.name());
            return;
        }
        let scorer = judge.scorer("Does the response correctly name the capital of France?");
        let sample = Sample::new("capital", "What is the capital of France?").expected("Paris");
        let t = Transcript::response("The capital of France is Paris.");
        let s = scorer.score(&sample, &t).await;
        assert!(!s.is_na(), "judge returned N/A: {}", s.reason);
        assert!(s.pass, "expected pass, got {} ({})", s.value, s.reason);
    }

    #[tokio::test]
    #[ignore = "hits the OpenAI Chat Completions API; needs OPENAI_API_KEY"]
    async fn openai_completions_grades_correct_answer() {
        integration_smoke(LlmJudge::openai_completions("gpt-4o-mini")).await;
    }

    #[tokio::test]
    #[ignore = "hits the OpenAI Responses API; needs OPENAI_API_KEY"]
    async fn openai_responses_grades_correct_answer() {
        integration_smoke(LlmJudge::openai_responses("gpt-4o-mini")).await;
    }

    #[tokio::test]
    #[ignore = "hits the Anthropic Messages API; needs ANTHROPIC_API_KEY"]
    async fn claude_grades_correct_answer() {
        integration_smoke(LlmJudge::claude("claude-haiku-4-5")).await;
    }
}
