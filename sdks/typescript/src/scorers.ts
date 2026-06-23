// Built-in scorers and the `scorer(...)` escape hatch.
//
// A scorer maps `(sample, transcript) -> Score`. `value` is a continuous 0..1
// signal; `pass` the boolean verdict; `na: true` means "couldn't evaluate"
// (excluded from the cell verdict and aggregate), mirroring `mira::Score`.
import type { Score, Transcript } from "./wire.js";
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

/** Passes when the trimmed final response equals `target` exactly. */
export function equals(target: string): Scorer {
  const name = `equals("${target}")`;
  return named(name, (_s, t) => {
    const ok = (t.final_response ?? "").trim() === target;
    return makeScore(name, ok ? 1 : 0, ok, ok ? "exact match" : "mismatch");
  });
}

/** Passes when `pattern` matches anywhere in the final response. */
export function regex(pattern: string): Scorer {
  const name = `regex(${JSON.stringify(pattern)})`;
  const re = new RegExp(pattern);
  return named(name, (_s, t) => {
    const ok = re.test(t.final_response ?? "");
    return makeScore(name, ok ? 1 : 0, ok, ok ? "matched" : "no match");
  });
}

/**
 * Wrap an arbitrary predicate. The callback may return a boolean (turned into a
 * pass/fail Score) or a fully-formed Score for graded / N/A control.
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
