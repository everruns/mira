"""Cross-language scorer parity. Runs the canonical vectors in
`schema/v1/conformance/scorers.json` (encoding the behaviour of the Rust
scorers in `crates/mira-eval/src/scorer.rs`, the source of truth) through this
SDK's hand-written scorers and asserts the verdict matches.

Only the verdict-affecting fields (pass/value/na) are checked — `reason` text is
human-facing and allowed to differ. A coverage check ensures every scorer kind
in the vectors is implemented here (or explicitly declared unsupported), so a
scorer added to Rust can't silently go missing in Python.
"""
import json
import math
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SDK = ROOT / "sdks" / "python"
sys.path.insert(0, str(SDK))

import mira  # noqa: E402
from mira import _codec  # noqa: E402
from mira._wire import Transcript  # noqa: E402

VECTORS = json.loads((ROOT / "schema" / "v1" / "conformance" / "scorers.json").read_text())

# Scorers that are intentionally not portable to this SDK (no deterministic,
# language-neutral spec). They never appear in the vectors; listed for clarity.
UNSUPPORTED = {"model_graded", "scorer"}


def build(spec: dict) -> mira.Scorer:
    """Construct a scorer from a canonical descriptor (recursive for combinators)."""
    kind = spec["kind"]
    if kind == "contains":
        return mira.contains(spec["needle"])
    if kind == "not_contains":
        return mira.not_contains(spec["needle"])
    if kind == "equals":
        return mira.equals(spec["expected"])
    if kind == "regex":
        return mira.regex(spec["pattern"])
    if kind == "matches_expected":
        return mira.matches_expected()
    if kind == "non_empty":
        return mira.non_empty()
    if kind == "succeeded":
        return mira.succeeded()
    if kind == "file_exists":
        return mira.file_exists(spec["path"])
    if kind == "file_contains":
        return mira.file_contains(spec["path"], spec["needle"])
    if kind == "tool_called":
        return mira.tool_called(spec["tool"])
    if kind == "tool_not_called":
        return mira.tool_not_called(spec["tool"])
    if kind == "tool_calls_within":
        return mira.tool_calls_within(spec["max"])
    if kind == "turns_within":
        return mira.turns_within(spec["max"])
    if kind == "tools_used_exactly":
        return mira.tools_used_exactly(spec["tools"])
    if kind == "tool_called_before":
        return mira.tool_called_before(spec["first"], spec["second"])
    if kind == "cost_within":
        return mira.cost_within(spec["max_usd"])
    if kind == "tokens_within":
        return mira.tokens_within(spec["max"])
    if kind == "output_tokens_within":
        return mira.output_tokens_within(spec["max"])
    if kind == "latency_within":
        return mira.latency_within(spec["max_ms"])
    if kind == "ttft_within":
        return mira.ttft_within(spec["max_ms"])
    if kind == "metric_within":
        return mira.metric_within(spec["name"], spec["max"])
    if kind == "metric_at_least":
        return mira.metric_at_least(spec["name"], spec["min"])
    if kind == "json_valid":
        return mira.json_valid()
    if kind == "json_field_equals":
        return mira.json_field_equals(spec["key"], spec["value"])
    if kind == "produced_modality":
        return mira.produced_modality(spec["modality"])
    if kind == "all_of":
        return mira.all_of(spec["name"], [build(s) for s in spec["of"]])
    if kind == "any_of":
        return mira.any_of(spec["name"], [build(s) for s in spec["of"]])
    if kind == "not":
        return mira.not_(build(spec["of"]))
    raise KeyError(f"unhandled scorer kind: {kind}")


def _transcript(name: str) -> Transcript:
    return _codec.from_dict(Transcript, VECTORS["transcripts"][name])


def _sample(case: dict) -> mira.Sample:
    s = case.get("sample", {})
    return mira.Sample("s", expected=s.get("expected"))


@pytest.mark.parametrize("case", VECTORS["cases"], ids=[c["name"] for c in VECTORS["cases"]])
def test_scorer_matches_rust(case):
    scorer = build(case["scorer"])
    score = scorer.score(_sample(case), _transcript(case["transcript"]))
    expect = case["expect"]
    assert score.pass_ is expect["pass"], f"{case['name']}: pass mismatch ({score.reason})"
    assert score.na is expect["na"], f"{case['name']}: na mismatch ({score.reason})"
    assert math.isclose(score.value, expect["value"], abs_tol=1e-9), \
        f"{case['name']}: value {score.value} != {expect['value']}"


def _distinct_specs(spec, acc):
    """Collect every (sub)spec by kind, using the real descriptors from the
    vectors so combinators recurse without dummy args."""
    acc.setdefault(spec["kind"], spec)
    of = spec.get("of")
    if isinstance(of, list):
        for s in of:
            _distinct_specs(s, acc)
    elif isinstance(of, dict):
        _distinct_specs(of, acc)
    return acc


def test_every_vector_kind_is_implemented():
    """Coverage: each scorer kind used by the vectors must build here (parity
    dashboard — a kind added to Rust + vectors that we haven't mirrored fails)."""
    specs = {}
    for c in VECTORS["cases"]:
        _distinct_specs(c["scorer"], specs)
    missing = []
    for kind, spec in sorted(specs.items()):
        try:
            build(spec)
        except KeyError:
            if kind not in UNSUPPORTED:
                missing.append(kind)
    assert not missing, f"scorer kinds in vectors not implemented in Python: {missing}"
