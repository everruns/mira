// Built-in scorers and the `scorer(...)` escape hatch.
//
// A scorer maps `(sample, transcript) -> Score`. `value` is a continuous 0..1
// signal; `pass` the boolean verdict; `na: true` means "couldn't evaluate"
// (excluded from the case verdict and aggregate), mirroring `mira::Score`.
//
// PARITY — source of truth: `crates/mira-eval/src/scorer.rs`.
// The Rust scorers are canonical; every function here is a hand-written mirror
// of its Rust twin (same name, same verdict). Behaviour is pinned by the shared
// vectors in `schema/v1/conformance/scorers.json` and verified by
// `tests/scorer-parity.test.mjs`. Change a scorer in Rust → update the vectors →
// mirror the change here and in the Python SDK. `reason` strings are
// human-facing and may differ across languages; the verdict (pass/value/na)
// must not. The LLM-judge (`model_graded`) is deliberately not mirrored — it is
// not a deterministic, portable scorer.
import type { Score, Transcript, Part } from "./wire.js";
import type { Sample } from "./study.js";

export interface Scorer {
  name: string;
  score(sample: Sample, transcript: Transcript): Score;
}

export function makeScore(
  name: string,
  value: number,
  passed: boolean,
  reason: string,
  na = false,
): Score {
  return { scorer: name, value, pass: passed, reason, na };
}

/** Build a named scorer from a `(sample, transcript) -> Score` function. */
function named(name: string, fn: (s: Sample, t: Transcript) => Score): Scorer {
  return { name, score: fn };
}

function passfail(name: string, ok: boolean, yes: string, no: string): Score {
  return makeScore(name, ok ? 1 : 0, ok, ok ? yes : no);
}

/** Distinct tool names in first-seen order — mirrors `Transcript::tools_used`. */
function toolsUsed(t: Transcript): string[] {
  const seen: string[] = [];
  for (const n of t.tool_calls ?? []) if (!seen.includes(n)) seen.push(n);
  return seen;
}

// ----- text scorers ---------------------------------------------------------

/** Passes when the subject produced no error. */
export function succeeded(): Scorer {
  return named("succeeded", (_s, t) => {
    const ok = t.error === null || t.error === undefined;
    return makeScore("succeeded", ok ? 1 : 0, ok, ok ? "no error" : t.error || "errored");
  });
}

/** Passes when `text` appears in the final response. */
export function contains(text: string): Scorer {
  const name = `contains("${text}")`;
  return named(name, (_s, t) => {
    const found = (t.final_response ?? "").includes(text);
    return makeScore(name, found ? 1 : 0, found, found ? `found "${text}"` : `missing "${text}"`);
  });
}

/** Passes when `text` does NOT appear in the final response. */
export function notContains(text: string): Scorer {
  const name = `not_contains("${text}")`;
  return named(name, (_s, t) => {
    const present = (t.final_response ?? "").includes(text);
    return passfail(name, !present, `absent "${text}"`, `unexpectedly found "${text}"`);
  });
}

/** Trimmed, ASCII-case-insensitive match — mirrors Rust `equals`. */
export function equals(target: string): Scorer {
  const name = `equals("${target}")`;
  return named(name, (_s, t) => {
    const ok = (t.final_response ?? "").trim().toLowerCase() === target.trim().toLowerCase();
    return passfail(name, ok, "exact match", "mismatch");
  });
}

/** Passes when `pattern` matches anywhere in the final response. */
export function regex(pattern: string): Scorer {
  const name = `regex(${JSON.stringify(pattern)})`;
  const re = new RegExp(pattern);
  return named(name, (_s, t) => {
    const ok = re.test(t.final_response ?? "");
    return passfail(name, ok, "matched", "no match");
  });
}

