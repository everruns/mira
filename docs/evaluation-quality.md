# Trustworthy evaluation workflows

An evaluation has two independent outcomes: how well the target performed, and
whether enough evidence was evaluated to support that conclusion. Mira keeps
infrastructure errors as N/A and lets you require evaluation coverage for CI.

## Require evidence in CI

```bash
mira run --study study.rs --min-coverage 1 --max-infra-rate 0 \
  --require-scorer 'llm_judge:*'
```

`--min-coverage` is the fraction of selected cases with a real score and every
required scorer pattern satisfied. Skipped cases remain in its denominator.
Each `--require-scorer` glob must match at least one scorer on every
case, and every matching scorer must have a non-N/A verdict. `--max-infra-rate` counts cases with any N/A scorer or an infrastructure
error, divided by all selected cases. All fractions use the range 0 through 1.

The defaults preserve offline runs: no coverage requirement is imposed. A strict
run exits 2 when configured coverage gates are unmet, 1 when coverage passes but target
quality fails, and 0 when both gates pass. An empty selection cannot satisfy
positive coverage or required-scorer gates. `evaluation.json` records the
separate outcomes in a saved run. Its `complete` field describes actual evidence
completeness; `gates_passed` and `gates_configured` describe the policy. An ungated
offline run can exit 0 while clearly reporting incomplete evaluation. The same
outcome and policy are embedded in `meta.json` and the saved JSON/HTML reports,
and survive `mira report` without a study. A filtered report re-evaluates the
saved policy on its selected slice.

JSON and HTML embed an `evaluation` object. HTML and Markdown display evidence
completeness and gate status. JUnit adds a `mira.coverage` suite, with a failing
test when coverage gates fail. JSONL preserves one `RunResult` per line and puts
run evidence in `transcript.metadata["mira.evaluation"]`; CSV exposes that as
`meta.mira.evaluation`. Empty row exports have no data rows, so use JSON or the
saved `evaluation.json` for a zero-case verdict.

You can put defaults in `mira.toml`:

```toml
[presets.release.gates]
min_coverage = 1.0
max_infra_rate = 0.0
require_scorer = ["llm_judge:*"]
```

Global `[gates]` defaults apply to both `run` and `score`. A run's preset overrides
global defaults, and explicit CLI gates override the preset. Experiment runs
forward explicit coverage gates to every treatment.

## Compare versions on the same cases

```bash
mira compare BASELINE_RUN CANDIDATE_RUN --max-regressions 0 \
  --max-score-drop 0.02 --max-cost-increase 0.10 --out comparison.json
```

Use `--filter 'coding/*@sim*'` for a case slice and `--scorer 'correctness*'` for
a scoring dimension. The comparison pairs exact case/trial keys, requires equal
archived samples, seeds, trial counts, and scorer identities, and reports missing
or N/A evidence as incomplete. Legacy results without sample snapshots are
unverified. Each logical case must have its complete trial set.

A regression is a decrease in a logical case's pass rate. Scores are reported
separately by scorer, and `--max-score-drop` limits the mean decrease for each
scorer. Cost and duration limits are relative increases in summed subject
metrics: `0.10` means 10%. With a zero baseline, any positive increase exceeds a
finite relative limit. Judge inference costs are not included in subject cost.

The pass-rate delta gives each logical case equal weight. Its deterministic
95% percentile bootstrap interval resamples logical cases, retaining repetitions
inside each case; fewer than two logical cases produce no interval. This is an
uncertainty estimate for the observed suite, not a guarantee about production.
Comparison exits 2 for incomplete evidence, 1 for breached comparison gates,
and 0 otherwise. Without explicit limits it reports changes without gating them.

[Experiment plans](features/experiment-plans.md) apply the same comparison after
all treatments finish. A failing baseline remains useful comparison data.

## Preserve the sample and grader

For studies advertising `execute` and `score`, saved runs retain full execution
artifacts under `artifacts/` before scoring. Their transcripts include the exact
sample snapshot and SHA-256 identities for the sample and execution. Repeated
cases retain their trial index, count, and seed through execution and scoring. The Rust,
Python, and TypeScript study implementations capture these automatically.

```bash
mira score --study study.rs --run-id ORIGINAL_RUN
mira score --study study.rs --run-id ORIGINAL_RUN --use-current-samples
```

The first command uses archived samples, including their original expected
answers, even when the current dataset changes. The second explicitly replaces
the reference data. Each scoring invocation creates a new run, retains its full
artifacts, and records the source and reference mode in `lineage.json`.

`provenance.json` archives the catalog, sample fingerprints, scorer definitions,
selection, Git commit, and tracked-diff fingerprint. Resume rejects changed
definitions or selection. Explicit execute-only artifact directories retain the
same guard under `.mira/`. Untracked code and external inputs need explicit
versioning; experiment treatments can declare `files` to fingerprint binaries,
prompts, and data files. Hashes verify bytes but do not archive those files.

