// Every message the SDK emits must validate against the canonical JSON Schema
// (schema/v1/schema.json) — the same contract the Rust host is generated from.
// This is the cross-language drift guard, the TypeScript dual of the Python
// conformance suite and of mira-schema-gen's validation.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import Ajv2020Pkg from "ajv/dist/2020.js";

import { Study, sample, target, succeeded, contains, transcript, usage, axis } from "../dist/index.js";

const Ajv2020 = Ajv2020Pkg.default ?? Ajv2020Pkg.Ajv2020;
const HERE = dirname(fileURLToPath(import.meta.url));
const SCHEMA = JSON.parse(readFileSync(join(HERE, "../../../schema/v1/schema.json"), "utf8"));

const ajv = new Ajv2020({ strict: false, allErrors: true });

function validator(defName) {
  return ajv.compile({ $schema: SCHEMA.$schema, $defs: SCHEMA.$defs, $ref: `#/$defs/${defName}` });
}

function check(defName, value) {
  const v = validator(defName);
  assert.ok(v(value), `${defName}: ${ajv.errorsText(v.errors)}`);
}

function buildStudy() {
  const s = new Study("conformance", { version: "0.1.0" });
  s.eval({
    name: "greet",
    samples: [sample("hi", { prompt: "hi", tags: ["smoke"] })],
    targets: [target("sim"), target("anthropic/x", { provider: "anthropic", available: false })],
    scorers: [succeeded(), contains("42")],
    axes: [axis("effort", ["low", "high"])],
    metadata: { suite: "smoke" },
    run: () => transcript("the answer is 42", { iterations: 1, usage: usage({ inputTokens: 10, outputTokens: 4 }) }),
  });
  return s;
}

const cases = [
  ["initialize", {}, "InitializeResult"],
  ["list", {}, "ListResult"],
  ["list_samples", { eval: "greet", cursor: "0" }, "ListSamplesResult"],
  ["run", { eval: "greet", sample: "hi", target: "sim" }, "RunResult"],
  ["execute", { eval: "greet", sample: "hi", target: "sim" }, "ExecuteResult"],
];

for (const [method, params, def] of cases) {
  test(`${method} result matches schema`, async () => {
    const result = await buildStudy().handle(method, params);
    check(def, result);
    // The full line must also validate against the root (anyOf envelopes).
    const root = ajv.compile(SCHEMA);
    assert.ok(root({ id: 1, result }), ajv.errorsText(root.errors));
  });
}

test("score path matches schema", async () => {
  const s = buildStudy();
  const ex = await s.handle("execute", { eval: "greet", sample: "hi", target: "sim" });
  const scored = await s.handle("score", { eval: "greet", sample: "hi", target: "sim", transcript: ex.transcript });
  check("RunResult", scored);
});

test("axis params flow through", async () => {
  const result = await buildStudy().handle("run", {
    eval: "greet",
    sample: "hi",
    target: "sim",
    params: { effort: "high" },
  });
  assert.deepEqual(result.params, { effort: "high" });
  check("RunResult", result);
});

test("capabilities advertise axes", async () => {
  const init = await buildStudy().handle("initialize", {});
  for (const cap of ["axes", "usage", "execute", "score", "paginate"]) {
    assert.ok(init.capabilities.includes(cap), `missing capability: ${cap}`);
  }
});
