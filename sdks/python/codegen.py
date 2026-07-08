#!/usr/bin/env python3
"""Generate the protocol layer (mira/_wire.py, mira/_meta.py) from schema/v1/.

The protocol layer is *derived* from the language-neutral contract the Rust host
is generated from (mira-schema-gen):

- `_wire.py` — the wire types, from `schema/v1/schema.json`.
- `_meta.py` — the protocol version, method list, and capability tokens, from
  `schema/v1/meta.json`.

So the SDK never hand-mirrors the Rust types, the protocol version, or the method
set, and can't drift from the wire. Mirrors the Rust `--check` drift guard.

    python3 codegen.py            # rewrite the generated files
    python3 codegen.py --check    # exit 1 if any is stale (CI)
"""
from __future__ import annotations

import json
import keyword
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCHEMA = HERE / "../../schema/v1/schema.json"
META = HERE / "../../schema/v1/meta.json"
OUT_WIRE = HERE / "mira" / "_wire.py"
OUT_META = HERE / "mira" / "_meta.py"

SCALAR = {"string": "str", "integer": "int", "number": "float", "boolean": "bool"}
SCALAR_DEFAULT = {"str": '""', "int": "0", "float": "0.0", "bool": "False"}


def ref_name(schema: dict) -> str | None:
    ref = schema.get("$ref")
    return ref.rsplit("/", 1)[-1] if ref else None


def is_object_def(defs: dict, name: str) -> bool:
    """A $def we emit as a dataclass (vs. a union: a `oneOf` const enum like
    ErrorKind -> Literal, a `oneOf` object union like Part/Source -> dict alias,
    or an `anyOf` untagged union like StepContent -> permissive alias)."""
    d = defs.get(name, {})
    return "oneOf" not in d and "anyOf" not in d


def is_const_enum(schema: dict) -> bool:
    """A `oneOf` whose members are all bare `const`s (e.g. ErrorKind) -> Literal.
    The other `oneOf` shape is a union of *objects* (a tagged union like Part, or
    an externally-tagged enum like Source) -> emitted as a permissive dict alias,
    since the codec passes non-dataclass JSON objects through unchanged."""
    return all("const" in m for m in schema["oneOf"])


def py_type(defs: dict, schema) -> str:
    """JSON-Schema fragment -> Python type annotation (as source text)."""
    if schema is True or schema == {}:
        return "Any"
    if "$ref" in schema:
        return f'"{ref_name(schema)}"'
    if "anyOf" in schema:
        subs = [s for s in schema["anyOf"] if s.get("type") != "null"]
        nullable = any(s.get("type") == "null" for s in schema["anyOf"])
        inner = py_type(defs, subs[0]) if subs else "Any"
        return f"Optional[{inner}]" if nullable else inner
    t = schema.get("type")
    if isinstance(t, list):  # e.g. ["string", "null"] or ["array", "null"]
        non_null = [x for x in t if x != "null"]
        if non_null == ["array"]:
            inner = f"List[{py_type(defs, schema.get('items', True))}]"
        else:
            inner = SCALAR.get(non_null[0], "Any") if non_null else "Any"
        return f"Optional[{inner}]" if "null" in t else inner
    if t == "array":
        return f"List[{py_type(defs, schema.get('items', True))}]"
    if t == "object":
        ap = schema.get("additionalProperties", True)
        return f"Dict[str, {py_type(defs, ap)}]"
    if t in SCALAR:
        return SCALAR[t]
    return "Any"  # e.g. {"default": null} (free-form params)


def field_default(defs: dict, schema, required: bool) -> str | None:
    """Default expression for a field, or None for 'no default'. We give every
    field a default so dataclass field ordering is never a problem; __required__
    governs serialization, not construction."""
    ann = py_type(defs, schema)
    if ann.startswith("Optional[") or ann == "Any":
        return "None"
    if ann.startswith("List["):
        return "field(default_factory=list)"
    if ann.startswith("Dict["):
        return "field(default_factory=dict)"
    refn = ref_name(schema) if isinstance(schema, dict) else None
    if refn and is_object_def(defs, refn):
        # A nested message object (a bare, non-nullable $ref): default to a
        # real instance so e.g. Transcript() always carries a Usage(), even
        # where the wire leaves the field optional (the codec drops
        # still-default instances of optional fields on encode, mirroring the
        # Rust skip_serializing_if). A lambda defers name resolution so a
        # forward reference (def emitted later) still works.
        return f"field(default_factory=lambda: {refn}())"
    if refn:  # Literal alias (ErrorKind): optional, omitted when None.
        return "None"
    return SCALAR_DEFAULT.get(ann, "None")


def emit_literal(name: str, schema: dict) -> str:
    vals = ", ".join(json.dumps(c["const"]) for c in schema["oneOf"])
    return f"{name} = Literal[{vals}]\n"


