"""Mira eval SDK for Python.

Author an eval *study* in Python and run it with the `mira` host CLI — no Rust
dependency, just the protocol (newline-delimited JSON over stdio). The wire
types in `mira._wire` are generated from the canonical JSON Schema under
`schema/v1/`, so they never drift from the Rust host.

    import mira

    study = mira.Study("my-evals", version="0.1.0")

    @study.eval(
        samples=[mira.Sample("hi", prompt="Say hi and the answer to life.")],
        models=[mira.model("sim")],
        scorers=[mira.succeeded(), mira.contains("42")],
    )
    def greet(sample, cx):
        return mira.transcript(f"Hi! The answer is 42. ({sample.text})")

    if __name__ == "__main__":
        study.serve()
"""
from __future__ import annotations

from typing import Any, Dict, Optional

from . import _wire
from ._serve import (
    PROTOCOL_VERSION,
    Eval,
    Model,
    RunCx,
    Sample,
    Study,
    log,
    model,
    serve,
)
from ._wire import AxisInfo, Score, Timing, Transcript, Usage
from .scorers import Scorer, contains, equals, make_score, regex, scorer, succeeded


def transcript(final_response: str = "", **kwargs: Any) -> Transcript:
    """Convenience builder for a `Transcript` (subjects' return value)."""
    return Transcript(final_response=final_response, **kwargs)


def axis(name: str, values) -> AxisInfo:
    """Declare an extra matrix axis (crossed with the model matrix)."""
    return AxisInfo(name=name, values=list(values))


__all__ = [
    "PROTOCOL_VERSION",
    "AxisInfo",
    "Eval",
    "Model",
    "RunCx",
    "Sample",
    "Score",
    "Scorer",
    "Study",
    "Timing",
    "Transcript",
    "Usage",
    "axis",
    "contains",
    "equals",
    "log",
    "make_score",
    "model",
    "regex",
    "scorer",
    "serve",
    "succeeded",
    "transcript",
    "_wire",
]
