// Cross-language ATIF trajectory conformance. Runs the canonical vectors in
// `schema/v1/conformance/trajectory.json` (encoding the behaviour of the Rust
// types in `crates/mira-eval/src/trajectory.rs`, the source of truth) through
// this SDK's generated wire types and hand-written projection mirror
// (`src/trajectory.ts`), asserting each document parses (or is rejected),
// round-trips through the codec (extra maps included; unknown fields
// tolerated), and projects onto the pinned Transcript flat fields. The
// `scorers.json` three-runner pattern.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import {
  ATIF_VERSION,
  Study,
  fromTrajectory,
  parseTrajectory,
  projectInto,
  sample,
  target,
  toWire,
  contains,
  toolCalled,
  toolCallsWithin,
} from "../dist/index.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const VECTORS = JSON.parse(
  readFileSync(join(HERE, "../../../schema/v1/conformance/trajectory.json"), "utf8"),
);
const CASES = VECTORS.cases;
const ACCEPTED = CASES.filter((c) => !c.rejects);

for (const c of CASES) {
  test(`${c.name}: parses or rejects`, () => {
    if (c.rejects) {
      assert.throws(() => parseTrajectory(c.trajectory), /schema_version/);
    } else {
      parseTrajectory(c.trajectory);
    }
  });
}

for (const c of ACCEPTED) {
  test(`${c.name}: round-trips through the codec`, () => {
    const first = parseTrajectory(c.trajectory);
    const encoded = toWire("Trajectory", first);
    const again = parseTrajectory(JSON.parse(JSON.stringify(encoded)));
    // Lossless: re-encode of the re-parse is identical (extra maps included).
    assert.deepEqual(toWire("Trajectory", again), encoded);
    // Non-empty extra maps survive.
    if (first.extra && Object.keys(first.extra).length) {
      assert.deepEqual(encoded.extra, first.extra);
    }
  });

  test(`${c.name}: projects the pinned flat fields`, () => {
    const trajectory = parseTrajectory(c.trajectory);
    const expect = c.projection;

    // The zero-burden constructor is the pinned path.
    const t = fromTrajectory(trajectory);
    assert.equal(t.final_response, expect.final_response);
    assert.deepEqual(t.tool_calls, expect.tool_calls);
    assert.equal(t.tool_calls_count, expect.tool_calls_count);
    assert.equal(t.iterations, expect.iterations);
    assert.equal(t.usage.input_tokens, expect.usage.input_tokens);
    assert.equal(t.usage.output_tokens, expect.usage.output_tokens);
    assert.equal(t.usage.cache_read_tokens ?? 0, expect.usage.cache_read_tokens);
    assert.equal(t.usage.reasoning_tokens ?? 0, expect.usage.reasoning_tokens);
    assert.ok(Math.abs(t.usage.cost_usd - expect.usage.cost_usd) < 1e-9);
  });
}

test("trajectory-only transcript scores with name-based scorers", async () => {
  // Zero client burden, end-to-end: a subject returns a transcript that sets
  // ONLY `trajectory` — no flat fields, no events — and the built-in
  // name-based scorers see the projected names via the serve loop's
  // normalization.
  const doc = {
    schema_version: ATIF_VERSION,
    agent: { name: "external-agent", version: "1.0" },
    steps: [
      { step_id: 1, source: "user", message: "hi" },
      {
        step_id: 2,
        source: "agent",
        message: "hi there",
        tool_calls: [{ tool_call_id: "c1", function_name: "search", arguments: { q: "hi" } }],
        metrics: { prompt_tokens: 10, completion_tokens: 4 },
      },
    ],
  };

  const s = new Study("traj", { version: "0.0.1" });
  s.eval({
    name: "greet",
    samples: [sample("hi", { prompt: "say hi" })],
    targets: [target("sim")],
    scorers: [contains("hi there"), toolCalled("search"), toolCallsWithin(1)],
    // The transcript carries ONLY the trajectory — nothing else to call.
    run: () => ({ trajectory: parseTrajectory(doc) }),
  });

  const base = { eval: "greet", sample: "hi", target: "sim" };
  const run = await s.handle("run", base);
  assert.ok(run.passed, JSON.stringify(run.scores));
  assert.equal(run.transcript.final_response, "hi there");
  assert.deepEqual(run.transcript.tool_calls, ["search"]);
  assert.equal(run.transcript.usage.input_tokens, 10);

  // The score path (a replayed trajectory-only transcript) normalizes too.
  const scored = await s.handle("score", { ...base, transcript: { trajectory: doc } });
  assert.ok(scored.passed, JSON.stringify(scored.scores));
  assert.deepEqual(scored.transcript.tool_calls, ["search"]);

  // execute returns the full transcript with the trajectory + projections.
  const ex = await s.handle("execute", base);
  assert.equal(ex.transcript.trajectory.schema_version, ATIF_VERSION);
  assert.deepEqual(ex.transcript.tool_calls, ["search"]);

  // The capability + params are advertised.
  const init = await s.handle("initialize", {});
  assert.ok(init.capabilities.includes("trajectory"));
  assert.deepEqual(init.capability_params.trajectory, { format: "ATIF", version: "1.7" });
});

test("explicit flat fields are never overwritten", () => {
  const trajectory = parseTrajectory({
    schema_version: "ATIF-v1.7",
    agent: { name: "a", version: "1" },
    steps: [{ step_id: 1, source: "agent", message: "derived" }],
  });
  const t = { final_response: "explicit", iterations: 7 };
  projectInto(trajectory, t);
  assert.equal(t.final_response, "explicit");
  assert.equal(t.iterations, 7);
});
