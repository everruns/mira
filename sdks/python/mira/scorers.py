"""Built-in scorers and the `scorer(...)` escape hatch.

A scorer maps `(sample, transcript) -> Score`. `value` is a continuous 0..1
signal; `pass_` the boolean verdict; `na=True` means "couldn't evaluate"
(excluded from the case verdict and aggregate), mirroring `mira::Score`.

PARITY — source of truth: `crates/mira-eval/src/scorer.rs`.
The Rust scorers are canonical; every function here is a hand-written mirror of
its Rust twin (same name, same verdict). Behaviour is pinned by the shared
vectors in `schema/v1/conformance/scorers.json` and verified by
`tests/test_scorer_parity.py`. Change a scorer in Rust → update the vectors →
mirror the change here and in the TypeScript SDK. `reason` strings are
human-facing and may differ across languages; the verdict (pass/value/na) must
not. The LLM-judge (`model_graded`) is deliberately not mirrored — it is not a
deterministic, portable scorer.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any, Callable, Iterator, List, Optional, Sequence, Tuple, Union

from ._wire import Score, Trajectory
from .trajectory import _content_text


def make_score(name: str, value: float, passed: bool, reason: str, na: bool = False) -> Score:
    return Score(scorer=name, value=value, pass_=passed, reason=reason, na=na)


@dataclass
class Scorer:
    """A named `(sample, transcript) -> Score`."""

    name: str
    fn: Callable[["object", "object"], Score]

    def score(self, sample, transcript) -> Score:
        return self.fn(sample, transcript)


def _passfail(name: str, ok: bool, yes: str, no: str) -> Score:
    return make_score(name, 1.0 if ok else 0.0, ok, yes if ok else no)


def _tools_used(transcript) -> List[str]:
    """Distinct tool names in first-seen order — mirrors `Transcript::tools_used`."""
    seen: List[str] = []
    for n in (transcript.tool_calls or []):
        if n not in seen:
            seen.append(n)
    return seen


# ----- text scorers ---------------------------------------------------------

def succeeded() -> Scorer:
    def fn(_sample, transcript) -> Score:
        ok = transcript.error is None
        return make_score("succeeded", 1.0 if ok else 0.0, ok,
                          "no error" if ok else (transcript.error or "errored"))
    return Scorer("succeeded", fn)


def contains(text: str) -> Scorer:
    name = f'contains("{text}")'

    def fn(_sample, transcript) -> Score:
        found = text in (transcript.final_response or "")
        return make_score(name, 1.0 if found else 0.0, found,
                          f'found "{text}"' if found else f'missing "{text}"')
    return Scorer(name, fn)


def not_contains(text: str) -> Scorer:
    name = f'not_contains("{text}")'

    def fn(_sample, transcript) -> Score:
        present = text in (transcript.final_response or "")
        return _passfail(name, not present, f'absent "{text}"', f'unexpectedly found "{text}"')
    return Scorer(name, fn)


def equals(target: str) -> Scorer:
    """Trimmed, ASCII-case-insensitive match — mirrors Rust `equals`."""
    name = f'equals("{target}")'

    def fn(_sample, transcript) -> Score:
        ok = (transcript.final_response or "").strip().lower() == target.strip().lower()
        return _passfail(name, ok, "exact match", "mismatch")
    return Scorer(name, fn)


def regex(pattern: str) -> Scorer:
    name = f"regex({pattern!r})"
    compiled = re.compile(pattern)

    def fn(_sample, transcript) -> Score:
        ok = compiled.search(transcript.final_response or "") is not None
        return _passfail(name, ok, "matched", "no match")
    return Scorer(name, fn)


def matches_expected() -> Scorer:
    """Trimmed, case-sensitive match against the sample's expected answer."""
    name = "matches_expected"

    def fn(sample, transcript) -> Score:
        expected = getattr(sample, "expected", None)
        if expected is None:
            return make_score(name, 0.0, False, "sample has no string expected answer")
        ok = (transcript.final_response or "").strip() == expected.strip()
        return _passfail(name, ok, "matched expected", f"expected {expected!r}")
    return Scorer(name, fn)