def emit_untagged_union(name: str, schema: dict) -> str:
    """A top-level `anyOf` def — an *untagged* union of shapes (e.g. ATIF's
    StepContent: a plain string OR a ContentPart array). Carried permissively as
    `Any`: the codec passes the raw JSON value through unchanged, and the member
    shapes stay documented in schema/v1/schema.json."""
    members = " | ".join(py_type({}, m) for m in schema["anyOf"] if m.get("type") != "null")
    return f"{name} = Any  # untagged union: {members}\n"


def emit_object_union(name: str, schema: dict) -> str:
    """A `oneOf` of objects (Part's `kind`-tagged union, Source's externally-
    tagged enum). Each variant is a self-describing JSON object, so the SDK
    carries it as a plain dict — the codec serializes/deserializes it unchanged.
    The canonical shape lives in schema/v1/schema.json."""
    kinds = [m.get("properties", {}).get("kind", {}).get("const") for m in schema["oneOf"]]
    tags = ", ".join(k for k in kinds if k)
    note = f"  # tagged union: kind in ({tags})" if tags else "  # union of objects"
    return f"{name} = Dict[str, Any]{note}\n"


def emit_dataclass(defs: dict, name: str, schema: dict) -> str:
    props = schema.get("properties", {})
    required = set(schema.get("required", []))
    lines = ["@dataclass", f"class {name}:"]
    if not props:
        lines.append("    pass")
        return "\n".join(lines) + "\n"
    for prop, pschema in props.items():
        py_name = f"{prop}_" if keyword.iskeyword(prop) else prop
        ann = py_type(defs, pschema)
        default = field_default(defs, pschema, prop in required)
        meta = []
        if py_name != prop:
            meta.append(f'"wire": "{prop}"')
        if meta:  # carry wire name through dataclasses.field metadata
            if default and default.startswith("field("):
                default = default[:-1] + f", metadata={{{', '.join(meta)}}})"
            else:
                default = f"field(default={default}, metadata={{{', '.join(meta)}}})"
        lines.append(f"    {py_name}: {ann} = {default}")
    req = ", ".join(json.dumps(r) for r in sorted(required))
    lines.append(f"    __required__ = ({req}{',' if len(required) == 1 else ''})")
    return "\n".join(lines) + "\n"


def render_wire(schema_doc: dict) -> str:
    defs = schema_doc["$defs"]
    out = [
        '"""Wire types for the Mira eval protocol — GENERATED, do not edit.',
        "",
        "Regenerate with `python3 codegen.py` from schema/v1/schema.json (the same",
        "language-neutral contract the Rust host is generated from). CI runs",
        "`codegen.py --check` to fail on drift.",
        '"""',
        "from __future__ import annotations",
        "",
        "from dataclasses import dataclass, field",
        "from typing import Any, Dict, List, Literal, Optional",
        "",
    ]
    for name in sorted(defs):
        schema = defs[name]
        if is_object_def(defs, name):
            out.append(emit_dataclass(defs, name, schema))
        elif "anyOf" in schema:
            out.append(emit_untagged_union(name, schema))
        elif is_const_enum(schema):
            out.append(emit_literal(name, schema))
        else:
            out.append(emit_object_union(name, schema))
    return "\n".join(out).rstrip() + "\n"


def _str_tuple(items: list) -> str:
    body = ", ".join(json.dumps(i) for i in items)
    return f"({body},)" if len(items) == 1 else f"({body})"


def render_meta(meta_doc: dict) -> str:
    """The protocol version, methods, and capability tokens — so the SDK derives
    them from meta.json instead of hardcoding (which drifts on a minor bump)."""
    return "\n".join([
        '"""Protocol version, methods, and capability tokens — GENERATED, do not edit.',
        "",
        "Regenerate with `python3 codegen.py` from schema/v1/meta.json. CI runs",
        "`codegen.py --check` to fail on drift.",
        '"""',
        "",
        f"PROTOCOL_VERSION = {json.dumps(meta_doc['version'])}",
        f"MIN_PROTOCOL_VERSION = {json.dumps(meta_doc['min_version'])}",
        "",
        f"METHODS = {_str_tuple(meta_doc['methods'])}",
        f"CAPABILITIES = {_str_tuple(meta_doc['capabilities'])}",
    ]) + "\n"


def artifacts() -> list:
    """The (path, body) pairs that make up the generated protocol layer."""
    return [
        (OUT_WIRE, render_wire(json.loads(SCHEMA.read_text()))),
        (OUT_META, render_meta(json.loads(META.read_text()))),
    ]


def main() -> int:
    check = "--check" in sys.argv[1:]
    stale = []
    for path, body in artifacts():
        if check:
            if (path.read_text() if path.exists() else "") != body:
                stale.append(path.relative_to(HERE))
        else:
            path.write_text(body)
            print(f"wrote {path.relative_to(HERE)}")
    if check:
        if stale:
            print(f"stale (run `python3 codegen.py`): {', '.join(map(str, stale))}",
                  file=sys.stderr)
            return 1
        print("protocol layer up to date")
        return 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
