# Scorers

A `Scorer` grades a `Transcript` for one `Sample` into a `Score`. Every scorer on
an eval runs against every cell; a cell **passes** iff all its scorers pass.

```rust
#[async_trait]
pub trait Scorer: Send + Sync {
    fn name(&self) -> String;
    async fn score(&self, sample: &Sample, transcript: &Transcript) -> Score;
}
```

A `Score` carries both a continuous `value` (`0.0..=1.0`) and a boolean `pass`,
so a scorer can report a graded signal while still contributing pass/fail to the
matrix. `aggregate` on a cell is the mean of the values.

## Three surfaces to score against

Every scorer is handed the `Sample` and the full `Transcript`, so it can grade:

- **the agent's result** — `Transcript.final_response` (e.g. `contains`, `equals`,
  `regex`, `json_valid`);
- **the transcript** — including the ordered `tool_calls`, `iterations`, `files`,
  and raw `events` (e.g. `tool_called`, `tools_used_exactly`, `tool_called_before`,
  `file_contains`);
- **prebuilt metrics** — the operational fields `Transcript.usage` (tokens, cost)
  and `Transcript.timing` (latency, TTFT) (e.g. `tokens_within`, `cost_within`,
  `latency_within`).

The provider-backed LLM judge (below) can grade any of these via `Include`.

## N/A: when a scorer can't run

A scorer that depends on infrastructure (an LLM judge, a network call) will
sometimes fail for reasons unrelated to the subject — a missing API key, a rate
limit, a 5xx. For those, return **N/A** instead of crashing the run or scoring a
misleading `fail`:

```rust
use mira::Score;
Score::na("judge", "model unreachable — skipped");
```

An N/A score is **excluded** from the cell verdict and the aggregate: it neither
passes nor fails. A cell passes iff it has at least one applicable score and all
applicable scores pass; if *every* score is N/A, the cell does not pass (nothing
was evaluated). Combinators (`all_of`/`any_of`/`not`) ignore N/A inner scores and
become N/A themselves only when all of their inputs are N/A. Reports render N/A
with a `–` glyph, and JUnit never counts it as a failure. This is how a run with
no provider credentials stays green: every judge cell is simply N/A.

## Built-in scorers

**Text & output**

| Scorer | Passes when |
|--------|-------------|
| `succeeded()` | the subject completed without an `error` |
| `non_empty()` | the final response is non-empty (trimmed) |
| `contains(s)` / `not_contains(s)` | the final response does / doesn't contain `s` |
| `equals(s)` | the final response equals `s` (trimmed, case-insensitive) |
| `regex(p)` | the final response matches regex `p` |
| `matches_expected()` | the final response equals the sample's string `expected` |
| `json_valid()` | the final response parses as JSON |
| `json_field_equals(k, v)` | the response is a JSON object with top-level `k == v` |

**Tools**

| Scorer | Passes when |
|--------|-------------|
| `tool_called(t)` | a tool named `t` was invoked |
| `tool_not_called(t)` | a tool named `t` was never invoked |
| `tool_calls_within(n)` | at most `n` tool calls were made |
| `tools_used_exactly([…])` | exactly that set of tools was used (order-independent) |
| `tool_called_before(a, b)` | tool `a` was invoked before tool `b` |

**Operational budgets** (tokens, cost, latency)

| Scorer | Passes when |
|--------|-------------|
| `tokens_within(n)` | total (input+output) tokens ≤ `n` |
| `output_tokens_within(n)` | completion tokens ≤ `n` |
| `cost_within(usd)` | total cost ≤ `usd` |
| `turns_within(n)` | at most `n` reasoning iterations |
| `latency_within(ms)` | wall-clock duration ≤ `ms` |
| `ttft_within(ms)` | time-to-first-token ≤ `ms` (fails if unmeasured) |
| `metric_within(name, max)` | custom metric `name` ≤ `max` (fails if unreported) |
| `metric_at_least(name, min)` | custom metric `name` ≥ `min` (fails if unreported) |

**Files**

| Scorer | Passes when |
|--------|-------------|
| `file_exists(path)` | the captured workspace has a file at `path` |
| `file_contains(path, s)` | that file exists and contains `s` |

**Combinators** — compose scorers without a new type:

| Scorer | Passes when |
|--------|-------------|
| `all_of(name, [..])` | every inner scorer passes (value = mean) |
| `any_of(name, [..])` | any inner scorer passes (value = max) |
| `not(scorer)` | the inner scorer fails |

```rust
use mira::scorer::*;

let eval = Eval::new("coding")
    .subject(/* … */)
    .scorer(succeeded())
    .scorer(tool_called("edit_file"))
    .scorer(tools_used_exactly(["read_file", "edit_file"]))
    .scorer(file_contains("lib.rs", "fn greet"))
    // One "within SLA" verdict from several budgets.
    .scorer(all_of("within_sla", vec![
        latency_within(2_000),
        cost_within(0.05),
        tokens_within(4_000),
    ]))
    .build();
```