/** Trimmed, case-sensitive match against the sample's expected answer. */
export function matchesExpected(): Scorer {
  const name = "matches_expected";
  return named(name, (s, t) => {
    const expected = s.expected;
    if (expected === undefined || expected === null) {
      return makeScore(name, 0, false, "sample has no string expected answer");
    }
    const ok = (t.final_response ?? "").trim() === expected.trim();
    return passfail(name, ok, "matched expected", `expected ${JSON.stringify(expected)}`);
  });
}

/** Passes when the final response is non-empty after trimming. */
export function nonEmpty(): Scorer {
  const name = "non_empty";
  return named(name, (_s, t) => {
    const ok = (t.final_response ?? "").trim().length > 0;
    return passfail(name, ok, "non-empty response", "empty response");
  });
}

// ----- file / workspace scorers ---------------------------------------------

/** Passes when a file at `path` exists in the captured workspace. */
export function fileExists(path: string): Scorer {
  const name = `file_exists(${path})`;
  return named(name, (_s, t) => {
    const ok = Object.prototype.hasOwnProperty.call(t.files ?? {}, path);
    return passfail(name, ok, `${path} exists`, `no such file: ${path}`);
  });
}

/** Passes when the file at `path` exists and contains `needle`. */
export function fileContains(path: string, needle: string): Scorer {
  const name = `file_contains(${path}, ${JSON.stringify(needle)})`;
  return named(name, (_s, t) => {
    const files = t.files ?? {};
    const contents = files[path];
    if (contents === undefined) {
      return makeScore(name, 0, false, `no such file: ${path}`);
    }
    const ok = contents.includes(needle);
    return passfail(name, ok, `${path} contains ${JSON.stringify(needle)}`, `${path} missing ${JSON.stringify(needle)}`);
  });
}

// ----- tool-call scorers ----------------------------------------------------

/** Passes when a tool named `tool` was invoked at least once. */
export function toolCalled(tool: string): Scorer {
  const name = `tool_called(${tool})`;
  return named(name, (_s, t) => {
    const ok = (t.tool_calls ?? []).includes(tool);
    return passfail(name, ok, `${tool} was called`, `${tool} never called`);
  });
}

/** Passes when a tool named `tool` was never invoked. */
export function toolNotCalled(tool: string): Scorer {
  const name = `tool_not_called(${tool})`;
  return named(name, (_s, t) => {
    const called = (t.tool_calls ?? []).includes(tool);
    return passfail(name, !called, `${tool} never called`, `${tool} was called`);
  });
}

/** Passes when the run used no more than `max` tool calls. */
export function toolCallsWithin(max: number): Scorer {
  const name = `tool_calls_within(${max})`;
  return named(name, (_s, t) => {
    const n = t.tool_calls_count ?? 0;
    return passfail(name, n <= max, `${n} <= ${max}`, `${n} > ${max}`);
  });
}

/** Passes when the run took no more than `max` reasoning iterations. */
export function turnsWithin(max: number): Scorer {
  const name = `turns_within(${max})`;
  return named(name, (_s, t) => {
    const n = t.iterations ?? 0;
    return passfail(name, n <= max, `${n} <= ${max}`, `${n} > ${max}`);
  });
}

/** Passes when exactly the given set of tools was used (order-independent). */
export function toolsUsedExactly(tools: string[]): Scorer {
  const expected = [...new Set(tools)].sort();
  const label = expected.join(",");
  const name = `tools_used_exactly([${label}])`;
  return named(name, (_s, t) => {
    const used = toolsUsed(t).sort();
    const ok = used.length === expected.length && used.every((v, i) => v === expected[i]);
    return passfail(name, ok, `used exactly [${label}]`, `expected [${label}], used [${used.join(",")}]`);
  });
}

/** Passes when tool `first` was invoked before tool `second` (both must appear). */
export function toolCalledBefore(first: string, second: string): Scorer {
  const name = `tool_called_before(${first}, ${second})`;
  return named(name, (_s, t) => {
    const calls = t.tool_calls ?? [];
    const fi = calls.indexOf(first);
    const si = calls.indexOf(second);
    if (fi >= 0 && si >= 0) {
      return passfail(name, fi < si, `${first} before ${second}`, `${first} not before ${second}`);
    }
    return makeScore(name, 0, false, `both ${first} and ${second} must be called`);
  });
}

