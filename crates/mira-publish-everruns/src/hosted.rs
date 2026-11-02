//! A Mira [`Subject`] that runs each sample against an agent hosted on
//! Everruns, through its Agent Execution API.
//!
//! Design notes:
//! - **The Agent Execution API, not management.** The subject talks to the
//!   agent's own base URL (`…/v1/channels/{channel_id}` on a server, an agent
//!   route on `everruns-serve`) with an agent key or a personal access token,
//!   the same contract an application calling the agent uses. It never reads
//!   or changes agent configuration.
//! - **One session per sample.** A sample's turns are sent in order on a fresh
//!   session, each after the previous turn finished, so no state leaks between
//!   samples or trials.
//! - **Polling, not streaming.** After each message the subject pages the
//!   session's events until the turn completes. That keeps the subject to one
//!   HTTP client and no reconnect logic, at the cost of up to one poll interval
//!   of extra latency per turn, which an eval does not notice.
//! - **The agent's model is the agent's.** The Execution API cannot override
//!   it. A model matrix maps each target label to an agent configured with
//!   that model ([`HostedAgent::route`]).
//! - **Who is at fault.** A turn the agent failed is a subject failure, scored
//!   like a wrong answer. A refused request, a dropped connection or a turn
//!   that outlives the timeout is an infrastructure error: scored N/A and
//!   retried by the host.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mira::{RunCx, Sample, Subject, Transcript, Usage};
use serde_json::{Value, json};

/// Events fetched per page.
const PAGE: usize = 100;

/// Default wait for one turn.
const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(300);

/// Default pause between event polls.
const DEFAULT_POLL: Duration = Duration::from_millis(500);

/// A deployed Everruns agent as a Mira subject.
///
/// ```no_run
/// use mira::{Eval, scorer::contains};
/// use mira_publish_everruns::HostedAgent;
///
/// // EVERRUNS_AGENT_URL and EVERRUNS_AGENT_KEY, as the Everruns SDKs read them.
/// let agent = HostedAgent::from_env().expect("agent URL and key");
/// let _eval = Eval::new("support")
///     .sample("refund", "Refund $450 for order 1182")
///     .subject(agent)
///     .scorer(contains("approval"));
/// ```
#[derive(Clone, Debug)]
pub struct HostedAgent {
    agent_url: String,
    credential: String,
    routes: BTreeMap<String, String>,
    turn_timeout: Duration,
    poll: Duration,
    client: reqwest::Client,
}

impl HostedAgent {
    /// An agent at `agent_url` (its Execution API base URL), authenticated
    /// with an agent key (`evr_ak_…`) or a personal access token.
    pub fn new(agent_url: impl Into<String>, credential: impl Into<String>) -> Self {
        Self {
            agent_url: trim_url(agent_url.into()),
            credential: credential.into(),
            routes: BTreeMap::new(),
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            poll: DEFAULT_POLL,
            client: reqwest::Client::new(),
        }
    }

    /// From `EVERRUNS_AGENT_URL` and `EVERRUNS_AGENT_KEY`, the variables the
    /// Everruns SDKs' agent clients read.
    pub fn from_env() -> anyhow::Result<Self> {
        let url = std::env::var("EVERRUNS_AGENT_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("EVERRUNS_AGENT_URL is not set"))?;
        let key = std::env::var("EVERRUNS_AGENT_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("EVERRUNS_AGENT_KEY is not set"))?;
        Ok(Self::new(url, key))
    }

    /// Run the target labelled `target` against the agent at `agent_url`
    /// instead, for a matrix of agents that differ in model or setup. Targets
    /// without a route use the default agent.
    pub fn route(mut self, target: impl Into<String>, agent_url: impl Into<String>) -> Self {
        self.routes
            .insert(target.into(), trim_url(agent_url.into()));
        self
    }

    /// Longest wait for one turn (default five minutes).
    pub fn turn_timeout(mut self, timeout: Duration) -> Self {
        self.turn_timeout = timeout;
        self
    }

    /// Pause between event polls (default half a second).
    pub fn poll_interval(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn base_for(&self, cx: &RunCx) -> &str {
        self.routes
            .get(&cx.target.label)
            .map(String::as_str)
            .unwrap_or(&self.agent_url)
    }

    async fn post(&self, url: String, body: Value) -> Result<Value, String> {
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.credential)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("POST {url}: {e}"))?;
        read_json(resp, &url).await
    }

    async fn get(&self, url: String) -> Result<Value, String> {
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.credential)
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        read_json(resp, &url).await
    }

