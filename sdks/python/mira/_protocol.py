"""Typed study surface and dispatch — GENERATED, do not edit.

Regenerate with `python3 codegen.py` from schema/v1/. CI runs
`codegen.py --check` to fail on drift.
"""
from __future__ import annotations

from typing import Any, Dict

from . import _codec
from ._wire import CancelParams, CancelResult, ExecuteResult, InitializeParams, InitializeResult, ListResult, ListSamplesParams, ListSamplesResult, RunParams, RunResult, ScoreParams


class MethodNotFound(Exception):
    """A method this study does not answer. Becomes a -32601 response."""

    def __init__(self, method: str) -> None:
        super().__init__(f"unknown method: {method}")
        self.method = method


class StudyHandler:
    """What a study answers: the methods the host sends.

    Every method defaults to `MethodNotFound`, so a study implements only
    what it actually handles. Adding a method to the protocol adds it here,
    which is how an unhandled one shows up as a refusal rather than as a
    silently missing branch in a hand-written dispatch chain.
    """

    def initialize(self, params: InitializeParams) -> InitializeResult:
        """Announce the host and learn what the study is and can do."""
        raise MethodNotFound("initialize")

    def list(self) -> ListResult:
        """The eval catalogue, with the first page of each eval's samples inline."""
        raise MethodNotFound("list")

    def list_samples(self, params: ListSamplesParams) -> ListSamplesResult:
        """The next page of one eval's samples, for datasets too large (or too lazy) to enumerate in one `list`."""
        raise MethodNotFound("list_samples")

    def run(self, params: RunParams) -> RunResult:
        """Execute and score one matrix case in a single call."""
        raise MethodNotFound("run")

    def execute(self, params: RunParams) -> ExecuteResult:
        """Execute one case's subject without scoring, returning the full transcript, for run-now-score-later workflows."""
        raise MethodNotFound("execute")

    def score(self, params: ScoreParams) -> RunResult:
        """Score a supplied transcript without re-executing the subject, for deferred scoring and re-scoring."""
        raise MethodNotFound("score")

    def cancel(self, params: CancelParams) -> CancelResult:
        """Abort one in-flight `run`/`execute`/`score` by its request `id`.  A request rather than a notification, and acknowledged: the host learns whether the run was still in flight. That divergence from other protocols, where cancel is fire-and-forget, is why lanok's cancellation is a hook the protocol fills rather than a setting."""
        raise MethodNotFound("cancel")


def dispatch(handler: StudyHandler, method: str, params: Dict[str, Any]) -> Any:
    """Decode `params`, call the handler, encode the result.

    Exhaustive over the declaration: a method that is not in it raises
    `MethodNotFound` rather than falling through to something else.
    """
    if method == "initialize":
        return _codec.to_dict(handler.initialize(_codec.from_dict(InitializeParams, params or {})))
    if method == "list":
        return _codec.to_dict(handler.list())
    if method == "list_samples":
        return _codec.to_dict(handler.list_samples(_codec.from_dict(ListSamplesParams, params or {})))
    if method == "run":
        return _codec.to_dict(handler.run(_codec.from_dict(RunParams, params or {})))
    if method == "execute":
        return _codec.to_dict(handler.execute(_codec.from_dict(RunParams, params or {})))
    if method == "score":
        return _codec.to_dict(handler.score(_codec.from_dict(ScoreParams, params or {})))
    if method == "cancel":
        return _codec.to_dict(handler.cancel(_codec.from_dict(CancelParams, params or {})))
    raise MethodNotFound(method)
