// Protocol version, methods, and capability tokens — GENERATED, do not edit.
//
// Regenerate with `node codegen.mjs` from schema/v1/meta.json. CI runs
// `node codegen.mjs --check` to fail on drift.

export const PROTOCOL_VERSION = "1.1";
export const MIN_PROTOCOL_VERSION = "1.0";

export const METHODS = ["initialize", "list", "list_samples", "run", "execute", "score", "cancel"] as const;
export const CAPABILITIES = ["axes", "events", "usage", "execute", "score", "trials", "cancel", "paginate", "trajectory"] as const;