    /// Send one turn and wait for it to finish. Returns its events.
    async fn turn(
        &self,
        base: &str,
        session_id: &str,
        text: &str,
        after: &mut i64,
    ) -> Result<(Vec<Value>, TurnEnd), String> {
        self.post(
            format!("{base}/sessions/{session_id}/messages"),
            json!({"message": {"role": "user", "content": [{"type": "text", "text": text}]}}),
        )
        .await?;
        let deadline = Instant::now() + self.turn_timeout;
        let mut events = Vec::new();
        loop {
            let page = self
                .get(format!(
                    "{base}/sessions/{session_id}/events?after_sequence={after}&limit={PAGE}"
                ))
                .await?;
            let batch = page
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let full = batch.len() >= PAGE;
            for event in batch {
                if let Some(seq) = event.get("sequence").and_then(Value::as_i64) {
                    *after = (*after).max(seq);
                }
                let end = turn_end(&event);
                events.push(event);
                if let Some(end) = end {
                    return Ok((events, end));
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the turn did not finish within {}s",
                    self.turn_timeout.as_secs()
                ));
            }
            if !full {
                tokio::time::sleep(self.poll).await;
            }
        }
    }
}

#[async_trait]
impl Subject for HostedAgent {
    async fn run(&self, sample: &Sample, cx: &RunCx) -> Transcript {
        let started = Instant::now();
        let base = self.base_for(cx).to_string();
        let session = match self
            .post(
                format!("{base}/sessions"),
                json!({"title": format!("mira: {}", sample.id)}),
            )
            .await
        {
            Ok(session) => session,
            Err(e) => return Transcript::infra_error(e),
        };
        let Some(session_id) = session
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Transcript::infra_error("the session response carried no id");
        };

        let mut after = 0i64;
        let mut events = Vec::new();
        let mut failure = None;
        for text in &sample.input {
            match self.turn(&base, &session_id, text, &mut after).await {
                Ok((turn_events, end)) => {
                    events.extend(turn_events);
                    if let TurnEnd::Failed(reason) = end {
                        failure = Some(reason);
                        break;
                    }
                }
                Err(e) => {
                    let mut transcript = fold(events);
                    transcript.error = Some(e);
                    transcript.error_kind = mira::ErrorKind::Infra;
                    stamp(&mut transcript, &base, &session_id, started);
                    return transcript;
                }
            }
        }

        let mut transcript = fold(events);
        transcript.error = failure;
        stamp(&mut transcript, &base, &session_id, started);
        transcript
    }
}

/// How a turn ended.
#[derive(Debug, PartialEq)]
enum TurnEnd {
    Completed,
    Failed(String),
}

fn turn_end(event: &Value) -> Option<TurnEnd> {
    let data = &event["data"];
    match event.get("type").and_then(Value::as_str)? {
        "turn.completed" => Some(TurnEnd::Completed),
        "turn.failed" => Some(TurnEnd::Failed(
            data.get("error")
                .and_then(Value::as_str)
                .unwrap_or("the turn failed")
                .to_string(),
        )),
        "turn.cancelled" => Some(TurnEnd::Failed("the turn was cancelled".into())),
        _ => None,
    }
}

