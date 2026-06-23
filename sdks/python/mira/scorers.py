"""Built-in scorers and the `scorer(...)` escape hatch.

A scorer maps `(sample, transcript) -> Score`. `value` is a continuous 0..1
signal; `pass_` the boolean verdict; `na=True` means "couldn't evaluate"
(excluded from the case verdict and aggregate), mirroring `mira::Score`.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Callable, Union

from ._wire import Score


def make_score(name: str, value: float, passed: bool, reason: str, na: bool = False) -> Score:
    return Score(scorer=name, value=value, pass_=passed, reason=reason, na=na)


@dataclass
class Scorer:
    """A named `(sample, transcript) -> Score`."""

    name: str
    fn: Callable[["object", "object"], Score]

    def score(self, sample, transcript) -> Score:
        return self.fn(sample, transcript)


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


def equals(target: str) -> Scorer:
    name = f'equals("{target}")'

    def fn(_sample, transcript) -> Score:
        ok = (transcript.final_response or "").strip() == target
        return make_score(name, 1.0 if ok else 0.0, ok,
                          "exact match" if ok else "mismatch")
    return Scorer(name, fn)


def regex(pattern: str) -> Scorer:
    name = f"regex({pattern!r})"
    compiled = re.compile(pattern)

    def fn(_sample, transcript) -> Score:
        ok = compiled.search(transcript.final_response or "") is not None
        return make_score(name, 1.0 if ok else 0.0, ok,
                          "matched" if ok else "no match")
    return Scorer(name, fn)


def scorer(name: str, fn: Callable[["object", "object"], Union[bool, Score]]) -> Scorer:
    """Wrap an arbitrary predicate. The callable may return a bool (turned into a
    pass/fail Score) or a fully-formed Score for graded/NA control."""

    def wrapped(sample, transcript) -> Score:
        out = fn(sample, transcript)
        if isinstance(out, Score):
            return out
        return make_score(name, 1.0 if out else 0.0, bool(out), "ok" if out else "failed")
    return Scorer(name, wrapped)
