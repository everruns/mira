#!/usr/bin/env node
/**
 * Generate the protocol layer (src/wire.ts, src/meta.ts) from schema/v1/.
 *
 * The protocol layer is *derived* from the language-neutral contract the Rust
 * host is generated from (mira-schema-gen):
 *
 *   - `wire.ts` — the wire types (TypeScript interfaces) plus a runtime
 *     `WIRE_FIELDS` descriptor the codec uses, from `schema/v1/schema.json`.
 *   - `meta.ts` — the protocol version, method list, and capability tokens, from
 *     `schema/v1/meta.json`.
 *
 * So the SDK never hand-mirrors the Rust types, the protocol version, or the
 * method set, and can't drift from the wire. The TypeScript dual of the Rust
 * `--check` drift guard (and of the Python `codegen.py`).
 *
 *   node codegen.mjs            # rewrite the generated files
 *   node codegen.mjs --check    # exit 1 if any is stale (CI)
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCHEMA = join(HERE, "../../schema/v1/schema.json");
const META = join(HERE, "../../schema/v1/meta.json");
const OUT_WIRE = join(HERE, "src", "wire.ts");
const OUT_META = join(HERE, "src", "meta.ts");

const SCALAR = { string: "string", integer: "number", number: "number", boolean: "boolean" };

const refName = (ref) => ref.split("/").pop();

/** A $def we emit as an interface (vs. a `oneOf`/`anyOf` union). */
const isObjectDef = (defs, name) => {
  const d = defs[name] ?? {};
  return !("oneOf" in d) && !("anyOf" in d);
};

/** A `oneOf` whose members are all bare `const`s (e.g. ErrorKind) -> string union.
 * The other `oneOf` shape is a union of *objects* (a tagged union like Part, or
 * an externally-tagged enum like Source) -> a permissive object alias, since the
 * codec passes self-describing JSON objects through unchanged. */
const isConstEnum = (schema) => schema.oneOf.every((m) => "const" in m);

/** JSON-Schema fragment -> TypeScript type (as source text). */
function tsType(defs, schema) {
  if (schema === true || (schema && typeof schema === "object" && Object.keys(schema).length === 0))
    return "unknown";
  if (schema.$ref) return refName(schema.$ref);
  if (schema.anyOf) {
    const subs = schema.anyOf.filter((s) => s.type !== "null");
    const nullable = schema.anyOf.some((s) => s.type === "null");
    const inner = subs.length ? tsType(defs, subs[0]) : "unknown";
    return nullable ? `${inner} | null` : inner;
  }
  const t = schema.type;
  if (Array.isArray(t)) {
    // e.g. ["string", "null"] or ["array", "null"]
    const nonNull = t.filter((x) => x !== "null");
    const inner =
      nonNull[0] === "array"
        ? `${tsType(defs, schema.items ?? true)}[]`
        : (SCALAR[nonNull[0]] ?? "unknown");
    return t.includes("null") ? `${inner} | null` : inner;
  }
  if (t === "array") return `${tsType(defs, schema.items ?? true)}[]`;
  if (t === "object") {
    const ap = schema.additionalProperties ?? true;
    return `Record<string, ${tsType(defs, ap)}>`;
  }
  if (SCALAR[t]) return SCALAR[t];
  return "unknown";
}

/** Runtime descriptor for one property: whether it nests an object def the codec
 * must recurse into (and clean), and whether it is required (so a required-but-
 * empty field — `0`, `false`, `""` — is kept rather than dropped on encode). */
function fieldDesc(defs, schema, required) {
  const parts = [`required: ${required}`];
  if (schema && schema.$ref && isObjectDef(defs, refName(schema.$ref))) {
    parts.push(`ref: ${JSON.stringify(refName(schema.$ref))}`);
  } else if (schema && schema.type === "array" && schema.items && schema.items.$ref &&
             isObjectDef(defs, refName(schema.items.$ref))) {
    parts.push(`arrayRef: ${JSON.stringify(refName(schema.items.$ref))}`);
  }
  return `{ ${parts.join(", ")} }`;
}

function emitInterface(defs, name, schema) {
  const props = schema.properties ?? {};
  const required = new Set(schema.required ?? []);
  const lines = [`export interface ${name} {`];
  for (const [prop, pschema] of Object.entries(props)) {
    const opt = required.has(prop) ? "" : "?";
    const key = /^[A-Za-z_$][\w$]*$/.test(prop) ? prop : JSON.stringify(prop);
    lines.push(`  ${key}${opt}: ${tsType(defs, pschema)};`);
  }
  lines.push("}");
  return lines.join("\n") + "\n";
}

