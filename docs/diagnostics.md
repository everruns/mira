# Diagnose and accept an evaluation

Mira separates case scoring, report acceptance, and test-category coverage. A
scorer explains one execution. An acceptance policy decides whether a saved run
meets your release criteria. A coverage inventory shows which declared scenarios
and quality dimensions your cases exercise.

## Grade specific evidence

The diagnostic scorers are available in Rust, Python, and TypeScript, with shared
conformance tests. Rust and Python use the names below; TypeScript uses camelCase.

| Scorer | Evidence and semantics |
|---|---|
| `tool_invocation(tool, pointer, expected, observation)` | One ATIF invocation must match the JSON Pointer argument and its correlated observation substring. Observations may be in a later step. The reason identifies the matching step and call. |
| `tool_sequence(tools)` | An ordered subsequence across distinct ATIF steps. Other calls may intervene. Calls in one step do not establish causal order. An empty sequence fails. |
| `retrieval_precision_at(k, threshold)` | Relevant IDs in the first `k` ranks divided by `k`. Missing ranks count as misses. |
| `retrieval_recall_at(k, threshold)` | Relevant IDs in the first `k` ranks divided by the number of reference IDs. |
| `retrieval_average_precision_at(k, threshold)` | Sum of precision at relevant ranks divided by `min(k, reference count)`. Moving relevant results down lowers the score. |

Retrieval scorers read ranked string IDs from
`Transcript.metadata["retrieved_ids"]` and reference string IDs from
`Sample.expected["relevant_ids"]`. Both arrays must contain unique, nonempty
strings, and the reference array must not be empty. Empty retrieved results score
zero. Missing or malformed evidence fails visibly; it does not become N/A. `k`
must be positive and the threshold must be finite and in `0..1`.

```rust
use mira::{Sample, Transcript};
use mira::scorer::{retrieval_average_precision_at, tool_invocation};
use serde_json::json;

let sample = Sample::new("docs", "Find the release docs.")
    .expected(json!({"relevant_ids": ["a", "b"]}));
let mut transcript = Transcript::response("Release 42 shipped.");
transcript.metadata.insert("retrieved_ids".into(), json!(["a", "noise", "b"]));
let ranking = retrieval_average_precision_at(3, 0.8);
let order = tool_invocation("lookup_order", "/id", json!(42), "shipped");
```

Keep retrieval quality, factual grounding, conversation retention, and style as
separate scorers. `mira_judge::LlmJudge::groundedness()` supplies a rubric for
answer claims supported by captured tool observations;
`conversation_retention()` supplies a rubric for retaining applicable facts and
constraints from earlier input turns. Both include transcript evidence. These are
model judgments: calibrate them on reviewed, held-out labels before gating a
release. They do not prove correctness through string matching.

## Apply absolute acceptance policies

Run `mira assess RUN --policy policy.toml --out assessment.json` on a finished
saved run. The command accepts a run ID or a run directory. It never launches the
study or changes the source run. The JSON includes the exact policy and actual
measurements; version the TOML alongside your study.

```toml
[[rules]]
name = "safety"
tag = "safety"
min_cases = 10
min_pass_rate = 1.0

[[rules]]
name = "completion"
filter = "tasks/*"
scorer = "task-complete"
min_pass_rate = 0.95
max_p95_latency_ms = 30000
max_subject_cost_usd = 2.0

[[rules]]
name = "judge-spend"
scorer = "llm_judge:*"
max_grading_cost_usd = 0.50
```

Every rule must pass. `filter` is a glob over the full trial-aware case key;
`tag` selects an archived sample tag. If both are specified, both must match.
`scorer` selects score names by glob and must match at least one available score
on every selected execution. Scope it narrowly when grading dimensions differ.
`min_score` optionally gates the mean selected score. Rule names must be unique
and nonempty, and each rule must specify at least one acceptance limit.

Pass rates and score means give each logical case equal weight, keeping its
trials together. Every selected logical case must have its complete trial set,
with available, finite scores. `min_cases` defaults to one; empty slices cannot
pass. P95 latency uses the nearest-rank method over executed trials. Costs sum
across selected trials. Grading cost considers the selected scorers only, and
unreported or unpriced grading makes a cost gate incomplete.