Operational scorers read `Transcript.usage` (tokens incl. `cache_read` /
`reasoning`, and `cost_usd`) and `Transcript.timing` (`duration_ms`,
`time_to_first_token_ms`). Subjects populate what they can measure — `CliSubject`
and `RuntimeSubject` time the run automatically, and the JSONL event walker
totals usage from a transcript stream. File-based scorers read `Transcript.files`
(a `CliSubject` fills it with `.capture_files()`).

`metric_within`/`metric_at_least` read the open `Transcript.metrics` map, so a
subject can report any numeric metric the core doesn't model as a typed field
(recall@k, energy_joules, p95 latency, …) and grade it without a new scorer type.
See [Metrics](metrics.md) for the full model and how to add your own.

## Closures: the escape hatch

The bar for bespoke logic is a closure, not a new type:

```rust
use mira::{Score, scorer::scorer};

let nonempty = scorer("nonempty", |_sample, t| {
    if t.final_response.trim().is_empty() {
        Score::fail("nonempty", "empty response")
    } else {
        Score::pass("nonempty", "ok")
    }
});
```

Use `Score::graded(name, value, threshold, reason)` to emit a partial score:

```rust
let overlap = scorer("token_f1", |sample, t| {
    let f1 = compute_f1(sample.expected_str().unwrap_or(""), &t.final_response);
    Score::graded("token_f1", f1, 0.6, format!("F1={f1:.2}"))
});
```

## LLM-as-judge

`model_graded(rubric, judge)` shows that LLM-as-judge is just another scorer —
not a special case in the engine. The judge is an async closure backed by a
(typically cheaper) model, kept independent of the model under test.

```rust
use mira::{Score, scorer::{model_graded, JudgeFn}};

let judge: JudgeFn = Box::new(|rubric, transcript| {
    Box::pin(async move {
        // Call your judge model here; return a Score.
        let passed = call_judge(&rubric, &transcript.final_response).await;
        Score::graded("judge", if passed { 1.0 } else { 0.0 }, 0.5, rubric)
    })
});

let eval = Eval::new("qa")
    .subject(/* … */)
    .scorer(model_graded("Is the answer correct and well-cited?", judge))
    .build();
```

### Provider-backed judges (`mira-judge`)

`model_graded` is the bare mechanism — you bring the model call. The
**`mira-judge`** crate is the batteries-included integration: an `LlmJudge`
wired to a real endpoint, exposed as an ordinary `Scorer`. Three transports
ship today:

| Constructor | Provider / endpoint | Key |
|-------------|---------------------|-----|
| `LlmJudge::openai_completions(model)` | OpenAI Chat Completions (`/v1/chat/completions`) | `OPENAI_API_KEY` |
| `LlmJudge::openai_responses(model)` | OpenAI Responses (`/v1/responses`) | `OPENAI_API_KEY` |
| `LlmJudge::claude(model)` | Anthropic Messages (`/v1/messages`) | `ANTHROPIC_API_KEY` |

```rust
use mira::scorer::succeeded;
use mira_judge::{Include, LlmJudge};

let eval = Eval::new("qa")
    .subject(/* … */)
    .scorer(succeeded())
    .scorer(
        LlmJudge::claude("claude-haiku-4-5")
            .include(Include::Transcript) // Response | Transcript | Full
            .threshold(0.7)
            .scorer("Is the answer correct, concise, and free of tool misuse?"),
    )
    .build();
```

- **`Include`** selects the surface the judge sees: `Response` (final answer
  only), `Transcript` (answer + tool calls, the default), or `Full` (also
  tokens/cost/latency). This is how a judge grades the result, the transcript,
  or the metrics.
- **Infra-safe by construction.** No API key, a non-2xx, a transport error, or
  an unparseable reply all yield **N/A** — never a crash or a spurious fail. A
  key-free run stays green.
- The judge keeps its model independent of the model under test, requests
  deterministic JSON (`temperature: 0`, JSON output mode), and tolerates replies
  wrapped in prose or code fences.

Integration tests that hit the live APIs are `#[ignore]`d (they cost money); CI
runs them with `--ignored` after injecting keys from Doppler.

## Custom scorer types

For reusable scorers with state, implement the trait directly:

```rust
use async_trait::async_trait;
use mira::{Sample, Score, Transcript, scorer::Scorer};

struct MinLength(usize);

#[async_trait]
impl Scorer for MinLength {
    fn name(&self) -> String { format!("min_length({})", self.0) }
    async fn score(&self, _: &Sample, t: &Transcript) -> Score {
        let n = t.final_response.len();
        if n >= self.0 {
            Score::pass("min_length", format!("{n} >= {}", self.0))
        } else {
            Score::fail("min_length", format!("{n} < {}", self.0))
        }
    }
}

// .scorer(Box::new(MinLength(20)))
```
