"""The SDK's protocol coverage must track the generated `_meta` (from
schema/v1/meta.json). These guard the gaps that wire-type codegen alone can't:
the protocol version, the method set, and the capability vocabulary.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SDK = ROOT / "sdks" / "python"
sys.path.insert(0, str(SDK))

import mira  # noqa: E402
from mira import _meta  # noqa: E402
from mira._serve import HANDLED_METHODS  # noqa: E402

META_JSON = json.loads((ROOT / "schema" / "v1" / "meta.json").read_text())


def test_generated_meta_matches_source():
    assert _meta.PROTOCOL_VERSION == META_JSON["version"]
    assert _meta.MIN_PROTOCOL_VERSION == META_JSON["min_version"]
    assert set(_meta.METHODS) == set(META_JSON["methods"])
    assert set(_meta.CAPABILITIES) == set(META_JSON["capabilities"])


def test_serve_handles_every_protocol_method():
    # A new method in the protocol must be dispatched by the serve loop — not
    # silently unhandled. (`events` is a notification kind, not a method.)
    assert set(_meta.METHODS) <= set(HANDLED_METHODS)


def test_handled_methods_actually_dispatch():
    s = mira.Study("t")

    @s.eval(samples=[mira.Sample("x", prompt="p")], models=[mira.model("sim")],
            scorers=[mira.succeeded()])
    def e(sample, cx):
        return mira.transcript("ok", usage=mira.Usage(input_tokens=1, output_tokens=1))

    base = {"eval": "e", "sample": "x", "model": "sim"}
    ex = s.handle("execute", base)
    payloads = {
        "initialize": {}, "list": {}, "run": base, "execute": base,
        "score": {**base, "transcript": ex["transcript"]},
    }
    for method in HANDLED_METHODS:
        s.handle(method, payloads[method])  # must not raise "unknown method"


def test_advertised_capabilities_are_known_tokens():
    s = mira.Study("t")

    @s.eval(samples=[mira.Sample("x", prompt="p")], models=[mira.model("sim")],
            scorers=[mira.succeeded()], axes=[mira.axis("effort", ["low", "high"])])
    def e(sample, cx):
        return mira.transcript("ok", usage=mira.Usage(input_tokens=1, output_tokens=1))

    advertised = s.handle("initialize", {})["capabilities"]
    assert set(advertised) <= set(_meta.CAPABILITIES)
    # The axis-bearing study must advertise `axes`.
    assert "axes" in advertised


def test_protocol_version_is_reported():
    init = mira.Study("t").handle("initialize", {})
    assert init["protocol_version"] == _meta.PROTOCOL_VERSION
