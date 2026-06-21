"""Wire types for the Mira eval protocol — GENERATED, do not edit.

Regenerate with `python3 codegen.py` from schema/v1/schema.json (the same
language-neutral contract the Rust host is generated from). CI runs
`codegen.py --check` to fail on drift.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional

@dataclass
class AxisInfo:
    name: str = ""
    values: List[str] = field(default_factory=list)
    __required__ = ("name", "values")

ErrorKind = Literal["subject", "infra"]

@dataclass
class EvalInfo:
    axes: List["AxisInfo"] = field(default_factory=list)
    description: str = ""
    max_turns: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)
    models: List["ModelInfo"] = field(default_factory=list)
    name: str = ""
    samples: List["SampleInfo"] = field(default_factory=list)
    scorers: List[str] = field(default_factory=list)
    __required__ = ("models", "name", "samples", "scorers")

@dataclass
class ExecuteResult:
    eval: str = ""
    model: str = ""
    params: Dict[str, str] = field(default_factory=dict)
    sample: str = ""
    skipped: bool = False
    transcript: "Transcript" = field(default_factory=lambda: Transcript())
    __required__ = ("eval", "model", "sample", "transcript")

@dataclass
class InitializeResult:
    capabilities: List[str] = field(default_factory=list)
    evals: int = 0
    protocol_version: str = ""
    study: str = ""
    study_version: Optional[str] = None
    __required__ = ("evals", "protocol_version", "study")

@dataclass
class ListResult:
    evals: List["EvalInfo"] = field(default_factory=list)
    __required__ = ("evals",)

@dataclass
class ModelInfo:
    available: bool = False
    label: str = ""
    provider: str = ""
    __required__ = ("available", "label")

@dataclass
class Notification:
    method: str = ""
    params: Any = None
    __required__ = ("method",)

@dataclass
class Request:
    id: int = 0
    method: str = ""
    params: Any = None
    __required__ = ("id", "method")

@dataclass
class Response:
    error: Optional["RpcError"] = None
    id: int = 0
    result: Any = None
    __required__ = ("id",)

@dataclass
class RpcError:
    code: int = 0
    data: Any = None
    message: str = ""
    retryable: bool = False
    __required__ = ("message",)

@dataclass
class RunParams:
    eval: str = ""
    model: str = ""
    params: Dict[str, str] = field(default_factory=dict)
    sample: str = ""
    __required__ = ("eval", "model", "sample")

@dataclass
class RunResult:
    aggregate: float = 0.0
    eval: str = ""
    model: str = ""
    params: Dict[str, str] = field(default_factory=dict)
    passed: bool = False
    sample: str = ""
    scores: List["Score"] = field(default_factory=list)
    skipped: bool = False
    transcript: "TranscriptSummary" = field(default_factory=lambda: TranscriptSummary())
    __required__ = ("aggregate", "eval", "model", "passed", "sample", "scores", "transcript")

@dataclass
class SampleInfo:
    id: str = ""
    tags: List[str] = field(default_factory=list)
    __required__ = ("id",)

@dataclass
class Score:
    na: bool = False
    pass_: bool = field(default=False, metadata={"wire": "pass"})
    reason: str = ""
    scorer: str = ""
    value: float = 0.0
    __required__ = ("pass", "reason", "scorer", "value")

@dataclass
class ScoreParams:
    eval: str = ""
    model: str = ""
    params: Dict[str, str] = field(default_factory=dict)
    sample: str = ""
    transcript: "Transcript" = field(default_factory=lambda: Transcript())
    __required__ = ("eval", "model", "sample", "transcript")

@dataclass
class Timing:
    duration_ms: int = 0
    time_to_first_token_ms: Optional[int] = None
    __required__ = ()

@dataclass
class Transcript:
    error: Optional[str] = None
    error_kind: "ErrorKind" = None
    events: List[Any] = field(default_factory=list)
    files: Dict[str, str] = field(default_factory=dict)
    final_response: str = ""
    iterations: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)
    metrics: Dict[str, float] = field(default_factory=dict)
    timing: "Timing" = None
    tool_calls: List[str] = field(default_factory=list)
    tool_calls_count: int = 0
    usage: "Usage" = field(default_factory=lambda: Usage())
    __required__ = ("final_response", "iterations", "tool_calls_count", "usage")

@dataclass
class TranscriptSummary:
    error: Optional[str] = None
    error_kind: "ErrorKind" = None
    final_response: str = ""
    iterations: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)
    metrics: Dict[str, float] = field(default_factory=dict)
    timing: "Timing" = None
    tool_calls: List[str] = field(default_factory=list)
    tool_calls_count: int = 0
    usage: "Usage" = field(default_factory=lambda: Usage())
    __required__ = ("final_response", "iterations", "tool_calls_count", "usage")

@dataclass
class Usage:
    cache_read_tokens: int = 0
    cost_usd: float = 0.0
    input_tokens: int = 0
    output_tokens: int = 0
    reasoning_tokens: int = 0
    __required__ = ("cost_usd", "input_tokens", "output_tokens")
