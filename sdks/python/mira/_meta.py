"""Protocol vocabulary — GENERATED, do not edit.

Regenerate with `python3 codegen.py` from schema/v1/meta.json. CI runs
`codegen.py --check` to fail on drift.
"""

PROTOCOL_VERSION = "1.1"
MIN_PROTOCOL_VERSION = "1.0"

# Every method in the protocol, either direction.
METHODS = ("initialize", "list", "list_samples", "run", "execute", "score", "cancel", "event", "log")
CAPABILITIES = ("axes", "events", "usage", "execute", "score", "trials", "cancel", "paginate", "trajectory")

# What a study answers: the methods the host sends.
SERVED_METHODS = ("initialize", "list", "list_samples", "run", "execute", "score", "cancel")
# What a study may send back: the notifications.
EMITTED_METHODS = ("event", "log")
# The capability a method needs, for the methods that need one.
REQUIRES = {
    "cancel": "cancel",
    "event": "events",
    "execute": "execute",
    "list_samples": "paginate",
    "score": "score"
}
