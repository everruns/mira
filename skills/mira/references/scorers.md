# Scorer catalog

A scorer grades a `Transcript` and returns a `Score` (pass / fail / N/A). Add
them with `.scorer(...)`; a cell passes only if every scorer passes. Import from
`mira::scorer`. Canonical prose + semantics:
<https://github.com/everruns/mira/blob/main/docs/scorers.md>.

## Text / output

| Scorer | Passes when |
|--------|-------------|
| `succeeded()` | The run completed without an error result. |
| `non_empty()` | `final_response` is non-empty. |
| `contains(s)` | Output contains substring `s`. |
| `not_contains(s)` | Output does not contain `s`. |
| `equals(s)` | Output equals `s` exactly. |
| `regex(pat)` | Output matches the regex `pat`. |
| `matches_expected()` | Output matches the sample's `expected` field. |
| `json_valid()` | Output parses as JSON. |
| `json_field_equals(path, v)` | JSON field at `path` equals `v`. |

## Tools

| Scorer | Passes when |
|--------|-------------|
| `tool_called(name)` | The subject called tool `name`. |
| `tool_not_called(name)` | The subject never called `name`. |
| `tool_calls_within(n)` | Total tool calls ≤ `n`. |
| `tools_used_exactly([...])` | The exact set of tools was used. |
| `tool_called_before(a, b)` | Tool `a` was called before tool `b`. |

## Budgets (operational)

| Scorer | Passes when |
|--------|-------------|
| `tokens_within(n)` | Total tokens ≤ `n`. |
| `output_tokens_within(n)` | Output tokens ≤ `n`. |
| `cost_within(usd)` | Estimated cost ≤ `usd`. |
| `turns_within(n)` | Turn count ≤ `n`. |
| `latency_within(ms)` | Wall-clock duration ≤ `ms`. |
| `ttft_within(ms)` | Time-to-first-token ≤ `ms`. |

Budget scorers grade the metrics the subject reports — see
<https://github.com/everruns/mira/blob/main/docs/metrics.md>.

## Files

| Scorer | Passes when |
|--------|-------------|
| `file_exists(path)` | `Transcript.files` contains `path`. |
| `file_contains(path, s)` | That file's contents contain `s`. |

## Combinators / custom

| Scorer | Meaning |
|--------|---------|
| `all_of([...])` | All inner scorers pass. |
| `any_of([...])` | At least one inner scorer passes. |
| `not(scorer)` | Inverts an inner scorer. |
| `scorer(name, closure)` | Arbitrary `Fn(&Sample, &Transcript) -> Score`. |
| `model_graded(rubric, judge)` | LLM-as-judge against a rubric; N/A without a key. |

### Closure escape hatch

```rust
use mira::{Score, scorer::scorer};
let s = scorer("nonempty", |_, t| {
    if t.final_response.trim().is_empty() { Score::fail("nonempty", "empty") }
    else { Score::pass("nonempty", "ok") }
});
```

### LLM-as-judge

`model_graded` (and the provider-backed judges in `mira-judge`) score against a
rubric. The judge is **N/A** when its API key is missing, so suites stay green
offline. See the `llm_judge` example:
<https://github.com/everruns/mira/tree/main/examples/llm_judge>.
