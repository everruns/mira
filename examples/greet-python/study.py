#!/usr/bin/env python3
"""A Mira eval study written in Python with the Mira Python SDK.

This is the polyglot seam: Mira's host speaks newline-delimited JSON over stdio
to a child process, so an eval study can be written in any language. This one
mirrors the Rust `greet` example. Drive it with the host CLI:

    mira --cmd "python3 examples/greet-python/study.py" list
    mira --cmd "python3 examples/greet-python/study.py" run

The SDK has no Rust dependency — its wire types are generated from the protocol
JSON Schema (schema/v1/). stdout carries ONLY protocol JSON; logs go to stderr.
"""
import sys
from pathlib import Path

# Make the in-repo SDK importable without an install, so the example runs in CI.
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdks" / "python"))

import mira  # noqa: E402

study = mira.Study("greet-python", version="0.1.0")


@study.eval(
    name="greet",
    description="Greets the user and reports the answer to life (Python SDK study)",
    samples=[mira.Sample("hi", prompt="Say hi and tell me the answer to life.", tags=["smoke"])],
    models=[mira.model("sim")],
    scorers=[mira.succeeded(), mira.contains("42")],
    metadata={"suite": "smoke", "lang": "python"},
)
def greet(sample, cx):
    # A real subject would call a model; this one fakes a good answer.
    response = f"Hi! In response to {sample.text!r}: the answer is 42."
    out_tokens = len(response.split())
    return mira.transcript(
        response,
        iterations=1,
        usage=mira.Usage(input_tokens=40 + out_tokens * 3, output_tokens=out_tokens),
        timing=mira.Timing(duration_ms=60 + out_tokens * 4),
    )


if __name__ == "__main__":
    study.serve()
