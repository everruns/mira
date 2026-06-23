// Encode authored wire objects into the exact JSON the Rust host emits.
//
// One contract from docs/protocol.md drives this: **omit empty optionals** on
// encode, mirroring the Rust `skip_serializing_if`, so a TypeScript study's
// lines look like the host's. Required fields survive even when empty (`0`,
// `false`, `""`, `[]`) — `WIRE_FIELDS` (generated from the schema) says which.
//
// Decoding needs no codec: `JSON.parse` already yields plain objects, and
// reading only known fields ignores unknown ones (forward compatibility) for
// free.
import { WIRE_FIELDS } from "./wire.js";

function isEmpty(v: unknown): boolean {
  if (v === null || v === undefined || v === "") return true;
  if (Array.isArray(v)) return v.length === 0;
  if (typeof v === "object") return Object.keys(v as object).length === 0;
  return false;
}

/**
 * A wire object of type `typeName` -> the JSON the host expects, dropping empty
 * non-required fields and recursing into nested object defs. Numbers `0`,
 * `false`, and `""` are kept when the field is required.
 */
export function toWire(typeName: string, obj: Record<string, unknown>): Record<string, unknown> {
  const fields = WIRE_FIELDS[typeName];
  if (!fields) return obj; // not a known def — pass through unchanged
  const out: Record<string, unknown> = {};
  for (const [key, meta] of Object.entries(fields)) {
    let value = obj[key];
    if (value !== null && value !== undefined) {
      if (meta.ref) {
        value = toWire(meta.ref, value as Record<string, unknown>);
      } else if (meta.arrayRef) {
        value = (value as Record<string, unknown>[]).map((v) => toWire(meta.arrayRef!, v));
      }
    }
    if (meta.required || !isEmpty(value)) out[key] = value;
  }
  return out;
}
