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

## Built-in scorers

| Scorer | Passes when |
|--------|-------------|
| `succeeded()` | the subject completed without an `error` |
| `contains(s)` | the final response contains `s` |
| `not_contains(s)` | the final response does not contain `s` |
| `equals(s)` | the final response equals `s` (trimmed, case-insensitive) |
| `regex(p)` | the final response matches regex `p` |
| `matches_target()` | the final response equals the sample's string `target` |
| `tool_called(t)` | a tool named `t` was invoked |
| `tool_calls_within(n)` | at most `n` tool calls were made |
| `turns_within(n)` | at most `n` reasoning iterations were taken |
| `cost_within(usd)` | total cost stayed at or under `usd` |
| `file_exists(path)` | the captured workspace has a file at `path` |
| `file_contains(path, s)` | that file exists and contains `s` |

```rust
use mira::scorer::*;

let eval = Eval::new("coding")
    .subject(/* … */)
    .scorer(succeeded())
    .scorer(tool_called("edit_file"))
    .scorer(turns_within(5))
    .scorer(cost_within(0.05))
    .scorer(file_contains("lib.rs", "fn greet"))
    .build();
```

File-based scorers read `Transcript.files`. A `CliSubject` populates that map
when built with `.capture_files()`; other subjects fill it as appropriate.

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
    let f1 = compute_f1(sample.target_str().unwrap_or(""), &t.final_response);
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