def non_empty() -> Scorer:
    name = "non_empty"

    def fn(_sample, transcript) -> Score:
        ok = bool((transcript.final_response or "").strip())
        return _passfail(name, ok, "non-empty response", "empty response")
    return Scorer(name, fn)


# ----- file / workspace scorers ---------------------------------------------

def file_exists(path: str) -> Scorer:
    name = f"file_exists({path})"

    def fn(_sample, transcript) -> Score:
        ok = path in (transcript.files or {})
        return _passfail(name, ok, f"{path} exists", f"no such file: {path}")
    return Scorer(name, fn)


def file_contains(path: str, needle: str) -> Scorer:
    name = f"file_contains({path}, {needle!r})"

    def fn(_sample, transcript) -> Score:
        files = transcript.files or {}
        if path not in files:
            return make_score(name, 0.0, False, f"no such file: {path}")
        ok = needle in files[path]
        return _passfail(name, ok, f"{path} contains {needle!r}", f"{path} missing {needle!r}")
    return Scorer(name, fn)


# ----- tool-call scorers ----------------------------------------------------

def tool_called(tool: str) -> Scorer:
    name = f"tool_called({tool})"

    def fn(_sample, transcript) -> Score:
        ok = tool in (transcript.tool_calls or [])
        return _passfail(name, ok, f"{tool} was called", f"{tool} never called")
    return Scorer(name, fn)


def tool_not_called(tool: str) -> Scorer:
    name = f"tool_not_called({tool})"

    def fn(_sample, transcript) -> Score:
        called = tool in (transcript.tool_calls or [])
        return _passfail(name, not called, f"{tool} never called", f"{tool} was called")
    return Scorer(name, fn)


def tool_calls_within(maximum: int) -> Scorer:
    name = f"tool_calls_within({maximum})"

    def fn(_sample, transcript) -> Score:
        n = transcript.tool_calls_count
        return _passfail(name, n <= maximum, f"{n} <= {maximum}", f"{n} > {maximum}")
    return Scorer(name, fn)


def turns_within(maximum: int) -> Scorer:
    name = f"turns_within({maximum})"

    def fn(_sample, transcript) -> Score:
        n = transcript.iterations
        return _passfail(name, n <= maximum, f"{n} <= {maximum}", f"{n} > {maximum}")
    return Scorer(name, fn)


def tools_used_exactly(tools: Sequence[str]) -> Scorer:
    expected = sorted(set(tools))
    label = ",".join(expected)
    name = f"tools_used_exactly([{label}])"

    def fn(_sample, transcript) -> Score:
        used = sorted(_tools_used(transcript))
        ok = used == expected
        return _passfail(name, ok, f"used exactly [{label}]",
                         f"expected [{label}], used [{','.join(used)}]")
    return Scorer(name, fn)


def tool_called_before(first: str, second: str) -> Scorer:
    name = f"tool_called_before({first}, {second})"

    def fn(_sample, transcript) -> Score:
        calls = transcript.tool_calls or []
        fi = calls.index(first) if first in calls else None
        si = calls.index(second) if second in calls else None
        if fi is not None and si is not None:
            return _passfail(name, fi < si, f"{first} before {second}",
                             f"{first} not before {second}")
        return make_score(name, 0.0, False, f"both {first} and {second} must be called")
    return Scorer(name, fn)


# ----- trajectory (ATIF) scorers ----------------------------------------------
# These grade the structure of the ATIF trajectory (`Transcript.trajectory`,
# the protocol's primary trajectory contract) — tool arguments, correlated
# observations, step counts. A transcript without a trajectory FAILS with
# "subject reported no trajectory" (the ttft_within precedent: an unverifiable
# check fails, it isn't N/A — N/A is reserved for infra).

_MISSING = object()


