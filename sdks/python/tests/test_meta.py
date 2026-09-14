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
    assert set(_meta.METHODS) == {m["name"] for m in META_JSON["methods"]}
    assert set(_meta.CAPABILITIES) == set(META_JSON["capabilities"])


def test_generated_directions_match_source():
    # The split the serve loop dispatches on. A study answers what the host
    # sends and emits its own notifications; conflating the two is what the flat
    # method list used to allow.
    served = {m["name"] for m in META_JSON["methods"] if m["direction"] == "initiator"}
    emitted = {m["name"] for m in META_JSON["methods"] if m["direction"] == "responder"}
    assert set(_meta.SERVED_METHODS) == served
    assert set(_meta.EMITTED_METHODS) == emitted
    assert served.isdisjoint(emitted)
    assert _meta.REQUIRES == {
        m["name"]: m["requires"] for m in META_JSON["methods"] if m.get("requires")
    }


def test_serve_handles_every_method_a_host_sends():
    # A new host-sent method must be dispatched by the serve loop — not silently
    # unhandled. `event`/`log` are the study's own notifications, so they are
    # not in this set; before meta.json carried directions, the flat list could
    # not tell the difference.
    assert set(_meta.SERVED_METHODS) <= set(HANDLED_METHODS)


def test_handled_methods_actually_dispatch():
    s = mira.Study("t")

    @s.eval(samples=[mira.Sample("x", prompt="p")], targets=[mira.target("sim")],
            scorers=[mira.succeeded()])
    def e(sample, cx):
        return mira.transcript("ok", usage=mira.Usage(input_tokens=1, output_tokens=1))

    base = {"eval": "e", "sample": "x", "target": "sim"}
    ex = s.handle("execute", base)
    payloads = {
        "initialize": {}, "list": {}, "list_samples": {"eval": "e", "cursor": "0"},
        "run": base, "execute": base,
        "score": {**base, "transcript": ex["transcript"]},
        "cancel": {"id": 1},
    }
    for method in HANDLED_METHODS:
        s.handle(method, payloads[method])  # must not raise "unknown method"


def test_advertised_capabilities_are_known_tokens():
    s = mira.Study("t")

    @s.eval(samples=[mira.Sample("x", prompt="p")], targets=[mira.target("sim")],
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
