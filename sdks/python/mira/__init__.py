"""Mira eval SDK for Python.

Author an eval *study* in Python and run it with the `mira` host CLI — no Rust
dependency, just the protocol (newline-delimited JSON over stdio). The wire
types in `mira._wire` are generated from the canonical JSON Schema under
`schema/v1/`, so they never drift from the Rust host.

    import mira

    study = mira.Study("my-evals", version="0.1.0")

    @study.eval(
        samples=[mira.Sample("hi", prompt="Say hi and the answer to life.")],
        targets=[mira.target("sim")],
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
    Target,
    RunCx,
    Sample,
    Study,
    log,
    target,
    serve,
)
from ._wire import AxisInfo, Score, Timing, Transcript, Usage
from .scorers import (
    Scorer,
    all_of,
    any_of,
    contains,
    cost_within,
    equals,
    file_contains,
    file_exists,
    json_field_equals,
    json_valid,
    latency_within,
    make_score,
    matches_expected,
    metric_at_least,
    metric_within,
    non_empty,
    not_,
    not_contains,
    output_tokens_within,
    produced_modality,
    regex,
    scorer,
    succeeded,
    tokens_within,
    tool_called,
    tool_called_before,
    tool_calls_within,
    tool_not_called,
    tools_used_exactly,
    ttft_within,
    turns_within,
)


def transcript(final_response: str = "", **kwargs: Any) -> Transcript:
    """Convenience builder for a `Transcript` (subjects' return value)."""
    return Transcript(final_response=final_response, **kwargs)


def axis(name: str, values) -> AxisInfo:
    """Declare an extra matrix axis (crossed with the target matrix)."""
    return AxisInfo(name=name, values=list(values))


__all__ = [
    "PROTOCOL_VERSION",
    "AxisInfo",
    "Eval",
    "Target",
    "RunCx",
    "Sample",
    "Score",
    "Scorer",
    "Study",
    "Timing",
    "Transcript",
    "Usage",
    "axis",
    "all_of",
    "any_of",
    "contains",
    "cost_within",
    "equals",
    "file_contains",
    "file_exists",
    "json_field_equals",
    "json_valid",
    "latency_within",
    "log",
    "make_score",
    "matches_expected",
    "metric_at_least",
    "metric_within",
    "non_empty",
    "not_",
    "not_contains",
    "output_tokens_within",
    "produced_modality",
    "target",
    "regex",
    "scorer",
    "serve",
    "succeeded",
    "tokens_within",
    "tool_called",
    "tool_called_before",
    "tool_calls_within",
    "tool_not_called",
    "tools_used_exactly",
    "ttft_within",
    "turns_within",
    "transcript",
    "_wire",
]
