# mira-eval — Python SDK

Author a Mira eval **study** in Python and run it with the `mira` host CLI.

This is **not** a binding to the Rust core — it's a native, pure-stdlib library
that speaks the [Mira eval protocol](../../docs/protocol.md) (newline-delimited
JSON over stdio). The host owns selection, the model matrix, concurrency,
checkpoints, and reporting; the study owns subjects and scoring. Any language
that speaks the protocol is a first-class study — this SDK just makes the Python
side ergonomic.

The protocol layer is **generated** from the canonical artifacts under
[`schema/v1/`](../../schema/v1/) — the same language-neutral contract the Rust
host is generated from — so it never drifts from the wire format:
[`mira/_wire.py`](mira/_wire.py) (wire types, from `schema.json`) and
[`mira/_meta.py`](mira/_meta.py) (protocol version, methods, capability tokens,
from `meta.json`).

## Use

```python
import mira

study = mira.Study("my-evals", version="0.1.0")

@study.eval(
    samples=[mira.Sample("hi", prompt="Say hi and the answer to life.", tags=["smoke"])],
    models=[mira.model("sim")],
    scorers=[mira.succeeded(), mira.contains("42")],
)
def greet(sample, cx):
    # A real subject calls a model; route on cx.model / cx.provider.
    return mira.transcript(
        f"Hi! The answer is 42. ({sample.text})",
        usage=mira.Usage(input_tokens=40, output_tokens=8),
    )

if __name__ == "__main__":
    study.serve()
```

Drive it with the host:

```bash
mira --cmd "python3 study.py" list
mira --cmd "python3 study.py" run
# run-now, score-later (split execute/score path):
mira --cmd "python3 study.py" run --execute-only --artifacts art/
mira --cmd "python3 study.py" score --artifacts art/
```

A complete, runnable example lives in
[`examples/greet-python`](../../examples/greet-python).

## API

- `Study(name, version=None)` — the registry; `@study.eval(...)` registers a
  subject `fn(sample, cx) -> Transcript`; `study.serve()` runs the stdio loop
  (handling `initialize`/`list`/`run`/`execute`/`score`).
- `Sample(id, prompt=…|input=[…], tags=…, target=…, files=…, metadata=…)` —
  `sample.text` joins the input turns for the subject.
- `model(label, provider="", available=True)` — a matrix cell. An unavailable
  model is reported as **N/A** (infra), not a failure.
- `RunCx` — `cx.model`, `cx.provider`, `cx.max_turns`, `cx.param(name)`.
- `transcript(final_response, usage=…, timing=…, iterations=…, …)` and the
  `Usage`/`Timing` types.
- Scorers: `succeeded()`, `contains(text)`, `equals(text)`, `regex(pattern)`,
  and `scorer(name, fn)` for an arbitrary predicate (return a bool or a
  fully-formed `Score`, including `na=True`).
- `axis(name, values)` — an extra matrix axis (crossed with the model matrix).

## Develop

```bash
python3 codegen.py            # regenerate mira/_wire.py + mira/_meta.py from schema/v1/
python3 codegen.py --check    # fail if either is stale (CI drift guard)
pip install -e .[dev]
python3 -m pytest             # conformance + metadata-coverage + serve-loop tests
```

The runtime has **zero dependencies**; `jsonschema` and `pytest` are dev-only.
