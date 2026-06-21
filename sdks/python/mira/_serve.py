"""The study side of the protocol: an eval registry plus the stdio serve loop.

A `Study` answers `initialize`/`list`/`run`/`execute`/`score` over
newline-delimited JSON on stdio (see docs/protocol.md). stdout carries only
protocol JSON; logs go to stderr.
"""
from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Sequence

from . import _codec
from ._meta import PROTOCOL_VERSION
from ._wire import (
    AxisInfo,
    EvalInfo,
    ExecuteResult,
    InitializeResult,
    ListResult,
    ModelInfo,
    RunResult,
    SampleInfo,
    Score,
    Transcript,
    TranscriptSummary,
)
from .scorers import Scorer, make_score

# PROTOCOL_VERSION is imported from the generated `_meta` (above) — derived from
# schema/v1/meta.json, not hardcoded, so a version bump can't leave it stale.

# JSON-RPC error codes for the structured `error` object (mirrors the Rust
# `protocol::codes`). All caller mistakes here are non-retryable.
_CODE_METHOD_NOT_FOUND = -32601
_CODE_INVALID_PARAMS = -32602
_CODE_INTERNAL_ERROR = -32603

# The protocol methods this SDK dispatches in `Study.handle`. Kept explicit so a
# test can assert it covers every method in the generated `_meta.METHODS` — a new
# protocol method then fails CI until the serve loop handles it.
HANDLED_METHODS = ("initialize", "list", "run", "execute", "score")


# ----- authoring types --------------------------------------------------------

@dataclass
class Sample:
    """One dataset row. `prompt` is convenience for a single input turn; `input`
    holds multi-turn input. `text` joins them for the subject."""

    id: str
    prompt: Optional[str] = None
    input: List[str] = field(default_factory=list)
    tags: List[str] = field(default_factory=list)
    target: Optional[str] = None
    files: Dict[str, str] = field(default_factory=dict)
    metadata: Dict[str, Any] = field(default_factory=dict)

    @property
    def text(self) -> str:
        if self.prompt is not None:
            return self.prompt
        return "\n".join(self.input)


@dataclass
class Model:
    label: str
    provider: str = ""
    available: bool = True
    metadata: Dict[str, Any] = field(default_factory=dict)


def model(
    label: str,
    provider: str = "",
    available: bool = True,
    metadata: Optional[Dict[str, Any]] = None,
) -> Model:
    return Model(label=label, provider=provider, available=available,
                 metadata=dict(metadata or {}))


@dataclass
class RunCx:
    """Per-cell context handed to a subject: the matrix model, turn budget, and
    chosen axis values."""

    model: str
    provider: str = ""
    max_turns: int = 0
    params: Dict[str, str] = field(default_factory=dict)

    def param(self, name: str, default: str = "") -> str:
        return self.params.get(name, default)


Subject = Callable[[Sample, RunCx], Transcript]


@dataclass
class Eval:
    name: str
    subject: Subject
    samples: Sequence[Sample]
    models: Sequence[Model]
    scorers: Sequence[Scorer] = ()
    description: str = ""
    axes: Sequence[AxisInfo] = ()
    max_turns: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)

    def info(self) -> EvalInfo:
        return EvalInfo(
            name=self.name,
            description=self.description,
            samples=[SampleInfo(id=s.id, tags=list(s.tags), metadata=dict(s.metadata))
                     for s in self.samples],
            scorers=[sc.name for sc in self.scorers],
            models=[ModelInfo(label=m.label, provider=m.provider, available=m.available,
                              metadata=dict(m.metadata))
                    for m in self.models],
            axes=[AxisInfo(name=a.name, values=list(a.values)) for a in self.axes],
            max_turns=self.max_turns,
            metadata=dict(self.metadata),
        )

    def _sample(self, sid: str) -> Sample:
        for s in self.samples:
            if s.id == sid:
                return s
        raise ValueError(f"no such sample: {sid}")

    def _model(self, label: str) -> Model:
        for m in self.models:
            if m.label == label:
                return m
        return Model(label=label)


# ----- scoring (mirrors crate::runner) ----------------------------------------

def _score_transcript(ev: Eval, sample: Sample, transcript: Transcript) -> List[Score]:
    # Infra failure short-circuits to a single N/A, like score_transcript().
    if transcript.error is not None and transcript.error_kind == "infra":
        return [make_score("infra", 0.0, False, transcript.error, na=True)]
    return [sc.score(sample, transcript) for sc in ev.scorers]


def _verdict(scores: List[Score]) -> bool:
    applicable = [s for s in scores if not s.na]
    return bool(applicable) and all(s.pass_ for s in applicable)


def _aggregate(scores: List[Score]) -> float:
    applicable = [s.value for s in scores if not s.na]
    return sum(applicable) / len(applicable) if applicable else 0.0


# ----- study + serve loop -----------------------------------------------------

