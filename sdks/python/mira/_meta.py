"""Protocol version, methods, and capability tokens — GENERATED, do not edit.

Regenerate with `python3 codegen.py` from schema/v1/meta.json. CI runs
`codegen.py --check` to fail on drift.
"""

PROTOCOL_VERSION = "1.9"
MIN_PROTOCOL_VERSION = "1.0"

METHODS = ("initialize", "list", "run", "execute", "score", "cancel")
CAPABILITIES = ("axes", "events", "usage", "execute", "score", "trials", "cancel")