Exit codes are `0` for accepted, `1` for an acceptance breach, and `2` for
incomplete evidence or missing coverage categories. A malformed policy is a CLI
error. `mira compare` continues to handle relative baseline/candidate regression
gates; `assess` expresses absolute acceptance criteria.

## Account for grading separately

Each `Score` can carry optional `grading` usage: request/error counts, wall time,
input/output tokens, and estimated USD cost. `mira-judge` records all attempted
requests, including malformed verdicts. An unconfigured judge records zero
requests. Transport errors preserve the attempted-request count while leaving
unknown token and cost totals absent.

```rust
use mira_judge::{LlmJudge, Pricing};

let judge = LlmJudge::openai_responses("your-pinned-judge-model")
    .pricing(Pricing::new(1.0, 4.0).expect("valid effective rates"));
let grounding = judge.groundedness();
```

Prices above are illustrative, explicit flat effective rates per million tokens.
Configure rates for your provider contract, including expected cache discounts.
Mira does not fetch prices or claim invoice-level billing accuracy. Provider token
usage remains available without prices; missing prices remain unknown. Anthropic
cached-input counts are included in total input tokens.

Custom SDK scorers can attach the generated `GradingUsage` type to their score.
Combinators retain reported resources, including failed children; any unreported
child leaves token/cost totals unknown. JSON reports include a separate `grading`
summary and per-score usage. HTML reports and the evidence reviewer display
grading separately from subject cost. Other report formats retain their existing
presentation; JSON/JSONL are the detailed resource export.

Acceptance spend limits are post-run gates. They do not cancel in-flight calls or
prevent spending. Use provider quotas and subject runtime limits to bound live
execution; use these reports to measure and gate the resulting bill.

## Declare your coverage inventory

Add category metadata to each sample:

```json
{"metadata": {"scenario": "order-support", "dimensions": ["correctness", "safety"]}}
```

Then declare the expected cross product in the acceptance policy:

```toml
[inventory]
scenarios = ["order-support", "document-search"]
dimensions = ["correctness", "safety", "robustness"]
min_cases = 2
```

`assess` lists every cell, including empty ones, with planned and evaluated
logical-case counts. A cell is covered when at least `min_cases` logical cases
have complete scored trial sets. Missing or undeclared category metadata is
reported as unclassified and blocks acceptance. Categories describe your test
intent; review whether the attached scorers actually test that intent. This is
separate from [evaluation coverage](evaluation-quality.md#require-evidence-in-ci),
which measures the availability of evaluation evidence.

## Review trajectories and label disagreements

```bash
mira review CANDIDATE --baseline BASELINE --out review.html
mira calibrate CANDIDATE --labels human-labels.jsonl --scorer 'llm_judge:*' \
  --out calibration.json
mira review CANDIDATE --baseline BASELINE --calibration calibration.json --out review.html
```

Open `review.html` locally. It is a self-contained offline viewer with archived
samples, final answers, scores, grading usage, captured files, and ATIF steps.
Baseline and candidate appear side by side. Choose a scorer, filter to
disagreements, and click a score reason to jump to a cited `step_id`. Missing
trajectories are stated explicitly. Incompatible paired comparisons show their
missing-evidence reasons.

Mark human pass/fail decisions, then export JSONL labels. Labels stay in memory
until you export them; the viewer never rewrites a run. Exports include execution
fingerprints when present. `calibrate` rejects fingerprint mismatches as
unavailable evidence. The reviewer imports exported labels and matching new
calibration reports; older calibration reports without execution fingerprints
must be regenerated before importing. An optional `--filter` case-key glob limits
the exported review artifact. Anyone with the HTML file can read its embedded
samples, transcripts, and files; distribute it as you would the saved run.

## Run the complete example

```bash
cargo build -p mira-cli
python3 examples/diagnostics/smoke.py --mira target/debug/mira --out /tmp/mira-diagnostics
```

The example runs baseline/candidate studies, positive and negative acceptance
gates, a missing robustness category, grading-spend gates, comparison, calibration,
and HTML review. Its subjects and usage are deterministic fixtures; no provider
calls are made. See [the example](../examples/diagnostics/README.md) for individual
commands and [scorers](scorers.md) for the complete vocabulary.
