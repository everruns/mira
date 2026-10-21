#!/usr/bin/env python3
"""Generate the protocol layer (mira/_wire.py, _meta.py, _protocol.py) from schema/v1/.

The protocol layer is *derived* from the language-neutral contract the Rust host
is generated from (mira-schema-gen):

- `_wire.py` — the wire types, from `schema/v1/schema.json`.
- `_meta.py` — the protocol version, method table, and capability tokens, from
  `schema/v1/meta.json`.
- `_protocol.py` — the typed handler base and the dispatcher, from both: the
  method table says which methods a study answers and what each one carries,
  and the schema defines those payload types.

So the SDK never hand-mirrors the Rust types, the protocol version, the method
set, or the decode/call/encode dance, and can't drift from the wire. Mirrors the
Rust `--check` drift guard.

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
OUT_PROTOCOL = HERE / "mira" / "_protocol.py"

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
    """The protocol vocabulary — version, methods, capability tokens — so the SDK
    derives them from meta.json instead of hardcoding (which drifts on a minor
    bump).

    `meta.json` describes each method, not just its name: which side sends it,
    whether it expects a response, and the capability it needs. That is what
    lets the serve loop derive the set it dispatches (the methods a *host*
    sends) instead of keeping a hand-written list beside it, and what makes the
    coverage test direction-aware — a study answers requests, it does not answer
    its own `event`/`log` notifications.
    """
    methods = meta_doc["methods"]
    served = [m["name"] for m in methods if m["direction"] == "initiator"]
    emitted = [m["name"] for m in methods if m["direction"] == "responder"]
    requires = {m["name"]: m["requires"] for m in methods if m.get("requires")}
    return "\n".join([
        '"""Protocol vocabulary — GENERATED, do not edit.',
        "",
        "Regenerate with `python3 codegen.py` from schema/v1/meta.json. CI runs",
        "`codegen.py --check` to fail on drift.",
        '"""',
        "",
        f"PROTOCOL_VERSION = {json.dumps(meta_doc['version'])}",
        f"MIN_PROTOCOL_VERSION = {json.dumps(meta_doc['min_version'])}",
        "",
        "# Every method in the protocol, either direction.",
        f"METHODS = {_str_tuple([m['name'] for m in methods])}",
        f"CAPABILITIES = {_str_tuple(meta_doc['capabilities'])}",
        "",
        "# What a study answers: the methods the host sends.",
        f"SERVED_METHODS = {_str_tuple(served)}",
        "# What a study may send back: the notifications.",
        f"EMITTED_METHODS = {_str_tuple(emitted)}",
        "# The capability a method needs, for the methods that need one.",
        f"REQUIRES = {json.dumps(requires, indent=4, sort_keys=True)}",
    ]) + "\n"


def render_protocol(meta_doc: dict) -> str:
    """The typed study surface: one method per protocol method a host sends, and
    the dispatcher that decodes params, calls it, and encodes the result.

    This is what `meta.json` carrying each method's payload *types* buys. With
    only a name list a generator can emit string constants and leave the author
    a dict; with the types it can emit `def run(self, params: RunParams) ->
    RunResult`, and do the decode/encode once here instead of once per method in
    a hand-written dispatch chain.

    Every method defaults to raising `MethodNotFound`, so a study implements
    only what it answers and an unimplemented method refuses politely rather
    than looking like a crash.
    """
    served = [m for m in meta_doc["methods"] if m["direction"] == "initiator"]
    used = sorted({t for m in served for t in (m.get("params"), m.get("result")) if t})

    out = [
        '"""Typed study surface and dispatch — GENERATED, do not edit.',
        "",
        "Regenerate with `python3 codegen.py` from schema/v1/. CI runs",
        "`codegen.py --check` to fail on drift.",
        '"""',
        "from __future__ import annotations",
        "",
        "from typing import Any, Dict",
        "",
        "from . import _codec",
        f"from ._wire import {', '.join(used)}",
        "",
        "",
        "class MethodNotFound(Exception):",
        '    """A method this study does not answer. Becomes a -32601 response."""',
        "",
        "    def __init__(self, method: str) -> None:",
        '        super().__init__(f"unknown method: {method}")',
        "        self.method = method",
        "",
        "",
        "class StudyHandler:",
        '    """What a study answers: the methods the host sends.',
        "",
        "    Every method defaults to `MethodNotFound`, so a study implements only",
        "    what it actually handles. Adding a method to the protocol adds it here,",
        "    which is how an unhandled one shows up as a refusal rather than as a",
        "    silently missing branch in a hand-written dispatch chain.",
        '    """',
    ]
    for m in served:
        name, ident = m["name"], m["name"]
        result = m.get("result")
        sig_params = f", params: {m['params']}" if m.get("params") else ""
        ret = result or "None"
        doc = m.get("doc", "").strip()
        out += [
            "",
            f"    def {ident}(self{sig_params}) -> {ret}:",
            f'        """{doc}"""' if doc else None,
            f'        raise MethodNotFound("{name}")',
        ]

    out += [
        "",
        "",
        "def dispatch(handler: StudyHandler, method: str, params: Dict[str, Any]) -> Any:",
        '    """Decode `params`, call the handler, encode the result.',
        "",
        "    Exhaustive over the declaration: a method that is not in it raises",
        "    `MethodNotFound` rather than falling through to something else.",
        '    """',
    ]
    for m in served:
        name = m["name"]
        if m.get("params"):
            call = f"handler.{name}(_codec.from_dict({m['params']}, params or {{}}))"
        else:
            call = f"handler.{name}()"
        out += [
            f'    if method == "{name}":',
            f"        return _codec.to_dict({call})",
        ]
    out += ["    raise MethodNotFound(method)"]

    return "\n".join(line for line in out if line is not None) + "\n"


def artifacts() -> list:
    """The (path, body) pairs that make up the generated protocol layer."""
    meta_doc = json.loads(META.read_text())
    return [
        (OUT_WIRE, render_wire(json.loads(SCHEMA.read_text()))),
        (OUT_META, render_meta(meta_doc)),
        (OUT_PROTOCOL, render_protocol(meta_doc)),
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
