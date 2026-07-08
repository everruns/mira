"""Cross-language ATIF trajectory conformance. Runs the canonical vectors in
`schema/v1/conformance/trajectory.json` (encoding the behaviour of the Rust
types in `crates/mira-eval/src/trajectory.rs`, the source of truth) through
this SDK's generated wire types and hand-written projection mirror
(`mira/trajectory.py`), asserting each document parses (or is rejected),
round-trips (extra maps included; unknown fields tolerated), and projects onto
the pinned Transcript flat fields. The `scorers.json` three-runner pattern.
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
from mira import trajectory as trj  # noqa: E402
from mira._wire import Trajectory, Usage  # noqa: E402

VECTORS = json.loads(
    (ROOT / "schema" / "v1" / "conformance" / "trajectory.json").read_text()
)
CASES = VECTORS["cases"]
ACCEPTED = [c for c in CASES if not c.get("rejects")]


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_parses_or_rejects(case):
    if case.get("rejects"):
        with pytest.raises(ValueError, match="schema_version"):
            trj.parse_trajectory(case["trajectory"])
    else:
        trj.parse_trajectory(case["trajectory"])


@pytest.mark.parametrize("case", ACCEPTED, ids=[c["name"] for c in ACCEPTED])
def test_round_trips(case):
    first = trj.parse_trajectory(case["trajectory"])
    encoded = _codec.to_dict(first)
    again = trj.parse_trajectory(encoded)
    # Lossless: re-encode of the re-parse is identical (extra maps included).
    assert _codec.to_dict(again) == encoded


@pytest.mark.parametrize("case", ACCEPTED, ids=[c["name"] for c in ACCEPTED])
def test_projects_pinned_flat_fields(case):
    trajectory = trj.parse_trajectory(case["trajectory"])
    expect = case["projection"]

    # The zero-burden constructor is the pinned path.
    t = trj.from_trajectory(trajectory)
    assert t.final_response == expect["final_response"]
    assert t.tool_calls == expect["tool_calls"]
    assert t.tool_calls_count == expect["tool_calls_count"]
    assert t.iterations == expect["iterations"]
    usage = expect["usage"]
    assert t.usage.input_tokens == usage["input_tokens"]
    assert t.usage.output_tokens == usage["output_tokens"]
    assert t.usage.cache_read_tokens == usage["cache_read_tokens"]
    assert t.usage.reasoning_tokens == usage["reasoning_tokens"]
    assert math.isclose(t.usage.cost_usd, usage["cost_usd"], abs_tol=1e-9)


def test_trajectory_only_transcript_scores_with_name_based_scorers():
    """Zero client burden, end-to-end: a subject returns a transcript that sets
    ONLY `trajectory` — no flat fields, no events — and the built-in name-based
    scorers see the projected names via the serve loop's normalization."""
    s = mira.Study("traj", version="0.0.1")

    doc = {
        "schema_version": trj.ATIF_VERSION,
        "agent": {"name": "external-agent", "version": "1.0"},
        "steps": [
            {"step_id": 1, "source": "user", "message": "hi"},
            {
                "step_id": 2,
                "source": "agent",
                "message": "hi there",
                "tool_calls": [
                    {"tool_call_id": "c1", "function_name": "search",
                     "arguments": {"q": "hi"}}
                ],
                "metrics": {"prompt_tokens": 10, "completion_tokens": 4},
            },
        ],
    }

    @s.eval(
        name="greet",
        samples=[mira.Sample("hi", prompt="say hi")],
        targets=[mira.target("sim")],
        scorers=[mira.contains("hi there"), mira.tool_called("search"),
                 mira.tool_calls_within(1)],
    )
    def greet(sample, cx):
        # The transcript carries ONLY the trajectory — nothing else to call.
        return mira.Transcript(trajectory=trj.parse_trajectory(doc))

    base = {"eval": "greet", "sample": "hi", "target": "sim"}
    run = s.handle("run", base)
    assert run["passed"], run["scores"]
    assert run["transcript"]["final_response"] == "hi there"
    assert run["transcript"]["tool_calls"] == ["search"]
    assert run["transcript"]["usage"]["input_tokens"] == 10

    # The score path (a replayed trajectory-only transcript) normalizes too.
    scored = s.handle("score", {**base, "transcript": {"trajectory": doc}})
    assert scored["passed"], scored["scores"]
    assert scored["transcript"]["tool_calls"] == ["search"]

    # execute returns the full transcript with the trajectory + projections.
    ex = s.handle("execute", base)
    assert ex["transcript"]["trajectory"]["schema_version"] == trj.ATIF_VERSION
    assert ex["transcript"]["tool_calls"] == ["search"]

    # The capability + params are advertised.
    init = s.handle("initialize", {})
    assert "trajectory" in init["capabilities"]
    assert init["capability_params"]["trajectory"] == {"format": "ATIF", "version": "1.7"}


def test_explicit_flat_fields_are_never_overwritten():
    trajectory = trj.parse_trajectory({
        "schema_version": "ATIF-v1.7",
        "agent": {"name": "a", "version": "1"},
        "steps": [{"step_id": 1, "source": "agent", "message": "derived"}],
    })
    t = mira.Transcript(final_response="explicit", iterations=7)
    trj.project_into(trajectory, t)
    assert t.final_response == "explicit"
    assert t.iterations == 7