/// Fold an Execution API event stream into a transcript: the last assistant
/// message, the tools the agent called, and the model calls' usage.
fn fold(events: Vec<Value>) -> Transcript {
    let mut transcript = Transcript::default();
    let mut usage = Usage::default();
    for event in &events {
        let data = &event["data"];
        match event.get("type").and_then(Value::as_str) {
            Some("output.message.completed") => {
                let text = message_text(&data["message"]);
                if !text.is_empty() {
                    transcript.final_response = text;
                }
            }
            Some("tool.completed") => {
                if let Some(name) = data.get("tool_name").and_then(Value::as_str) {
                    transcript.tool_calls.push(name.to_string());
                }
            }
            Some("llm.generation") => {
                transcript.iterations += 1;
                let metadata = &data["metadata"];
                let count = |key: &str| metadata["usage"][key].as_u64().unwrap_or(0);
                usage.input_tokens += count("input_tokens");
                usage.output_tokens += count("output_tokens");
                usage.cache_read_tokens += count("cache_read_tokens");
                usage.reasoning_tokens += count("reasoning_tokens");
                usage.cost_usd += metadata["cost_usd"]
                    .as_f64()
                    .or_else(|| metadata["usage"]["cost_usd"].as_f64())
                    .unwrap_or(0.0);
            }
            _ => {}
        }
    }
    transcript.tool_calls_count = transcript.tool_calls.len();
    transcript.usage = usage;
    transcript.events = events;
    transcript
}

fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn stamp(transcript: &mut Transcript, base: &str, session_id: &str, started: Instant) {
    transcript.timing.duration_ms = started.elapsed().as_millis() as u64;
    transcript
        .metadata
        .insert("everruns_agent_url".into(), json!(base));
    transcript
        .metadata
        .insert("everruns_session_id".into(), json!(session_id));
}