def _json_pointer(value: Any, pointer: str) -> Any:
    """Resolve an RFC 6901 JSON Pointer against a JSON value; `_MISSING` when it
    doesn't resolve (distinct from a resolved JSON null) — mirrors
    `serde_json::Value::pointer`."""
    if pointer == "":
        return value
    if not pointer.startswith("/"):
        return _MISSING
    for token in pointer[1:].split("/"):
        token = token.replace("~1", "/").replace("~0", "~")
        if isinstance(value, dict):
            if token not in value:
                return _MISSING
            value = value[token]
        elif isinstance(value, list):
            if not token.isdigit():
                return _MISSING
            idx = int(token)
            if idx >= len(value):
                return _MISSING
            value = value[idx]
        else:
            return _MISSING
    return value


def _json_eq(a: Any, b: Any) -> bool:
    """JSON value equality with Python's bool/int conflation removed
    (`True != 1`, mirroring `serde_json::Value` equality)."""
    if isinstance(a, bool) != isinstance(b, bool):
        return False
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_json_eq(v, b[k]) for k, v in a.items())
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_json_eq(x, y) for x, y in zip(a, b))
    if isinstance(a, (dict, list)) or isinstance(b, (dict, list)):
        return False
    return a == b


def _trajectory_invocations(trajectory: Trajectory) -> Iterator[Tuple[str, Any, Optional[Any]]]:
    """(function_name, arguments, observation content) triples in step order —
    the trajectory case of `Transcript::tool_invocations` (observation content
    joined on `source_call_id`)."""
    for step in trajectory.steps:
        results = step.observation.results if step.observation is not None else []
        for call in step.tool_calls:
            content = next(
                (r.content for r in results if r.source_call_id == call.tool_call_id),
                None,
            )
            yield call.function_name, call.arguments, content


def tool_called_with(tool: str, pointer: str, expected: Any) -> Scorer:
    """Passes when some invocation of `tool` has arguments whose value at
    `pointer` (an RFC 6901 JSON Pointer) equals `expected` (JSON equality).
    Requires the ATIF trajectory; a transcript without one fails."""
    name = f"tool_called_with({tool}, {pointer}, {json.dumps(expected)})"

    def fn(_sample, transcript) -> Score:
        trajectory = transcript.trajectory
        if trajectory is None:
            return make_score(name, 0.0, False, "subject reported no trajectory")
        for called, arguments, _content in _trajectory_invocations(trajectory):
            if called != tool:
                continue
            v = _json_pointer(arguments, pointer)
            if v is not _MISSING and _json_eq(v, expected):
                return make_score(name, 1.0, True, f"{tool} called with {pointer} == {expected!r}")
        return make_score(name, 0.0, False, f"no {tool} call with {pointer} == {expected!r}")
    return Scorer(name, fn)


def tool_arg_matches(tool: str, pointer: str, pattern: str) -> Scorer:
    """Passes when some invocation of `tool` has a **string** argument at
    `pointer` matching the regex `pattern` — the regex variant of
    `tool_called_with`. A non-string value at the pointer fails with a reason.
    Requires the ATIF trajectory; a transcript without one fails."""
    name = f"tool_arg_matches({tool}, {pointer}, {pattern!r})"
    compiled = re.compile(pattern)

    def fn(_sample, transcript) -> Score:
        trajectory = transcript.trajectory
        if trajectory is None:
            return make_score(name, 0.0, False, "subject reported no trajectory")
        non_string = False
        for called, arguments, _content in _trajectory_invocations(trajectory):
            if called != tool:
                continue
            v = _json_pointer(arguments, pointer)
            if v is _MISSING:
                continue
            if isinstance(v, str):
                if compiled.search(v):
                    return make_score(
                        name, 1.0, True, f"{tool} called with {pointer} matching {pattern!r}"
                    )
            else:
                non_string = True
        if non_string:
            return make_score(name, 0.0, False, f"non-string value at {pointer} in {tool} arguments")
        return make_score(name, 0.0, False, f"no {tool} call with {pointer} matching {pattern!r}")
    return Scorer(name, fn)