// ----- budget scorers -------------------------------------------------------

/** Passes when total cost stayed at or under `maxUsd`. */
export function costWithin(maxUsd: number): Scorer {
  const name = `cost_within($${maxUsd})`;
  return named(name, (_s, t) => {
    const c = t.usage?.cost_usd ?? 0;
    return passfail(name, c <= maxUsd, `$${c.toFixed(4)} <= $${maxUsd}`, `$${c.toFixed(4)} > $${maxUsd}`);
  });
}

/** Passes when total tokens (input + output) stayed at or under `max`. */
export function tokensWithin(max: number): Scorer {
  const name = `tokens_within(${max})`;
  return named(name, (_s, t) => {
    const total = (t.usage?.input_tokens ?? 0) + (t.usage?.output_tokens ?? 0);
    return passfail(name, total <= max, `${total} <= ${max} tokens`, `${total} > ${max} tokens`);
  });
}

/** Passes when output (completion) tokens stayed at or under `max`. */
export function outputTokensWithin(max: number): Scorer {
  const name = `output_tokens_within(${max})`;
  return named(name, (_s, t) => {
    const out = t.usage?.output_tokens ?? 0;
    return passfail(name, out <= max, `${out} <= ${max}`, `${out} > ${max}`);
  });
}

/** Passes when wall-clock duration stayed at or under `maxMs`. */
export function latencyWithin(maxMs: number): Scorer {
  const name = `latency_within(${maxMs}ms)`;
  return named(name, (_s, t) => {
    const ms = t.timing?.duration_ms ?? 0;
    return passfail(name, ms <= maxMs, `${ms}ms <= ${maxMs}ms`, `${ms}ms > ${maxMs}ms`);
  });
}

/** Passes when time-to-first-token stayed at or under `maxMs`. Unmeasured TTFT fails. */
export function ttftWithin(maxMs: number): Scorer {
  const name = `ttft_within(${maxMs}ms)`;
  return named(name, (_s, t) => {
    const ms = t.timing?.time_to_first_token_ms;
    if (ms === undefined || ms === null) {
      return makeScore(name, 0, false, "subject did not report TTFT");
    }
    return passfail(name, ms <= maxMs, `ttft ${ms}ms <= ${maxMs}ms`, `ttft ${ms}ms > ${maxMs}ms`);
  });
}

// ----- custom (open-vocabulary) metric scorers ------------------------------

/** Passes when the custom metric `metric` is at or below `max`. Unreported fails. */
export function metricWithin(metric: string, max: number): Scorer {
  const name = `metric_within(${metric} <= ${max})`;
  return named(name, (_s, t) => {
    const v = (t.metrics ?? {})[metric];
    if (v === undefined) return makeScore(name, 0, false, `subject did not report ${metric}`);
    return passfail(name, v <= max, `${metric}=${v} <= ${max}`, `${metric}=${v} > ${max}`);
  });
}

/** Passes when the custom metric `metric` is at or above `min`. Unreported fails. */
export function metricAtLeast(metric: string, min: number): Scorer {
  const name = `metric_at_least(${metric} >= ${min})`;
  return named(name, (_s, t) => {
    const v = (t.metrics ?? {})[metric];
    if (v === undefined) return makeScore(name, 0, false, `subject did not report ${metric}`);
    return passfail(name, v >= min, `${metric}=${v} >= ${min}`, `${metric}=${v} < ${min}`);
  });
}

// ----- JSON output scorers --------------------------------------------------

/** Passes when the final response parses as JSON. */
export function jsonValid(): Scorer {
  const name = "json_valid";
  return named(name, (_s, t) => {
    try {
      JSON.parse((t.final_response ?? "").trim());
      return makeScore(name, 1, true, "valid JSON");
    } catch (e) {
      return makeScore(name, 0, false, `invalid JSON: ${(e as Error).message}`);
    }
  });
}

