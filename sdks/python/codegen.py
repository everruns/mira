#!/usr/bin/env python3
"""Generate mira/_wire.py from the canonical protocol JSON Schema.

The wire types are *derived* from schema/v<major>/schema.json — the same
language-neutral contract the Rust host is generated from (mira-schema-gen) —
so the Python SDK never hand-mirrors the Rust structs and can't drift from the
wire format. Mirrors the Rust `--check` drift guard.

    python3 codegen.py            # rewrite mira/_wire.py
    python3 codegen.py --check    # exit 1 if mira/_wire.py is stale (CI)
"""
from __future__ import annotations

import json
import keyword
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCHEMA = HERE / "../../schema/v1/schema.json"
OUT = HERE / "mira" / "_wire.py"

SCALAR = {"string": "str", "integer": "int", "number": "float", "boolean": "bool"}
SCALAR_DEFAULT = {"str": '""', "int": "0", "float": "0.0", "bool": "False"}


def ref_name(schema: dict) -> str | None:
    ref = schema.get("$ref")
    return ref.rsplit("/", 1)[-1] if ref else None


def is_object_def(defs: dict, name: str) -> bool:
    """A $def we emit as a dataclass (vs. ErrorKind, emitted as a Literal)."""
    return "oneOf" not in defs.get(name, {})


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
    if isinstance(t, list):  # e.g. ["string", "null"]
        non_null = [x for x in t if x != "null"]
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
        # A nested message: required ones get a real instance so e.g.
        # Transcript() carries a Usage(). A lambda defers name resolution so a
        # forward reference (def emitted later) still works.
        return f"field(default_factory=lambda: {refn}())" if required else "None"
    if refn:  # Literal alias (ErrorKind): optional, omitted when None.
        return "None"
    return SCALAR_DEFAULT.get(ann, "None")


def emit_literal(name: str, schema: dict) -> str:
    vals = ", ".join(json.dumps(c["const"]) for c in schema["oneOf"])
    return f"{name} = Literal[{vals}]\n"


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


def render(schema_doc: dict) -> str:
    defs = schema_doc["$defs"]
    version = schema_doc.get("description", "")
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
        else:
            out.append(emit_literal(name, schema))
    return "\n".join(out).rstrip() + "\n"


def main() -> int:
    schema_doc = json.loads(SCHEMA.read_text())
    generated = render(schema_doc)
    check = "--check" in sys.argv[1:]
    if check:
        current = OUT.read_text() if OUT.exists() else ""
        if current != generated:
            print(
                f"{OUT.relative_to(HERE)} is stale; run `python3 codegen.py`",
                file=sys.stderr,
            )
            return 1
        print("wire types up to date")
        return 0
    OUT.write_text(generated)
    print(f"wrote {OUT.relative_to(HERE)} ({len(schema_doc['$defs'])} types)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
