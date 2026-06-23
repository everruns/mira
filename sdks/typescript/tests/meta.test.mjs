// The SDK's protocol coverage must track the generated `meta` (from
// schema/v1/meta.json). These guard the gaps wire-type codegen alone can't: the
// protocol version, the method set, and the capability vocabulary.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { Study, sample, target, succeeded, axis, transcript, usage } from "../dist/index.js";
import { HANDLED_METHODS } from "../dist/study.js";
import { PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, METHODS, CAPABILITIES } from "../dist/meta.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const META = JSON.parse(readFileSync(join(HERE, "../../../schema/v1/meta.json"), "utf8"));

test("generated meta matches source", () => {
  assert.equal(PROTOCOL_VERSION, META.version);
  assert.equal(MIN_PROTOCOL_VERSION, META.min_version);
  assert.deepEqual(new Set(METHODS), new Set(META.methods));
  assert.deepEqual(new Set(CAPABILITIES), new Set(META.capabilities));
});

test("serve handles every protocol method", () => {
  // A new method in the protocol must be dispatched by the serve loop — not
  // silently unhandled. (`events` is a notification kind, not a method.)
  const handled = new Set(HANDLED_METHODS);
  for (const m of METHODS) assert.ok(handled.has(m), `unhandled method: ${m}`);
});

test("handled methods actually dispatch", async () => {
  const s = new Study("t");
  s.eval({
    name: "e",
    samples: [sample("x", { prompt: "p" })],
    targets: [target("sim")],
    scorers: [succeeded()],
    run: () => transcript("ok", { usage: usage({ inputTokens: 1, outputTokens: 1 }) }),
  });
  const base = { eval: "e", sample: "x", target: "sim" };
  const ex = await s.handle("execute", base);
  const payloads = {
    initialize: {},
    list: {},
    list_samples: { eval: "e", cursor: "0" },
    run: base,
    execute: base,
    score: { ...base, transcript: ex.transcript },
    cancel: { id: 1 },
  };
  for (const method of HANDLED_METHODS) {
    await s.handle(method, payloads[method]); // must not throw "unknown method"
  }
});

test("advertised capabilities are known tokens", async () => {
  const s = new Study("t");
  s.eval({
    name: "e",
    samples: [sample("x", { prompt: "p" })],
    targets: [target("sim")],
    scorers: [succeeded()],
    axes: [axis("effort", ["low", "high"])],
    run: () => transcript("ok", { usage: usage({ inputTokens: 1, outputTokens: 1 }) }),
  });
  const advertised = (await s.handle("initialize", {})).capabilities;
  const known = new Set(CAPABILITIES);
  for (const c of advertised) assert.ok(known.has(c), `unknown capability: ${c}`);
  assert.ok(advertised.includes("axes"));
});

test("protocol version is reported", async () => {
  const init = await new Study("t").handle("initialize", {});
  assert.equal(init.protocol_version, PROTOCOL_VERSION);
});