def observation_contains(tool: str, needle: str) -> Scorer:
    """Passes when the observation content correlated to some invocation of
    `tool` (joined via `source_call_id`) contains `needle` as a substring
    (multimodal content is graded on its text projection). Requires the ATIF
    trajectory; a transcript without one fails."""
    name = f"observation_contains({tool}, {needle!r})"

    def fn(_sample, transcript) -> Score:
        trajectory = transcript.trajectory
        if trajectory is None:
            return make_score(name, 0.0, False, "subject reported no trajectory")
        for called, _arguments, content in _trajectory_invocations(trajectory):
            if called == tool and content is not None and needle in _content_text(content):
                return make_score(name, 1.0, True, f"{tool} observation contains {needle!r}")
        return make_score(name, 0.0, False, f"no {tool} observation contains {needle!r}")
    return Scorer(name, fn)


def steps_within(maximum: int) -> Scorer:
    """Passes when the trajectory has at most `maximum` steps — the ATIF
    step-count budget (distinct from `turns_within`, which counts
    subject-reported iterations). Requires the ATIF trajectory; a transcript
    without one fails."""
    name = f"steps_within({maximum})"

    def fn(_sample, transcript) -> Score:
        trajectory = transcript.trajectory
        if trajectory is None:
            return make_score(name, 0.0, False, "subject reported no trajectory")
        n = len(trajectory.steps)
        return _passfail(name, n <= maximum, f"{n} <= {maximum}", f"{n} > {maximum}")
    return Scorer(name, fn)


# ----- budget scorers -------------------------------------------------------

def cost_within(max_usd: float) -> Scorer:
    name = f"cost_within(${max_usd})"

    def fn(_sample, transcript) -> Score:
        c = transcript.usage.cost_usd
        return _passfail(name, c <= max_usd, f"${c:.4f} <= ${max_usd}", f"${c:.4f} > ${max_usd}")
    return Scorer(name, fn)


def tokens_within(maximum: int) -> Scorer:
    name = f"tokens_within({maximum})"

    def fn(_sample, transcript) -> Score:
        total = transcript.usage.input_tokens + transcript.usage.output_tokens
        return _passfail(name, total <= maximum, f"{total} <= {maximum} tokens",
                         f"{total} > {maximum} tokens")
    return Scorer(name, fn)


def output_tokens_within(maximum: int) -> Scorer:
    name = f"output_tokens_within({maximum})"

    def fn(_sample, transcript) -> Score:
        out = transcript.usage.output_tokens
        return _passfail(name, out <= maximum, f"{out} <= {maximum}", f"{out} > {maximum}")
    return Scorer(name, fn)


def latency_within(max_ms: int) -> Scorer:
    name = f"latency_within({max_ms}ms)"

    def fn(_sample, transcript) -> Score:
        ms = transcript.timing.duration_ms if transcript.timing else 0
        return _passfail(name, ms <= max_ms, f"{ms}ms <= {max_ms}ms", f"{ms}ms > {max_ms}ms")
    return Scorer(name, fn)


def ttft_within(max_ms: int) -> Scorer:
    name = f"ttft_within({max_ms}ms)"

    def fn(_sample, transcript) -> Score:
        ms = transcript.timing.time_to_first_token_ms if transcript.timing else None
        if ms is None:
            return make_score(name, 0.0, False, "subject did not report TTFT")
        return _passfail(name, ms <= max_ms, f"ttft {ms}ms <= {max_ms}ms",
                         f"ttft {ms}ms > {max_ms}ms")
    return Scorer(name, fn)


# ----- custom (open-vocabulary) metric scorers ------------------------------

def metric_within(metric: str, maximum: float) -> Scorer:
    name = f"metric_within({metric} <= {maximum})"

    def fn(_sample, transcript) -> Score:
        v = (transcript.metrics or {}).get(metric)
        if v is None:
            return make_score(name, 0.0, False, f"subject did not report {metric}")
        return _passfail(name, v <= maximum, f"{metric}={v} <= {maximum}", f"{metric}={v} > {maximum}")
    return Scorer(name, fn)


def metric_at_least(metric: str, minimum: float) -> Scorer:
    name = f"metric_at_least({metric} >= {minimum})"

    def fn(_sample, transcript) -> Score:
        v = (transcript.metrics or {}).get(metric)
        if v is None:
            return make_score(name, 0.0, False, f"subject did not report {metric}")
        return _passfail(name, v >= minimum, f"{metric}={v} >= {minimum}", f"{metric}={v} < {minimum}")
    return Scorer(name, fn)


