"""Wire types for the Mira eval protocol — GENERATED, do not edit.

Regenerate with `python3 codegen.py` from schema/v1/schema.json (the same
language-neutral contract the Rust host is generated from). CI runs
`codegen.py --check` to fail on drift.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional

@dataclass
class Agent:
    extra: Dict[str, Any] = field(default_factory=dict)
    model_name: Optional[str] = None
    name: str = ""
    tool_definitions: List[Any] = field(default_factory=list)
    version: str = ""
    __required__ = ("name", "version")

@dataclass
class AxisInfo:
    name: str = ""
    values: List[str] = field(default_factory=list)
    __required__ = ("name", "values")

@dataclass
class CancelParams:
    id: int = 0
    __required__ = ("id",)

@dataclass
class CancelResult:
    cancelled: bool = False
    __required__ = ("cancelled",)

@dataclass
class ContentPart:
    extra: Dict[str, Any] = field(default_factory=dict)
    source: Optional["ImageSource"] = None
    text: Optional[str] = None
    type: str = ""
    __required__ = ("type",)

ErrorKind = Literal["subject", "infra"]

@dataclass
class EvalInfo:
    axes: List["AxisInfo"] = field(default_factory=list)
    description: str = ""
    max_turns: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)
    name: str = ""
    next_cursor: Optional[str] = None
    samples: List["SampleInfo"] = field(default_factory=list)
    scorers: List[str] = field(default_factory=list)
    seed: Optional[int] = None
    targets: List["TargetInfo"] = field(default_factory=list)
    trials: int = 0
    __required__ = ("name", "samples", "scorers", "targets")

@dataclass
class EventParams:
    eval: str = ""
    kind: str = ""
    params: Dict[str, str] = field(default_factory=dict)
    request_id: int = 0
    sample: str = ""
    target: str = ""
    text: Optional[str] = None
    tool: Optional[str] = None
    turn: Optional[int] = None
    __required__ = ("eval", "kind", "sample", "target")

@dataclass
class ExecuteResult:
    eval: str = ""
    params: Dict[str, str] = field(default_factory=dict)
    sample: str = ""
    seed: Optional[int] = None
    skipped: bool = False
    target: str = ""
    transcript: "Transcript" = field(default_factory=lambda: Transcript())
    trial: int = 0
    trials: int = 0
    __required__ = ("eval", "sample", "target", "transcript")

@dataclass
class FinalMetrics:
    extra: Dict[str, Any] = field(default_factory=dict)
    total_cached_tokens: Optional[int] = None
    total_completion_tokens: Optional[int] = None
    total_cost_usd: Optional[float] = None
    total_prompt_tokens: Optional[int] = None
    total_steps: Optional[int] = None
    __required__ = ()

@dataclass
class ImageSource:
    media_type: str = ""
    path: str = ""
    __required__ = ("media_type", "path")

@dataclass
class InitializeResult:
    capabilities: List[str] = field(default_factory=list)
    capability_params: Dict[str, Any] = field(default_factory=dict)
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
class ListSamplesParams:
    cursor: str = ""
    eval: str = ""
    __required__ = ("cursor", "eval")

@dataclass
class ListSamplesResult:
    next_cursor: Optional[str] = None
    samples: List["SampleInfo"] = field(default_factory=list)
    __required__ = ("samples",)

@dataclass
class LogParams:
    message: str = ""
    request_id: int = 0
    __required__ = ("message",)

@dataclass
class Notification:
    method: str = ""
    params: Any = None
    __required__ = ("method",)

@dataclass
class Observation:
    results: List["ObservationResult"] = field(default_factory=list)
    __required__ = ("results",)

@dataclass
class ObservationResult:
    content: Optional["StepContent"] = None
    extra: Dict[str, Any] = field(default_factory=dict)
    source_call_id: Optional[str] = None
    subagent_trajectory_ref: List["SubagentTrajectoryRef"] = field(default_factory=list)
    __required__ = ()

Part = Dict[str, Any]  # tagged union: kind in (text, image, audio, file, json)

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
    params: Dict[str, str] = field(default_factory=dict)
    sample: str = ""
    seed: Optional[int] = None
    target: str = ""
    trial: int = 0
    trials: int = 0
    __required__ = ("eval", "sample", "target")

@dataclass
class RunResult:
    aggregate: float = 0.0
    eval: str = ""
    expected: Any = None
    input: List[str] = field(default_factory=list)
    params: Dict[str, str] = field(default_factory=dict)
    passed: bool = False
    sample: str = ""
    scores: List["Score"] = field(default_factory=list)
    seed: Optional[int] = None
    skipped: bool = False
    target: str = ""
    transcript: "TranscriptSummary" = field(default_factory=lambda: TranscriptSummary())
    trial: int = 0
    trials: int = 0
    __required__ = ("aggregate", "eval", "passed", "sample", "scores", "target", "transcript")

@dataclass
class SampleInfo:
    id: str = ""
    metadata: Dict[str, Any] = field(default_factory=dict)
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
    params: Dict[str, str] = field(default_factory=dict)
    sample: str = ""
    seed: Optional[int] = None
    target: str = ""
    transcript: "Transcript" = field(default_factory=lambda: Transcript())
    trial: int = 0
    trials: int = 0
    __required__ = ("eval", "sample", "target", "transcript")

Source = Dict[str, Any]  # union of objects

@dataclass
class Step:
    extra: Dict[str, Any] = field(default_factory=dict)
    is_copied_context: Optional[bool] = None
    llm_call_count: Optional[int] = None
    message: "StepContent" = None
    metrics: Optional["StepMetrics"] = None
    model_name: Optional[str] = None
    observation: Optional["Observation"] = None
    reasoning_content: Optional[str] = None
    reasoning_effort: Any = None
    source: str = ""
    step_id: int = 0
    timestamp: Optional[str] = None
    tool_calls: List["ToolCall"] = field(default_factory=list)
    __required__ = ("source", "step_id")

StepContent = Any  # untagged union: str | List["ContentPart"]

@dataclass
class StepMetrics:
    cached_tokens: Optional[int] = None
    completion_token_ids: Optional[List[int]] = None
    completion_tokens: Optional[int] = None
    cost_usd: Optional[float] = None
    extra: Dict[str, Any] = field(default_factory=dict)
    logprobs: Optional[List[float]] = None
    prompt_token_ids: Optional[List[int]] = None
    prompt_tokens: Optional[int] = None
    __required__ = ()

@dataclass
class SubagentTrajectoryRef:
    extra: Dict[str, Any] = field(default_factory=dict)
    session_id: Optional[str] = None
    trajectory_id: Optional[str] = None
    trajectory_path: Optional[str] = None
    __required__ = ()

@dataclass
class TargetInfo:
    available: bool = False
    label: str = ""
    metadata: Dict[str, Any] = field(default_factory=dict)
    provider: str = ""
    __required__ = ("available", "label")

@dataclass
class Timing:
    duration_ms: int = 0
    time_to_first_token_ms: Optional[int] = None
    __required__ = ()

@dataclass
class ToolCall:
    arguments: Any = None
    extra: Dict[str, Any] = field(default_factory=dict)
    function_name: str = ""
    tool_call_id: str = ""
    __required__ = ("arguments", "function_name", "tool_call_id")

@dataclass
class Trajectory:
    agent: "Agent" = field(default_factory=lambda: Agent())
    continued_trajectory_ref: Optional[str] = None
    extra: Dict[str, Any] = field(default_factory=dict)
    final_metrics: Optional["FinalMetrics"] = None
    notes: Optional[str] = None
    schema_version: str = ""
    session_id: Optional[str] = None
    steps: List["Step"] = field(default_factory=list)
    subagent_trajectories: List["Trajectory"] = field(default_factory=list)
    trajectory_id: Optional[str] = None
    __required__ = ("agent", "schema_version", "steps")

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
    output: List["Part"] = field(default_factory=list)
    timing: "Timing" = field(default_factory=lambda: Timing())
    tool_calls: List[str] = field(default_factory=list)
    tool_calls_count: int = 0
    trajectory: Optional["Trajectory"] = None
    usage: "Usage" = field(default_factory=lambda: Usage())
    __required__ = ()

@dataclass
class TranscriptSummary:
    error: Optional[str] = None
    error_kind: "ErrorKind" = None
    final_response: str = ""
    iterations: int = 0
    metadata: Dict[str, Any] = field(default_factory=dict)
    metrics: Dict[str, float] = field(default_factory=dict)
    output: List["Part"] = field(default_factory=list)
    timing: "Timing" = field(default_factory=lambda: Timing())
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
