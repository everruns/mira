"""ATIF trajectory helpers: parse, project, and build trajectory-only transcripts.

`Transcript.trajectory` (generated from schema/v1/) is the protocol's **primary
structured trajectory contract** — an ATIF document (harbor RFC 0001). A study
may return a transcript that sets *only* `trajectory`: the serve loop projects
the flat fields (`final_response`, `tool_calls`, `tool_calls_count`,
`iterations`, `usage`) from it automatically, so the built-in name-based scorers
keep working with zero calls on the study author's part. `events` is optional
and fully independent of the trajectory.

PARITY — source of truth: `crates/mira-eval/src/trajectory.rs`.
`project_into` here hand-mirrors `Trajectory::project_into` (fill-if-default,
never overwrite an explicitly set flat field), and `parse_trajectory` mirrors
`Trajectory::from_value` (accept any ATIF-v1.x, reject other prefixes with an
error). Behaviour is pinned by the shared vectors in
`schema/v1/conformance/trajectory.json`, run by
`tests/test_trajectory_conformance.py` (and the Rust + TypeScript twins).
"""
from __future__ import annotations

from typing import Any, Optional

from . import _codec
from ._wire import Trajectory, Transcript, Usage

# The ATIF schema version this SDK emits. Parsing is more lenient: any
# ATIF-v1.x document is accepted — this is not a ceiling on what is read.
ATIF_VERSION = "ATIF-v1.7"
ATIF_FORMAT = "ATIF"


def is_supported_schema_version(version: str) -> bool:
    """True when `version` names an ATIF v1 document this SDK can read."""
    return version == "ATIF-v1" or (isinstance(version, str) and version.startswith("ATIF-v1."))


def parse_trajectory(data: Any) -> Trajectory:
    """Parse one ATIF document (a JSON-decoded dict) into a `Trajectory`.

    Lenient within v1 (unknown fields are ignored by the codec); a non-v1
    `schema_version` raises `ValueError` — an error the serve loop can surface,
    never a crash on untrusted agent output.
    """
    if not isinstance(data, dict):
        raise ValueError("invalid ATIF trajectory: not a JSON object")
    trajectory = _codec.from_dict(Trajectory, data)
    if not is_supported_schema_version(trajectory.schema_version):
        raise ValueError(
            f"unsupported ATIF schema_version {trajectory.schema_version!r}: "
            f"this SDK reads ATIF-v1.x (emits {ATIF_VERSION})"
        )
    return trajectory


def _content_text(content: Any) -> str:
    """The text projection of a StepContent value: the string itself, or the
    `text` parts joined by newlines (image parts skipped)."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            p["text"] for p in content
            if isinstance(p, dict) and p.get("type") == "text" and isinstance(p.get("text"), str)
        )
    return ""


def _final_agent_text(trajectory: Trajectory) -> Optional[str]:
    for step in reversed(list(trajectory.steps)):
        if step.source == "agent":
            return _content_text(step.message)
    return None


def _tool_call_names(trajectory: Trajectory) -> list:
    return [call.function_name for step in trajectory.steps for call in step.tool_calls]


def _agent_iterations(trajectory: Trajectory) -> int:
    return sum(1 for s in trajectory.steps if s.source == "agent" and s.llm_call_count != 0)


def _extra_u64(extra: Optional[dict], key: str) -> int:
    v = (extra or {}).get(key)
    return v if isinstance(v, int) and not isinstance(v, bool) and v >= 0 else 0


def trajectory_usage(trajectory: Trajectory) -> Usage:
    """The `Usage` projection: `final_metrics` when present, else the per-step
    sum. `reasoning_tokens` is read from the corresponding `extra` map (ATIF has
    no first-class slot for it)."""
    fm = trajectory.final_metrics
    if fm is not None:
        return Usage(
            input_tokens=fm.total_prompt_tokens or 0,
            output_tokens=fm.total_completion_tokens or 0,
            cache_read_tokens=fm.total_cached_tokens or 0,
            reasoning_tokens=_extra_u64(fm.extra, "reasoning_tokens"),
            cost_usd=fm.total_cost_usd or 0.0,
        )
    usage = Usage()
    for step in trajectory.steps:
        m = step.metrics
        if m is None:
            continue
        usage.input_tokens += m.prompt_tokens or 0
        usage.output_tokens += m.completion_tokens or 0
        usage.cache_read_tokens += m.cached_tokens or 0
        usage.reasoning_tokens += _extra_u64(m.extra, "reasoning_tokens")
        usage.cost_usd += m.cost_usd or 0.0
    return usage


def project_into(trajectory: Trajectory, transcript: Transcript) -> None:
    """Fill the transcript's flat fields from `trajectory` — fill-if-default,
    so a flat field the study set explicitly is never overwritten. Mirrors
    `Trajectory::project_into` (see the module docstring for the rules)."""
    if not transcript.final_response:
        text = _final_agent_text(trajectory)
        if text is not None:
            transcript.final_response = text
    if not transcript.tool_calls:
        transcript.tool_calls = _tool_call_names(trajectory)
    if transcript.tool_calls_count == 0:
        transcript.tool_calls_count = len(transcript.tool_calls)
    if transcript.iterations == 0:
        transcript.iterations = _agent_iterations(trajectory)
    if transcript.usage is None or transcript.usage == Usage():
        transcript.usage = trajectory_usage(trajectory)


def from_trajectory(trajectory: Trajectory) -> Transcript:
    """A transcript built from a trajectory alone — the zero-burden path: the
    flat fields are projected automatically, and nothing else (not `events`)
    is required of the study."""
    transcript = Transcript()
    project_into(trajectory, transcript)
    transcript.trajectory = trajectory
    return transcript


def normalize(transcript: Transcript) -> Transcript:
    """Project `transcript.trajectory` (if any) into flat fields still at their
    defaults. The serve loop applies this to every produced or received
    transcript, so studies never call it themselves."""
    if transcript.trajectory is not None:
        project_into(transcript.trajectory, transcript)
    return transcript