# ----- JSON output scorers --------------------------------------------------

def json_valid() -> Scorer:
    name = "json_valid"

    def fn(_sample, transcript) -> Score:
        try:
            json.loads((transcript.final_response or "").strip())
            return make_score(name, 1.0, True, "valid JSON")
        except (ValueError, TypeError) as e:
            return make_score(name, 0.0, False, f"invalid JSON: {e}")
    return Scorer(name, fn)


def json_field_equals(key: str, value: str) -> Scorer:
    name = f"json_field_equals({key}={value!r})"

    def fn(_sample, transcript) -> Score:
        try:
            parsed = json.loads((transcript.final_response or "").strip())
        except (ValueError, TypeError):
            return make_score(name, 0.0, False, f"no JSON field {key}")
        if not isinstance(parsed, dict) or key not in parsed:
            return make_score(name, 0.0, False, f"no JSON field {key}")
        got = parsed[key]
        if isinstance(got, str) and got == value:
            return make_score(name, 1.0, True, f"{key} == {value!r}")
        return make_score(name, 0.0, False, f"{key} is {got}, expected {value!r}")
    return Scorer(name, fn)


# ----- multimodal output scorer ---------------------------------------------

def produced_modality(modality: str) -> Scorer:
    name = f"produced_modality({modality})"

    def fn(_sample, transcript) -> Score:
        ok = any((p or {}).get("kind") == modality for p in (transcript.output or []))
        return _passfail(name, ok, f"produced a {modality} part", f"no {modality} part in output")
    return Scorer(name, fn)


# ----- combinators ----------------------------------------------------------

def _glyph(s: Score) -> str:
    return "–" if s.na else ("✓" if s.pass_ else "✗")


def _combine(name: str, scorers: Sequence[Scorer], require_all: bool) -> Scorer:
    def fn(sample, transcript) -> Score:
        values: List[tuple] = []
        reasons: List[str] = []
        for sc in scorers:
            s = sc.score(sample, transcript)
            reasons.append(f"{_glyph(s)}{s.scorer}")
            if not s.na:
                values.append((s.value, s.pass_))
        reason = ", ".join(reasons)
        if not values:
            return make_score(name, 0.0, False, reason, na=True)
        if require_all:
            passed = all(p for _, p in values)
            value = sum(v for v, _ in values) / len(values)
        else:
            passed = any(p for _, p in values)
            value = max(v for v, _ in values)
        return make_score(name, value, passed, reason)
    return Scorer(name, fn)


def all_of(name: str, scorers: Sequence[Scorer]) -> Scorer:
    """Passes only if every inner scorer passes; value is their mean."""
    return _combine(name, scorers, require_all=True)


def any_of(name: str, scorers: Sequence[Scorer]) -> Scorer:
    """Passes if any inner scorer passes; value is the max."""
    return _combine(name, scorers, require_all=False)


def not_(inner: Scorer) -> Scorer:
    """Inverts a scorer; an N/A inner stays N/A (you can't invert 'unknown')."""
    name = f"not({inner.name})"

    def fn(sample, transcript) -> Score:
        s = inner.score(sample, transcript)
        if s.na:
            return make_score(f"not({s.scorer})", 0.0, False, f"inner N/A: {s.reason}", na=True)
        return make_score(f"not({s.scorer})", 1.0 - s.value, not s.pass_, f"inverted: {s.reason}")
    return Scorer(name, fn)


# ----- escape hatch ---------------------------------------------------------

def scorer(name: str, fn: Callable[["object", "object"], Union[bool, Score]]) -> Scorer:
    """Wrap an arbitrary predicate. The callable may return a bool (turned into a
    pass/fail Score) or a fully-formed Score for graded/NA control. This is a
    language-local escape hatch and intentionally has no cross-SDK parity."""

    def wrapped(sample, transcript) -> Score:
        out = fn(sample, transcript)
        if isinstance(out, Score):
            return out
        return make_score(name, 1.0 if out else 0.0, bool(out), "ok" if out else "failed")
    return Scorer(name, wrapped)