LLM judges have content identities covering model, provider, rubric, threshold,
evidence settings, endpoint, system prompt, and integration version. Their public
definitions exclude API keys and redact user information, query strings, and
fragments from endpoint URLs; a fingerprint retains exact endpoint identity. Custom Rust scorers can override
`Scorer::definition()`; otherwise their name and study source revision identify
them. Python `scorer(..., configuration={...})` and TypeScript
`scorer(name, fn, configuration)` record equivalent public definitions. Changing
those definitions invalidates paired comparisons even if the scorer name stays
the same. The configuration is recorded after execution, so a grader change does
not alter the captured execution identity used for calibration.

## Calibrate a judge against people

Create a reviewed JSONL label set for captured outputs. Each key identifies one
specific execution, including its trial suffix when repeated:

```json
{"case_key":"qa/capital@sim","pass":true}
{"case_key":"qa/hallucination@sim","pass":false}
```

```bash
mira calibrate RUN --labels human-labels.jsonl --scorer 'llm_judge:*' \
  --min-agreement 0.90 --max-false-pass-rate 0.05 --out calibration.json
```

Each label must have exactly one matching, available scorer. The report includes
the confusion matrix, agreement, false-pass rate, disagreement keys, and human
and judge verdicts. False-pass rate is false passes divided by human-negative
cases. A rate threshold cannot pass without negative labels. Missing evidence
exits 2; failed calibration thresholds exit 1.

After rescoring the same executions with a revised judge, add
`--previous OLD_RUN --previous-scorer 'OLD_SCORER'` to report verdict flips.
Execution identities must match: rerunning the target is a different experiment.
Use factuality, tool behavior, and style as separate rubrics so tradeoffs remain
visible. Calibration measures the labeled set; use held-out labels to assess a
judge after tuning its rubric.

## Turn failures into regression cases

```bash
mira failures RUN --filter 'coding/*' --out review.jsonl
```

Mira groups failed trials and targets for the same sample into a review record.
It preserves the sample, source case keys, scores, outputs, and available ATIF
and captured-file evidence. Infrastructure-only failures are excluded. Use
`--scorer GLOB` to select a failure category.

These records deliberately wrap the `sample` in `status: needs_review`; they
cannot be loaded directly as a dataset. Review the input, reference, fixtures,
and evidence, then extract approved `sample` objects into your versioned JSONL
dataset. Run the candidate against that expanded suite before accepting it.

## Grade the right surface

Run the offline [evidence recipes](../examples/evidence.rs):

```bash
mira run --study examples/evidence.rs
```

The recipes cover tool arguments and correlated observations, retention across
input turns, retrieval precision/recall, and generation grounded in retrieved
evidence. The subjects are deterministic fixtures; replace them with your real
agent. Keep retrieval and generation scores separate to locate regressions.

## Run the complete workflow

The [quality workflow example](../examples/quality_workflow) includes an
experiment plan, label fixtures, a study, and an assertion-based smoke script:

```bash
cargo build -p mira-cli
python3 examples/quality_workflow/smoke.py --mira target/debug/mira --out /tmp/mira-smoke
```

The script checks successful and failing gates, both reference modes, grader
flips, failure review, all six report formats, and ATIF export. Its subject and
judge are deterministic fixtures; the example makes no provider calls.

## Surface support

| Feature | Supported surface |
|---|---|
| Coverage policy and outcome | Rust `mira::evaluation` API; shared CLI for Rust, Python, TypeScript, and protocol studies |
| Trajectory-aware provider judges | Rust `mira-judge`: OpenAI Completions/Responses and Claude; SDK custom judges remain study-owned |
| Original sample/execution identities | Automatic in Rust, Python, and TypeScript studies; custom protocol studies implement the documented metadata convention |
| Public scorer definitions | Rust `Scorer::definition()`; Python and TypeScript custom-scorer configuration; catalog and result metadata |
| Experiments, comparison, calibration, failure review | Host CLI, independent of study language; comparison/calibration output is JSON, review records are JSONL |
| Run coverage reports | Terminal, JSON, HTML, Markdown, JUnit, JSONL, CSV; saved-run re-rendering |
| Everruns publishing | Coverage outcome in source metadata; case reference and provenance in transcript metadata |
| Documentation | This guide, protocol/reference docs, CLI help, SDK READMEs, Rust API docs, and the portable Mira skill |

The hosted Everruns viewer owns its UI; transferring metadata does not add a
coverage-specific screen there. Comparison and calibration do not currently
provide dedicated HTML viewers. The local smoke verifies report contents and
publish payload mapping; it does not deploy or test a hosted viewer.

## Diagnose and accept a saved run

Use `mira assess RUN --policy policy.toml` for absolute slice thresholds and a
declared scenario × quality-dimension inventory. Use `mira review CANDIDATE
--baseline BASELINE --out review.html` to inspect trajectories side by side and
export human labels tied to execution fingerprints. Scores can carry grading
resources independently of subject cost. See [diagnostics and acceptance](diagnostics.md)
and its [offline workflow](../examples/diagnostics).
