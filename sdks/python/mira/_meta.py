"""Protocol version, methods, and capability tokens — GENERATED, do not edit.

Regenerate with `python3 codegen.py` from schema/v1/meta.json. CI runs
`codegen.py --check` to fail on drift.
"""

PROTOCOL_VERSION = "1.11"
MIN_PROTOCOL_VERSION = "1.0"

METHODS = ("initialize", "list", "list_samples", "run", "execute", "score", "cancel")
CAPABILITIES = ("axes", "events", "usage", "execute", "score", "trials", "cancel", "paginate")
