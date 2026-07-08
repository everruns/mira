"""Generic JSON <-> dataclass codec for the generated wire types.

Two contracts from docs/protocol.md drive this:
- **Ignore unknown fields** on decode (forward compatibility): a newer host may
  send fields this build doesn't know.
- **Omit empty optionals** on encode, mirroring the Rust `skip_serializing_if`,
  so a Python study's lines look like the Rust host's.
"""
from __future__ import annotations

import dataclasses
import typing
from typing import Any, get_args, get_origin

# Cache resolved type hints per class (get_type_hints is not cheap).
_HINTS: dict[type, dict[str, Any]] = {}


def _hints(cls: type) -> dict[str, Any]:
    if cls not in _HINTS:
        _HINTS[cls] = typing.get_type_hints(cls)
    return _HINTS[cls]


def _wire_name(f: dataclasses.Field) -> str:
    return f.metadata.get("wire", f.name)


def _is_empty(v: Any) -> bool:
    return v is None or v == "" or v == [] or v == {}


def _is_default_instance(v: Any) -> bool:
    """A nested dataclass still equal to its default construction (e.g. an
    untouched `Usage()`/`Timing()`). Optional fields holding one are dropped on
    encode, mirroring the Rust `skip_serializing_if` for such fields."""
    return dataclasses.is_dataclass(v) and not isinstance(v, type) and v == type(v)()


def to_dict(obj: Any) -> Any:
    """Dataclass -> JSON-able dict, dropping empty non-required fields."""
    if dataclasses.is_dataclass(obj) and not isinstance(obj, type):
        required = getattr(type(obj), "__required__", ())
        out: dict[str, Any] = {}
        for f in dataclasses.fields(obj):
            wire = _wire_name(f)
            v = getattr(obj, f.name)
            if wire in required or not (_is_empty(v) or _is_default_instance(v)):
                out[wire] = to_dict(v)
        return out
    if isinstance(obj, list):
        return [to_dict(v) for v in obj]
    if isinstance(obj, dict):
        return {k: to_dict(v) for k, v in obj.items()}
    return obj


def from_dict(cls: type, data: Any) -> Any:
    """JSON dict -> dataclass instance, ignoring unknown keys."""
    if not (dataclasses.is_dataclass(cls) and isinstance(data, dict)):
        return data
    hints = _hints(cls)
    by_wire = {_wire_name(f): f for f in dataclasses.fields(cls)}
    kwargs: dict[str, Any] = {}
    for key, value in data.items():
        f = by_wire.get(key)
        if f is None:  # unknown field — forward compatibility
            continue
        kwargs[f.name] = _decode(hints[f.name], value)
    return cls(**kwargs)


def _decode(ann: Any, value: Any) -> Any:
    if value is None:
        return None
    origin = get_origin(ann)
    if origin in (list, typing.List):
        (item,) = get_args(ann) or (Any,)
        return [_decode(item, v) for v in value]
    if origin in (dict, typing.Dict):
        args = get_args(ann)
        vtype = args[1] if len(args) == 2 else Any
        return {k: _decode(vtype, v) for k, v in value.items()}
    if origin is typing.Union:  # Optional[X] -> first non-None arg
        for arg in get_args(ann):
            if arg is not type(None):
                return _decode(arg, value)
        return value
    if origin is typing.Literal:
        return value
    if dataclasses.is_dataclass(ann):
        return from_dict(ann, value)
    return value
