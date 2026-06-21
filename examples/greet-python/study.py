#!/usr/bin/env python3
"""A Mira eval study written in Python — no Mira dependency, just the protocol.

This is the polyglot seam: Mira's host speaks newline-delimited JSON over stdio
to a child process, so an eval study can be written in any language. This one
mirrors the Rust `greet` example. Drive it with the host CLI:

    mira --cmd "python3 examples/greet-python/study.py" list
    mira --cmd "python3 examples/greet-python/study.py" run

stdout carries ONLY protocol JSON (one object per line); logs go to stderr.
See docs/protocol.md for the normative reference.
"""
import json
import sys

PROTOCOL_VERSION = "1.0"


def log(msg):
    print(msg, file=sys.stderr, flush=True)


# --- the eval, defined as plain data -------------------------------------------
EVAL = {
    "name": "greet",
    "description": "Greets the user and reports the answer to life (Python study)",
    "samples": [{"id": "hi", "tags": ["smoke"]}],
    "scorers": ["succeeded", 'contains("42")'],
    "models": [{"label": "sim", "available": True}],
    "max_turns": 1,
    "metadata": {"suite": "smoke", "lang": "python"},
}

PROMPTS = {"hi": "Say hi and tell me the answer to life."}


def subject(prompt):
    """Stand-in 'agent': a real one would call a model. Returns a transcript."""
    response = f"Hi! In response to {prompt!r}: the answer is 42."
    out_tokens = len(response.split())
    return {
        "final_response": response,
        "iterations": 1,
        "tool_calls_count": 0,
        "tool_calls": [],
        "usage": {
            "input_tokens": 40 + out_tokens * 3,
            "output_tokens": out_tokens,
            "cost_usd": 0.0,
        },
        "timing": {"duration_ms": 60 + out_tokens * 4},
        "metadata": {},
        "error": None,
    }


def score(transcript):
    text = transcript["final_response"]
    scores = [
        {"scorer": "succeeded", "value": 1.0, "pass": True, "reason": "no error"},
        {
            "scorer": "contains",
            "value": 1.0 if "42" in text else 0.0,
            "pass": "42" in text,
            "reason": 'found "42"' if "42" in text else 'missing "42"',
        },
    ]
    aggregate = sum(s["value"] for s in scores) / len(scores)
    passed = all(s["pass"] for s in scores)
    return scores, aggregate, passed


def handle(method, params):
    if method == "initialize":
        return {
            "protocol_version": PROTOCOL_VERSION,
            "study": "greet-python",
            "study_version": "0.1.0",
            "evals": 1,
            "capabilities": ["usage"],
        }
    if method == "list":
        return {"evals": [EVAL]}
    if method == "run":
        sample = params["sample"]
        if params.get("eval") != EVAL["name"] or sample not in PROMPTS:
            raise ValueError(f"no such cell: {params}")
        transcript = subject(PROMPTS[sample])
        scores, aggregate, passed = score(transcript)
        return {
            "eval": params["eval"],
            "sample": sample,
            "model": params["model"],
            "passed": passed,
            "aggregate": aggregate,
            "scores": scores,
            "transcript": transcript,
            "skipped": False,
        }
    raise ValueError(f"unknown method: {method}")


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            print(json.dumps({"method": "log", "params": {"message": "bad json"}}), flush=True)
            continue
        rid, method, params = msg.get("id"), msg.get("method"), msg.get("params", {})
        try:
            result = handle(method, params)
            print(json.dumps({"id": rid, "result": result}), flush=True)
        except Exception as exc:  # noqa: BLE001 — report, don't crash the loop
            print(json.dumps({"id": rid, "error": {"message": str(exc)}}), flush=True)
    log("greet-python: stdin closed, exiting")


if __name__ == "__main__":
    main()
