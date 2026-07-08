// Cross-language scorer parity. Runs the canonical vectors in
// schema/v1/conformance/scorers.json (encoding the behaviour of the Rust
// scorers in crates/mira-eval/src/scorer.rs, the source of truth) through this
// SDK's hand-written scorers and asserts the verdict matches.
//
// Only the verdict-affecting fields (pass/value/na) are checked — `reason` text
// is human-facing and allowed to differ. A coverage check ensures every scorer
// kind in the vectors is implemented here (or explicitly declared unsupported),
// so a scorer added to Rust can't silently go missing in TypeScript.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import {
  allOf,
  anyOf,
  contains,
  costWithin,
  equals,
  fileContains,
  fileExists,
  jsonFieldEquals,
  jsonValid,
  latencyWithin,
  matchesExpected,
  metricAtLeast,
  metricWithin,
  nonEmpty,
  not,
  notContains,
  observationContains,
  outputTokensWithin,
  producedModality,
  regex,
  stepsWithin,
  succeeded,
  tokensWithin,
  toolArgMatches,
  toolCalled,
  toolCalledBefore,
  toolCalledWith,
  toolCallsWithin,
  toolNotCalled,
  toolsUsedExactly,
  ttftWithin,
  turnsWithin,
} from "../dist/index.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const VECTORS = JSON.parse(
  readFileSync(join(HERE, "../../../schema/v1/conformance/scorers.json"), "utf8"),
);

// Scorers intentionally not portable to this SDK (no deterministic,
// language-neutral spec). They never appear in the vectors; listed for clarity.
const UNSUPPORTED = new Set(["model_graded", "scorer"]);

function build(spec) {
  switch (spec.kind) {
    case "contains": return contains(spec.needle);
    case "not_contains": return notContains(spec.needle);
    case "equals": return equals(spec.expected);
    case "regex": return regex(spec.pattern);
    case "matches_expected": return matchesExpected();
    case "non_empty": return nonEmpty();
    case "succeeded": return succeeded();
    case "file_exists": return fileExists(spec.path);
    case "file_contains": return fileContains(spec.path, spec.needle);
    case "tool_called": return toolCalled(spec.tool);
    case "tool_not_called": return toolNotCalled(spec.tool);
    case "tool_calls_within": return toolCallsWithin(spec.max);
    case "turns_within": return turnsWithin(spec.max);
    case "tools_used_exactly": return toolsUsedExactly(spec.tools);
    case "tool_called_before": return toolCalledBefore(spec.first, spec.second);
    case "tool_called_with": return toolCalledWith(spec.tool, spec.pointer, spec.expected);
    case "tool_arg_matches": return toolArgMatches(spec.tool, spec.pointer, spec.pattern);
    case "observation_contains": return observationContains(spec.tool, spec.needle);
    case "steps_within": return stepsWithin(spec.max);
    case "cost_within": return costWithin(spec.max_usd);
    case "tokens_within": return tokensWithin(spec.max);
    case "output_tokens_within": return outputTokensWithin(spec.max);
    case "latency_within": return latencyWithin(spec.max_ms);
    case "ttft_within": return ttftWithin(spec.max_ms);
    case "metric_within": return metricWithin(spec.name, spec.max);
    case "metric_at_least": return metricAtLeast(spec.name, spec.min);
    case "json_valid": return jsonValid();
    case "json_field_equals": return jsonFieldEquals(spec.key, spec.value);
    case "produced_modality": return producedModality(spec.modality);
    case "all_of": return allOf(spec.name, spec.of.map(build));
    case "any_of": return anyOf(spec.name, spec.of.map(build));
    case "not": return not(build(spec.of));
    default: throw new RangeError(`unhandled scorer kind: ${spec.kind}`);
  }
}

for (const c of VECTORS.cases) {
  test(`parity: ${c.name}`, () => {
    const scorer = build(c.scorer);
    const sample = { expected: c.sample?.expected };
    const transcript = VECTORS.transcripts[c.transcript];
    const score = scorer.score(sample, transcript);
    assert.equal(score.pass, c.expect.pass, `pass mismatch (${score.reason})`);
    assert.equal(score.na, c.expect.na, `na mismatch (${score.reason})`);
    assert.ok(
      Math.abs(score.value - c.expect.value) < 1e-9,
      `value ${score.value} != ${c.expect.value}`,
    );
  });
}

test("coverage: every vector kind is implemented", () => {
  const specs = new Map();
  const collect = (spec) => {
    if (!specs.has(spec.kind)) specs.set(spec.kind, spec);
    if (Array.isArray(spec.of)) spec.of.forEach(collect);
    else if (spec.of && typeof spec.of === "object") collect(spec.of);
  };
  for (const c of VECTORS.cases) collect(c.scorer);
  const missing = [];
  for (const [kind, spec] of specs) {
    try {
      build(spec);
    } catch (e) {
      if (!UNSUPPORTED.has(kind)) missing.push(kind);
    }
  }
  assert.deepEqual(missing, [], `scorer kinds in vectors not implemented in TS: ${missing}`);
});