async fn read_json(resp: reqwest::Response, url: &str) -> Result<Value, String> {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail: String = body.chars().take(300).collect();
        return Err(format!("{url} answered {status}: {detail}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("{url}: unreadable response: {e}"))
}

fn trim_url(url: String) -> String {
    url.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_the_reply_tools_and_usage() {
        let events = vec![
            json!({"type": "llm.generation", "data": {"metadata": {"usage": {"input_tokens": 100, "output_tokens": 20}, "cost_usd": 0.01}}}),
            json!({"type": "tool.completed", "data": {"tool_name": "lookup_order", "success": true}}),
            json!({"type": "llm.generation", "data": {"metadata": {"usage": {"input_tokens": 150, "output_tokens": 30, "cache_read_tokens": 90}}}}),
            json!({"type": "output.message.completed", "data": {"message": {"role": "agent", "content": [{"type": "text", "text": "Sent for "}, {"type": "text", "text": "approval."}]}}}),
            json!({"type": "turn.completed", "data": {}}),
        ];
        let t = fold(events);
        assert_eq!(t.final_response, "Sent for approval.");
        assert_eq!(t.tool_calls, vec!["lookup_order"]);
        assert_eq!(t.tool_calls_count, 1);
        assert_eq!(t.iterations, 2);
        assert_eq!(t.usage.input_tokens, 250);
        assert_eq!(t.usage.output_tokens, 50);
        assert_eq!(t.usage.cache_read_tokens, 90);
        assert!((t.usage.cost_usd - 0.01).abs() < 1e-9);
        assert_eq!(t.events.len(), 5);
    }

    #[test]
    fn reads_how_a_turn_ended() {
        assert_eq!(
            turn_end(&json!({"type": "turn.completed"})),
            Some(TurnEnd::Completed)
        );
        assert_eq!(
            turn_end(&json!({"type": "turn.failed", "data": {"error": "model refused"}})),
            Some(TurnEnd::Failed("model refused".into()))
        );
        assert!(matches!(
            turn_end(&json!({"type": "turn.cancelled", "data": {}})),
            Some(TurnEnd::Failed(_))
        ));
        assert_eq!(turn_end(&json!({"type": "tool.started"})), None);
    }

    /// Serves canned answers in order, one connection per request, and
    /// returns the request lines it saw.
    async fn serve(
        answers: Vec<(&'static str, Value)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/channels/chan_1",
            listener.local_addr().unwrap()
        );
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in answers {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                seen.push(text.lines().next().unwrap_or_default().to_string());
                assert!(text.contains("authorization: Bearer evr_ak_test"));
                let body = body.to_string();
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
            seen
        });
        (url, handle)
    }

    fn cx() -> RunCx {
        RunCx {
            target: mira::Target::sim(),
            max_turns: 10,
            params: Default::default(),
            trial: mira::Trial::single(),
            conversation: Vec::new(),
        }
    }

    #[tokio::test]
    async fn runs_each_turn_on_one_session_and_waits_for_it() {
        let (url, server) = serve(vec![
            ("201 Created", json!({"id": "session_1"})),
            ("200 OK", json!({"id": "msg_1"})),
            // The first poll finds the turn still running.
            ("200 OK", json!({"data": [{"type": "turn.started", "sequence": 1, "data": {}}]})),
            ("200 OK", json!({"data": [
                {"type": "output.message.completed", "sequence": 2, "data": {"message": {"content": [{"type": "text", "text": "Hello"}]}}},
                {"type": "turn.completed", "sequence": 3, "data": {}}
            ]})),
            ("200 OK", json!({"id": "msg_2"})),
            ("200 OK", json!({"data": [
                {"type": "output.message.completed", "sequence": 4, "data": {"message": {"content": [{"type": "text", "text": "42"}]}}},
                {"type": "turn.completed", "sequence": 5, "data": {}}
            ]})),
        ])
        .await;
        let agent = HostedAgent::new(url, "evr_ak_test").poll_interval(Duration::from_millis(5));
        let sample = Sample::turns("greet", ["Hi", "The answer?"]);
        let t = agent.run(&sample, &cx()).await;
        assert_eq!(t.error, None);
        assert_eq!(t.final_response, "42");
        assert_eq!(t.metadata["everruns_session_id"], "session_1");
        let seen = server.await.unwrap();
        assert!(seen[0].starts_with("POST /v1/channels/chan_1/sessions "));
        assert!(seen[3].contains("after_sequence=1"));
        // The second turn reads only what came after the first.
        assert!(seen[5].contains("after_sequence=3"));
    }

    #[tokio::test]
    async fn a_refused_request_is_an_infrastructure_error() {
        let (url, _server) = serve(vec![("401 Unauthorized", json!({"error": "bad key"}))]).await;
        let t = HostedAgent::new(url, "evr_ak_test")
            .run(&Sample::new("x", "hi"), &cx())
            .await;
        assert_eq!(t.error_kind, mira::ErrorKind::Infra);
        assert!(t.error.unwrap().contains("401"));
    }

    #[tokio::test]
    async fn a_failed_turn_counts_against_the_agent() {
        let (url, _server) = serve(vec![
            ("201 Created", json!({"id": "session_1"})),
            ("200 OK", json!({})),
            ("200 OK", json!({"data": [{"type": "turn.failed", "sequence": 1, "data": {"error": "tool crashed"}}]})),
        ])
        .await;
        let t = HostedAgent::new(url, "evr_ak_test")
            .run(&Sample::new("x", "hi"), &cx())
            .await;
        assert_eq!(t.error.as_deref(), Some("tool crashed"));
        assert_eq!(t.error_kind, mira::ErrorKind::Subject);
    }

    #[test]
    fn routes_targets_to_their_agents() {
        let agent = HostedAgent::new("https://x/api/v1/channels/a/", "k")
            .route("openai/gpt-6.1-sol", "https://x/api/v1/channels/b");
        assert_eq!(agent.agent_url, "https://x/api/v1/channels/a");
        assert_eq!(
            agent.routes.get("openai/gpt-6.1-sol").map(String::as_str),
            Some("https://x/api/v1/channels/b")
        );
    }
}