class Study:
    def __init__(self, name: str, version: Optional[str] = None) -> None:
        self.name = name
        self.version = version
        self._evals: Dict[str, Eval] = {}

    def add(self, ev: Eval) -> None:
        self._evals[ev.name] = ev

    def eval(self, **kw) -> Callable[[Subject], Subject]:
        """Decorator: register a subject function as an eval."""
        def deco(fn: Subject) -> Subject:
            self.add(Eval(name=kw.get("name", fn.__name__), subject=fn,
                          samples=kw["samples"], models=kw["models"],
                          scorers=kw.get("scorers", ()), description=kw.get("description", ""),
                          axes=kw.get("axes", ()), max_turns=kw.get("max_turns", 0),
                          metadata=kw.get("metadata", {})))
            return fn
        return deco

    def _capabilities(self) -> List[str]:
        caps = ["usage", "execute", "score"]
        if any(ev.axes for ev in self._evals.values()):
            caps.insert(0, "axes")
        return caps

    # --- method handlers ---
    def _execute(self, params: dict) -> tuple[Transcript, bool]:
        """Run one cell's subject. Returns (transcript, skipped); an unavailable
        model is skipped with an infra-error transcript (scored N/A, not failed)."""
        ev = self._evals[params["eval"]]
        sample = ev._sample(params["sample"])
        m = ev._model(params["model"])
        if not m.available:
            return Transcript(error=f"model unavailable: {m.label}", error_kind="infra"), True
        cx = RunCx(model=m.label, provider=m.provider, max_turns=ev.max_turns,
                   params=params.get("params", {}))
        return ev.subject(sample, cx), False

    def handle(self, method: str, params: dict) -> dict:
        if method == "initialize":
            return _codec.to_dict(InitializeResult(
                protocol_version=PROTOCOL_VERSION, study=self.name,
                evals=len(self._evals), study_version=self.version,
                capabilities=self._capabilities()))
        if method == "list":
            return _codec.to_dict(ListResult(evals=[e.info() for e in self._evals.values()]))
        if method == "execute":
            transcript, skipped = self._execute(params)
            return _codec.to_dict(ExecuteResult(
                eval=params["eval"], sample=params["sample"], model=params["model"],
                params=params.get("params", {}), transcript=transcript, skipped=skipped))
        if method in ("run", "score"):
            ev = self._evals[params["eval"]]
            sample = ev._sample(params["sample"])
            if method == "score":
                transcript, skipped = _codec.from_dict(Transcript, params["transcript"]), False
            else:
                transcript, skipped = self._execute(params)
            scores = _score_transcript(ev, sample, transcript)
            return _codec.to_dict(RunResult(
                eval=params["eval"], sample=params["sample"], model=params["model"],
                params=params.get("params", {}), passed=_verdict(scores),
                aggregate=_aggregate(scores), scores=scores,
                transcript=_summary(transcript), skipped=skipped))
        raise ValueError(f"unknown method: {method}")

    def serve(self, stdin=None, stdout=None) -> None:
        serve(self, stdin=stdin, stdout=stdout)


def _summary(t: Transcript) -> TranscriptSummary:
    return TranscriptSummary(
        final_response=t.final_response, iterations=t.iterations,
        tool_calls_count=t.tool_calls_count, tool_calls=list(t.tool_calls),
        usage=t.usage, timing=t.timing, metrics=dict(t.metrics),
        metadata=dict(t.metadata), error=t.error, error_kind=t.error_kind)


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def _rpc_error(exc: Exception) -> dict:
    """Build a JSON-RPC-shaped `error` object, classifying the failure so the host
    can distinguish a caller mistake from an internal one (mirrors the Rust side).
    All these are non-retryable, so `retryable` is left at its default `false`."""
    message = str(exc)
    if message.startswith("unknown method"):
        code = _CODE_METHOD_NOT_FOUND
    elif isinstance(exc, (KeyError, ValueError)):
        # Unknown eval/sample/model or a malformed request — the caller's mistake.
        code = _CODE_INVALID_PARAMS
        message = message.strip("'") if isinstance(exc, KeyError) else message
    else:
        code = _CODE_INTERNAL_ERROR
    return {"code": code, "message": message}


def serve(study: Study, stdin=None, stdout=None) -> None:
    """Drive `study` over newline-delimited JSON. One object per line in; one
    Response/Notification per line out. Loops until stdin EOF."""
    rin = stdin or sys.stdin
    out = stdout or sys.stdout

    def emit(obj: dict) -> None:
        out.write(json.dumps(obj) + "\n")
        out.flush()

    for line in rin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            emit({"method": "log", "params": {"message": "bad json"}})
            continue
        rid = msg.get("id")
        try:
            result = study.handle(msg.get("method"), msg.get("params") or {})
            emit({"id": rid, "result": result})
        except Exception as exc:  # report, don't crash the loop
            emit({"id": rid, "error": _rpc_error(exc)})
    log(f"{study.name}: stdin closed, exiting")