/** Passes when the response is a JSON object whose top-level `key` equals `value`. */
export function jsonFieldEquals(key: string, value: string): Scorer {
  const name = `json_field_equals(${key}=${JSON.stringify(value)})`;
  return named(name, (_s, t) => {
    let parsed: unknown;
    try {
      parsed = JSON.parse((t.final_response ?? "").trim());
    } catch {
      return makeScore(name, 0, false, `no JSON field ${key}`);
    }
    if (typeof parsed !== "object" || parsed === null || !(key in parsed)) {
      return makeScore(name, 0, false, `no JSON field ${key}`);
    }
    const got = (parsed as Record<string, unknown>)[key];
    if (typeof got === "string" && got === value) {
      return makeScore(name, 1, true, `${key} == ${JSON.stringify(value)}`);
    }
    return makeScore(name, 0, false, `${key} is ${String(got)}, expected ${JSON.stringify(value)}`);
  });
}

// ----- multimodal output scorer ---------------------------------------------

/** Passes when the subject produced an output Part of the given modality. */
export function producedModality(modality: string): Scorer {
  const name = `produced_modality(${modality})`;
  return named(name, (_s, t) => {
    const ok = (t.output ?? []).some((p: Part) => (p as Record<string, unknown>).kind === modality);
    return passfail(name, ok, `produced a ${modality} part`, `no ${modality} part in output`);
  });
}

// ----- combinators ----------------------------------------------------------

function glyph(s: Score): string {
  return s.na ? "–" : s.pass ? "✓" : "✗";
}

function combine(name: string, scorers: Scorer[], requireAll: boolean): Scorer {
  return named(name, (sample, t) => {
    const values: Array<[number, boolean]> = [];
    const reasons: string[] = [];
    for (const sc of scorers) {
      const s = sc.score(sample, t);
      reasons.push(`${glyph(s)}${s.scorer}`);
      if (!s.na) values.push([s.value, s.pass]);
    }
    const reason = reasons.join(", ");
    if (values.length === 0) return makeScore(name, 0, false, reason, true);
    let passed: boolean;
    let value: number;
    if (requireAll) {
      passed = values.every(([, p]) => p);
      value = values.reduce((a, [v]) => a + v, 0) / values.length;
    } else {
      passed = values.some(([, p]) => p);
      value = values.reduce((a, [v]) => Math.max(a, v), 0);
    }
    return makeScore(name, value, passed, reason);
  });
}

/** Passes only if every inner scorer passes; value is their mean. */
export function allOf(name: string, scorers: Scorer[]): Scorer {
  return combine(name, scorers, true);
}

/** Passes if any inner scorer passes; value is the max. */
export function anyOf(name: string, scorers: Scorer[]): Scorer {
  return combine(name, scorers, false);
}

/** Inverts a scorer; an N/A inner stays N/A (you can't invert "unknown"). */
export function not(inner: Scorer): Scorer {
  const name = `not(${inner.name})`;
  return named(name, (sample, t) => {
    const s = inner.score(sample, t);
    if (s.na) return makeScore(`not(${s.scorer})`, 0, false, `inner N/A: ${s.reason}`, true);
    return makeScore(`not(${s.scorer})`, 1 - s.value, !s.pass, `inverted: ${s.reason}`);
  });
}

// ----- escape hatch ---------------------------------------------------------

/**
 * Wrap an arbitrary predicate. The callback may return a boolean (turned into a
 * pass/fail Score) or a fully-formed Score for graded / N/A control. This is a
 * language-local escape hatch and intentionally has no cross-SDK parity.
 */
export function scorer(
  name: string,
  fn: (sample: Sample, transcript: Transcript) => boolean | Score,
): Scorer {
  return named(name, (sample, t) => {
    const out = fn(sample, t);
    if (typeof out === "boolean") {
      return makeScore(name, out ? 1 : 0, out, out ? "ok" : "failed");
    }
    return out;
  });
}
