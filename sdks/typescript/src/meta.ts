// Protocol vocabulary — GENERATED, do not edit.
//
// Regenerate with `node codegen.mjs` from schema/v1/meta.json. CI runs
// `node codegen.mjs --check` to fail on drift.

export const PROTOCOL_VERSION = "1.1";
export const MIN_PROTOCOL_VERSION = "1.0";

// Every method in the protocol, either direction.
export const METHODS = ["initialize", "list", "list_samples", "run", "execute", "score", "cancel", "event", "log"] as const;
export const CAPABILITIES = ["axes", "events", "usage", "execute", "score", "trials", "cancel", "paginate", "trajectory"] as const;

// What a study answers: the methods the host sends.
export const SERVED_METHODS = ["initialize", "list", "list_samples", "run", "execute", "score", "cancel"] as const;
// What a study may send back: the notifications.
export const EMITTED_METHODS = ["event", "log"] as const;
// The capability a method needs, for the methods that need one.
export const REQUIRES: Readonly<Record<string, string>> = {
  "list_samples": "paginate",
  "execute": "execute",
  "score": "score",
  "cancel": "cancel",
  "event": "events"
};
