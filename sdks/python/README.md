# mira-eval, Python SDK

Author a Mira eval **study** in Python and run it with the `mira` host CLI.

This is **not** a binding to the Rust core, it's a native, pure-stdlib library
that speaks the [Mira eval protocol](../../docs/protocol.md) (newline-delimited
JSON over stdio). The host owns selection, the model matrix, concurrency,
saved runs, and reporting; the study owns subjects and scoring. Any language
that speaks the protocol is a first-class study, this SDK just makes the Python
side ergonomic.

The protocol layer is **generated** from the canonical artifacts under
[`schema/v1/`](../../schema/v1/), the same language-neutral contract the Rust
host is generated from, so it never drifts from the wire format:
[`mira/_wire.py`](mira/_wire.py) (wire types, from `schema.json`) and
[`mira/_meta.py`](mira/_meta.py) (protocol version, methods, capability tokens,
from `meta.json`).

## Use

```python
import mira

study = mira.Study("my-evals", version="0.1.0")

@study.eval(
    samples=[mira.Sample("hi", prompt="Say hi and the answer to life.", tags=["smoke"])],
    targets=[mira.target("sim")],
    scorers=[mira.succeeded(), mira.contains("42")],
)
def greet(sample, cx):
    # A real subject calls a model; route on cx.target / cx.provider.
    return mira.transcript(
        f"Hi! The answer is 42. ({sample.text})",
        usage=mira.Usage(input_tokens=40, output_tokens=8),
    )

if __name__ == "__main__":
    study.serve()
```

Drive it with the host:

```bash
mira list --study-python study.py
mira run --study-python study.py
# run-now, score-later (split execute/score path):
mira run --study-python study.py --execute-only --artifacts art/
mira score --study-python study.py --artifacts art/
```

`--study-python` is a convenience launcher (`python3 …`); `--study study.py` and
`--study-uv` run via `uv run …` (PEP 723 inline metadata), and
`--study-cmd "python3 study.py"` still works for an arbitrary command line.

A complete, runnable example lives in
[`examples/greet-python`](../../examples/greet-python).

## API

- `Study(name, version=None, page_size=500)`, the registry; `@study.eval(...)`
  registers a subject `fn(sample, cx) -> Transcript`; `study.serve()` runs the
  stdio loop (handling
  `initialize`/`list`/`list_samples`/`run`/`execute`/`score`). `page_size`
  paginates large datasets across `list` + `list_samples` (`0` disables).
- `Sample(id, prompt=…|input=[…], tags=…, expected=…, files=…, metadata=…)`:
  `sample.text` joins the input turns for the subject.
- `target(label, provider="", available=True)`, a matrix case (the model or
  harness under evaluation). An unavailable target is reported as **N/A**
  (infra), not a failure.
- `RunCx`, `cx.target`, `cx.provider`, `cx.max_turns`, `cx.param(name)`.
- `transcript(final_response, usage=…, timing=…, iterations=…, …)` and the
  `Usage`/`Timing` types.
- Scorers: `succeeded()`, `contains(text)`, `equals(text)`, `regex(pattern)`,
  and `scorer(name, fn)` for an arbitrary predicate (return a bool or a
  fully-formed `Score`, including `na=True`).
- `axis(name, values)`, an extra matrix axis (crossed with the model matrix).

## Develop

```bash
python3 codegen.py            # regenerate mira/_wire.py + mira/_meta.py from schema/v1/
python3 codegen.py --check    # fail if either is stale (CI drift guard)
pip install -e .[dev]
python3 -m pytest             # conformance + metadata-coverage + serve-loop tests
```

The runtime has **zero dependencies**; `jsonschema` and `pytest` are dev-only.

## Coverage, provenance, comparison, and review

These workflows run in the shared host, so this SDK receives the same coverage
gates, experiment comparisons, calibration reports, and failure export as Rust
studies. The SDK automatically archives the original sample and execution
identity; `score` uses that snapshot even if the current dataset changes.

```bash
mira run --study-python study.py --min-coverage 1 --require-scorer "correctness*"
mira score --study-python study.py --run-id RUN
mira compare BASELINE CANDIDATE --max-regressions 0
mira calibrate RUN --labels labels.jsonl --scorer 'correctness*' --min-agreement 0.9
mira failures RUN --out review.jsonl
```

Custom scorers can supply public configuration for catalog and result
provenance. Keep credentials out of this configuration:

```python
correct = mira.scorer("correctness:v1", lambda sample, t: t.final_response == sample.expected,
                      configuration={"rubric": "match reviewed reference", "version": 1})
```

The host stores these definitions in catalog and result metadata and rejects
changed grader configurations during paired comparisons. Model-backed judge
transports remain in the Rust `mira-judge` integration; SDK studies can implement
their own provider-backed scorers.

See [Evaluation quality](../../docs/evaluation-quality.md) for the support matrix,
report formats, reference-mode controls, and gate exit codes. The runnable
[quality workflow](../../examples/quality_workflow) demonstrates all host
workflows without provider credentials.

Repeated runs (`mira run --trials 3 --seed 10`) preserve trial identity through
execution artifacts, scoring, and reports. Subjects receive the zero-based
`cx.trial`, repetition count `cx.trials`, and optional per-trial `cx.seed`.
A subject can pass that seed to its model or local random generator.

## Diagnostics and grading resources

See [diagnostic workflows](https://github.com/everruns/mira/blob/main/docs/diagnostics.md)
for correlated tool evidence, ordered steps, ranked retrieval metrics, absolute
acceptance policies, coverage inventory, and offline human review. Protocol 1.2
carries optional per-score grading resources separately from subject usage.
