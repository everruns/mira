"""Every message the SDK emits must validate against the canonical JSON Schema
(schema/v1/schema.json) — the same contract the Rust host is generated from.
This is the cross-language drift guard, the Python dual of mira-schema-gen's
validation suite.
"""
import json
import subprocess
import sys
from pathlib import Path

import jsonschema
import pytest

ROOT = Path(__file__).resolve().parents[3]
SDK = ROOT / "sdks" / "python"
sys.path.insert(0, str(SDK))

import mira  # noqa: E402

SCHEMA = json.loads((ROOT / "schema" / "v1" / "schema.json").read_text())


def _subschema(def_name: str) -> dict:
    """A validator schema for one $def, carrying the shared $defs for $ref."""
    return {"$schema": SCHEMA["$schema"], "$defs": SCHEMA["$defs"],
            "$ref": f"#/$defs/{def_name}"}


@pytest.fixture
def study():
    s = mira.Study("conformance", version="0.1.0")

    @s.eval(
        name="greet",
        samples=[mira.Sample("hi", prompt="hi", tags=["smoke"])],
        models=[mira.model("sim"), mira.model("anthropic/x", provider="anthropic", available=False)],
        scorers=[mira.succeeded(), mira.contains("42")],
        axes=[mira.axis("effort", ["low", "high"])],
        metadata={"suite": "smoke"},
    )
    def greet(sample, cx):
        return mira.transcript("the answer is 42", iterations=1,
                               usage=mira.Usage(input_tokens=10, output_tokens=4))

    return s


@pytest.mark.parametrize(
    "method,params,result_def",
    [
        ("initialize", {}, "InitializeResult"),
        ("list", {}, "ListResult"),
        ("run", {"eval": "greet", "sample": "hi", "model": "sim"}, "RunResult"),
        ("execute", {"eval": "greet", "sample": "hi", "model": "sim"}, "ExecuteResult"),
    ],
)
def test_result_matches_schema(study, method, params, result_def):
    result = study.handle(method, params)
    jsonschema.validate(result, _subschema(result_def))
    # The full line must also validate against the root (anyOf envelopes).
    jsonschema.validate({"id": 1, "result": result}, SCHEMA)


def test_score_path_matches_schema(study):
    ex = study.handle("execute", {"eval": "greet", "sample": "hi", "model": "sim"})
    scored = study.handle("score", {"eval": "greet", "sample": "hi", "model": "sim",
                                    "transcript": ex["transcript"]})
    jsonschema.validate(scored, _subschema("RunResult"))


def test_axis_params_flow_through(study):
    result = study.handle("run", {"eval": "greet", "sample": "hi", "model": "sim",
                                  "params": {"effort": "high"}})
    assert result["params"] == {"effort": "high"}
    jsonschema.validate(result, _subschema("RunResult"))


def test_capabilities_advertise_axes(study):
    init = study.handle("initialize", {})
    assert set(init["capabilities"]) >= {"axes", "usage", "execute", "score"}


def test_codegen_is_in_sync():
    """The committed wire types must match the current schema (drift guard)."""
    proc = subprocess.run([sys.executable, "codegen.py", "--check"], cwd=SDK,
                          capture_output=True, text=True)
    assert proc.returncode == 0, proc.stderr
