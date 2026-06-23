// Serve-loop behaviour and scoring semantics (mirrors crate::runner).
import { test } from "node:test";
import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";

import {
  Study,
  sample,
  target,
  succeeded,
  contains,
  transcript,
  usage,
  makeScore,
  verdict,
  aggregate,
  PROTOCOL_VERSION,
} from "../dist/index.js";

function buildStudy() {
  const s = new Study("t");
  s.eval({
    name: "e",
    samples: [sample("s", { prompt: "p" })],
    targets: [target("sim"), target("gone", { available: false })],
    scorers: [succeeded(), contains("42")],
    run: () => transcript("answer 42", { usage: usage({ inputTokens: 1, outputTokens: 1 }) }),
  });
  return s;
}

function pagedStudy(samples, page) {
  const s = new Study("big", { pageSize: page });
  s.eval({
    name: "big",
    samples: Array.from({ length: samples }, (_, i) => sample(`s${i}`, { prompt: "go" })),
    targets: [target("sim")],
    scorers: [succeeded()],
    run: () => transcript("ok"),
  });
  return s;
}

async function drive(study, lines) {
  const input = Readable.from(lines.map((l) => l + "\n"));
  const chunks = [];
  const output = new Writable({
    write(chunk, _enc, cb) {
      chunks.push(chunk.toString());
      cb();
    },
  });
  await study.serve({ input, output });
  return chunks
    .join("")
    .split("\n")
    .filter(Boolean)
    .map((x) => JSON.parse(x));
}

test("full session over stdio", async () => {
  const msgs = await drive(buildStudy(), [
    JSON.stringify({ id: 1, method: "initialize", params: {} }),
    JSON.stringify({ id: 2, method: "list" }),
    JSON.stringify({ id: 3, method: "run", params: { eval: "e", sample: "s", target: "sim" } }),
  ]);
  assert.equal(msgs[0].result.study, "t");
  assert.equal(msgs[1].result.evals[0].name, "e");
  assert.equal(msgs[2].result.passed, true);
  assert.equal(msgs[2].result.aggregate, 1.0);
});

test("list paginates and list_samples walks pages", async () => {
  const s = pagedStudy(250, 100);
  const listing = await s.handle("list", {});
  const e = listing.evals[0];
  assert.equal(e.samples.length, 100);
  assert.equal(e.next_cursor, "100");
  assert.ok((await s.handle("initialize", {})).capabilities.includes("paginate"));

  const ids = e.samples.map((x) => x.id);
  let cursor = e.next_cursor;
  while (cursor != null) {
    const page = await s.handle("list_samples", { eval: "big", cursor });
    ids.push(...page.samples.map((x) => x.id));
    cursor = page.next_cursor ?? null;
  }
  assert.equal(ids.length, 250);
  assert.equal(ids[0], "s0");
  assert.equal(ids[249], "s249");
});

test("pagination disabled inlines all samples", async () => {
  const s = pagedStudy(250, 0);
  const e = (await s.handle("list", {})).evals[0];
  assert.equal(e.samples.length, 250);
  assert.equal(e.next_cursor, undefined);
});

test("list_samples rejects unknown eval and bad cursor", async () => {
  const s = pagedStudy(10, 5);
  for (const params of [
    { eval: "nope", cursor: "0" },
    { eval: "big", cursor: "xyz" },
  ]) {
    await assert.rejects(() => s.handle("list_samples", params));
  }
});

test("bad json logs and continues", async () => {
  const msgs = await drive(buildStudy(), ["{ not json", JSON.stringify({ id: 1, method: "initialize" })]);
  assert.equal(msgs[0].method, "log");
  assert.equal(msgs[1].result.protocol_version, PROTOCOL_VERSION);
});

test("unknown method errors without crashing", async () => {
  const msgs = await drive(buildStudy(), [
    JSON.stringify({ id: 1, method: "nope" }),
    JSON.stringify({ id: 2, method: "list" }),
  ]);
  assert.match(msgs[0].error.message, /unknown method/);
  assert.equal(msgs[0].error.code, -32601); // method not found, classifiable
  assert.ok("result" in msgs[1]); // loop kept going
});

test("unavailable target is skipped N/A", async () => {
  const result = await buildStudy().handle("run", { eval: "e", sample: "s", target: "gone" });
  assert.equal(result.skipped, true);
  // Infra error short-circuits to a single N/A — neither pass nor fail.
  assert.equal(result.passed, false);
  assert.equal(result.scores[0].na, true);
});

test("verdict and aggregate ignore N/A", () => {
  const passing = makeScore("a", 1.0, true, "");
  const failing = makeScore("b", 0.0, false, "");
  const na = makeScore("c", 0.0, false, "", true);
  assert.equal(verdict([passing, na]), true); // NA excluded
  assert.equal(verdict([passing, failing]), false);
  assert.equal(verdict([na]), false); // nothing applicable
  assert.equal(aggregate([passing, na]), 1.0); // NA not averaged in
  assert.equal(aggregate([na]), 0.0);
});

test("score path reuses a host-supplied transcript", async () => {
  const s = buildStudy();
  const ex = await s.handle("execute", { eval: "e", sample: "s", target: "sim" });
  const scored = await s.handle("score", { eval: "e", sample: "s", target: "sim", transcript: ex.transcript });
  assert.equal(scored.passed, true);
  assert.equal(scored.aggregate, 1.0);
});
