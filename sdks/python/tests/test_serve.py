"""Serve-loop behaviour and scoring semantics (mirrors crate::runner)."""
import io
import json
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SDK))

import mira  # noqa: E402
from mira._serve import _aggregate, _verdict  # noqa: E402
from mira._wire import Transcript  # noqa: E402


def _study():
    s = mira.Study("t")

    @s.eval(name="e", samples=[mira.Sample("s", prompt="p")],
            models=[mira.model("sim"), mira.model("gone", available=False)],
            scorers=[mira.succeeded(), mira.contains("42")])
    def e(sample, cx):
        return mira.transcript("answer 42", usage=mira.Usage(input_tokens=1, output_tokens=1))

    return s


def _drive(study, lines):
    out = io.StringIO()
    study.serve(stdin=io.StringIO("".join(l + "\n" for l in lines)), stdout=out)
    return [json.loads(x) for x in out.getvalue().splitlines() if x]


def test_full_session_over_stdio():
    msgs = _drive(_study(), [
        json.dumps({"id": 1, "method": "initialize", "params": {}}),
        json.dumps({"id": 2, "method": "list"}),
        json.dumps({"id": 3, "method": "run", "params": {"eval": "e", "sample": "s", "model": "sim"}}),
    ])
    assert msgs[0]["result"]["study"] == "t"
    assert msgs[1]["result"]["evals"][0]["name"] == "e"
    assert msgs[2]["result"]["passed"] is True
    assert msgs[2]["result"]["aggregate"] == 1.0


def test_bad_json_logs_and_continues():
    msgs = _drive(_study(), ["{ not json",
                             json.dumps({"id": 1, "method": "initialize"})])
    assert msgs[0]["method"] == "log"
    assert msgs[1]["result"]["protocol_version"] == mira.PROTOCOL_VERSION


def test_unknown_method_errors_without_crashing():
    msgs = _drive(_study(), [json.dumps({"id": 1, "method": "nope"}),
                             json.dumps({"id": 2, "method": "list"})])
    assert "unknown method" in msgs[0]["error"]["message"]
    assert "result" in msgs[1]  # loop kept going


def test_unavailable_model_is_skipped_na():
    result = _study().handle("run", {"eval": "e", "sample": "s", "model": "gone"})
    assert result["skipped"] is True
    # Infra error short-circuits to a single N/A — neither pass nor fail.
    assert result["passed"] is False
    assert result["scores"][0]["na"] is True


def test_verdict_and_aggregate_ignore_na():
    passing = mira.make_score("a", 1.0, True, "")
    failing = mira.make_score("b", 0.0, False, "")
    na = mira.make_score("c", 0.0, False, "", na=True)
    assert _verdict([passing, na]) is True          # NA excluded
    assert _verdict([passing, failing]) is False
    assert _verdict([na]) is False                   # nothing applicable
    assert _aggregate([passing, na]) == 1.0          # NA not averaged in
    assert _aggregate([na]) == 0.0


def test_forward_compat_ignores_unknown_fields():
    # A future host sends a transcript with a field this build doesn't know.
    from mira import _codec
    decoded = _codec.from_dict(Transcript, {
        "final_response": "hi", "iterations": 1, "tool_calls_count": 0,
        "usage": {"input_tokens": 1, "output_tokens": 1, "cost_usd": 0.0},
        "energy_joules": 99,  # unknown — must be ignored
    })
    assert decoded.final_response == "hi"
    assert decoded.usage.input_tokens == 1