function emitConstEnum(name, schema) {
  const vals = schema.oneOf.map((m) => JSON.stringify(m.const)).join(" | ");
  return `export type ${name} = ${vals};\n`;
}

/** A top-level `anyOf` def — an *untagged* union of shapes (e.g. ATIF's
 * StepContent: a plain string OR a ContentPart array). TypeScript can express
 * it precisely; the codec passes the raw JSON value through unchanged. */
function emitUntaggedUnion(defs, name, schema) {
  const members = schema.anyOf.filter((m) => m.type !== "null").map((m) => tsType(defs, m));
  return `export type ${name} = ${members.join(" | ")};\n`;
}

function emitObjectUnion(name, schema) {
  const kinds = schema.oneOf
    .map((m) => m.properties?.kind?.const)
    .filter(Boolean);
  const note = kinds.length
    ? ` // tagged union: kind in (${kinds.join(", ")})`
    : " // union of objects";
  return `export type ${name} = Record<string, unknown>;${note}\n`;
}

function renderWire(schemaDoc) {
  const defs = schemaDoc.$defs;
  const out = [
    "// Wire types for the Mira eval protocol — GENERATED, do not edit.",
    "//",
    "// Regenerate with `node codegen.mjs` from schema/v1/schema.json (the same",
    "// language-neutral contract the Rust host is generated from). CI runs",
    "// `node codegen.mjs --check` to fail on drift.",
    "",
  ];
  const fieldTable = [];
  for (const name of Object.keys(defs).sort()) {
    const schema = defs[name];
    if (isObjectDef(defs, name)) {
      out.push(emitInterface(defs, name, schema));
      const required = new Set(schema.required ?? []);
      const entries = Object.entries(schema.properties ?? {}).map(
        ([prop, ps]) => `    ${JSON.stringify(prop)}: ${fieldDesc(defs, ps, required.has(prop))},`,
      );
      fieldTable.push(`  ${name}: {\n${entries.join("\n")}\n  },`);
    } else if ("anyOf" in schema) {
      out.push(emitUntaggedUnion(defs, name, schema));
    } else if (isConstEnum(schema)) {
      out.push(emitConstEnum(name, schema));
    } else {
      out.push(emitObjectUnion(name, schema));
    }
  }
  out.push(
    "/** Per-type field metadata the codec uses to clean wire objects on encode:",
    " * required fields survive even when empty; nested object defs are recursed.",
    " * GENERATED alongside the interfaces above. */",
    "export interface FieldMeta {",
    "  required: boolean;",
    "  ref?: string;",
    "  arrayRef?: string;",
    "}",
    "",
    "export const WIRE_FIELDS: Record<string, Record<string, FieldMeta>> = {",
    fieldTable.join("\n"),
    "};",
  );
  return out.join("\n").replace(/\n+$/, "\n");
}

function renderMeta(meta) {
  const tuple = (xs) => `[${xs.map((x) => JSON.stringify(x)).join(", ")}] as const`;
  return [
    "// Protocol version, methods, and capability tokens — GENERATED, do not edit.",
    "//",
    "// Regenerate with `node codegen.mjs` from schema/v1/meta.json. CI runs",
    "// `node codegen.mjs --check` to fail on drift.",
    "",
    `export const PROTOCOL_VERSION = ${JSON.stringify(meta.version)};`,
    `export const MIN_PROTOCOL_VERSION = ${JSON.stringify(meta.min_version)};`,
    "",
    `export const METHODS = ${tuple(meta.methods)};`,
    `export const CAPABILITIES = ${tuple(meta.capabilities)};`,
    "",
  ].join("\n");
}

function artifacts() {
  return [
    [OUT_WIRE, renderWire(JSON.parse(readFileSync(SCHEMA, "utf8")))],
    [OUT_META, renderMeta(JSON.parse(readFileSync(META, "utf8")))],
  ];
}

function main() {
  const check = process.argv.slice(2).includes("--check");
  const stale = [];
  for (const [path, body] of artifacts()) {
    if (check) {
      const current = existsSync(path) ? readFileSync(path, "utf8") : "";
      if (current !== body) stale.push(relative(HERE, path));
    } else {
      writeFileSync(path, body);
      console.log(`wrote ${relative(HERE, path)}`);
    }
  }
  if (check) {
    if (stale.length) {
      console.error(`stale (run \`node codegen.mjs\`): ${stale.join(", ")}`);
      return 1;
    }
    console.log("protocol layer up to date");
  }
  return 0;
}

process.exit(main());
