// ATIF trajectory helpers: parse, project, and build trajectory-only transcripts.
//
// `Transcript.trajectory` (generated from schema/v1/) is the protocol's
// **primary structured trajectory contract** — an ATIF document (harbor RFC
// 0001). A study may return a transcript that sets *only* `trajectory`: the
// serve loop projects the flat fields (`final_response`, `tool_calls`,
// `tool_calls_count`, `iterations`, `usage`) from it automatically, so the
// built-in name-based scorers keep working with zero calls on the study
// author's part. `events` is optional and fully independent of the trajectory.
//
// PARITY — source of truth: `crates/mira-eval/src/trajectory.rs`.
// `projectInto` hand-mirrors `Trajectory::project_into` (fill-if-default,
// never overwrite an explicitly set flat field) and `parseTrajectory` mirrors
// `Trajectory::from_value` (accept any ATIF-v1.x, reject other prefixes with
// an error). Behaviour is pinned by the shared vectors in
// `schema/v1/conformance/trajectory.json`, run by
// `tests/trajectory-conformance.test.mjs` (and the Rust + Python twins).
import type { StepContent, Trajectory, Transcript, Usage } from "./wire.js";

/** The ATIF schema version this SDK emits. Parsing is more lenient: any
 * ATIF-v1.x document is accepted — this is not a ceiling on what is read. */
export const ATIF_VERSION = "ATIF-v1.7";
export const ATIF_FORMAT = "ATIF";

/** True when `version` names an ATIF v1 document this SDK can read. */
export function isSupportedSchemaVersion(version: string): boolean {
  return version === "ATIF-v1" || version.startsWith("ATIF-v1.");
}

/**
 * Parse one ATIF document (a JSON-decoded value) into a `Trajectory`.
 *
 * Lenient within v1 (unknown fields ride along untouched); a non-v1
 * `schema_version` — or a value that isn't a trajectory-shaped object —
 * throws an `Error` the serve loop can surface, never a crash on untrusted
 * agent output.
 */
export function parseTrajectory(data: unknown): Trajectory {
  if (typeof data !== "object" || data === null || Array.isArray(data)) {
    throw new Error("invalid ATIF trajectory: not a JSON object");
  }
  const t = data as Trajectory;
  if (typeof t.schema_version !== "string" || !isSupportedSchemaVersion(t.schema_version)) {
    throw new Error(
      `unsupported ATIF schema_version ${JSON.stringify(t.schema_version)}: ` +
        `this SDK reads ATIF-v1.x (emits ${ATIF_VERSION})`,
    );
  }
  if (!Array.isArray(t.steps) || typeof t.agent !== "object" || t.agent === null) {
    throw new Error("invalid ATIF trajectory: missing agent/steps");
  }
  return t;
}

/** The text projection of a StepContent value: the string itself, or the
 * `text` parts joined by newlines (image parts skipped) — mirrors
 * `StepContent::text`. Exported for the trajectory scorers. */
export function contentText(content: StepContent | null | undefined): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((p) => (p != null && p.type === "text" && typeof p.text === "string" ? p.text : null))
      .filter((x): x is string => x !== null)
      .join("\n");
  }
  return "";
}

function finalAgentText(trajectory: Trajectory): string | undefined {
  for (const step of [...trajectory.steps].reverse()) {
    if (step.source === "agent") return contentText(step.message);
  }
  return undefined;
}

function toolCallNames(trajectory: Trajectory): string[] {
  const names: string[] = [];
  for (const step of trajectory.steps) {
    for (const call of step.tool_calls ?? []) names.push(call.function_name);
  }
  return names;
}

function agentIterations(trajectory: Trajectory): number {
  return trajectory.steps.filter((s) => s.source === "agent" && s.llm_call_count !== 0).length;
}

function extraU64(extra: Record<string, unknown> | undefined, key: string): number {
  const v = extra?.[key];
  return typeof v === "number" && Number.isInteger(v) && v >= 0 ? v : 0;
}

/** The `Usage` projection: `final_metrics` when present, else the per-step
 * sum. `reasoning_tokens` is read from the corresponding `extra` map (ATIF
 * has no first-class slot for it). */
export function trajectoryUsage(trajectory: Trajectory): Usage {
  const fm = trajectory.final_metrics;
  if (fm != null) {
    return {
      input_tokens: fm.total_prompt_tokens ?? 0,
      output_tokens: fm.total_completion_tokens ?? 0,
      cache_read_tokens: fm.total_cached_tokens ?? 0,
      reasoning_tokens: extraU64(fm.extra, "reasoning_tokens"),
      cost_usd: fm.total_cost_usd ?? 0,
    };
  }
  const usage: Usage = {
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    reasoning_tokens: 0,
    cost_usd: 0,
  };
  for (const step of trajectory.steps) {
    const m = step.metrics;
    if (m == null) continue;
    usage.input_tokens += m.prompt_tokens ?? 0;
    usage.output_tokens += m.completion_tokens ?? 0;
    usage.cache_read_tokens = (usage.cache_read_tokens ?? 0) + (m.cached_tokens ?? 0);
    usage.reasoning_tokens = (usage.reasoning_tokens ?? 0) + extraU64(m.extra, "reasoning_tokens");
    usage.cost_usd += m.cost_usd ?? 0;
  }
  return usage;
}

function usageIsDefault(u: Usage | undefined): boolean {
  return (
    u == null ||
    (u.input_tokens === 0 &&
      u.output_tokens === 0 &&
      (u.cache_read_tokens ?? 0) === 0 &&
      (u.reasoning_tokens ?? 0) === 0 &&
      u.cost_usd === 0)
  );
}

/** Fill the transcript's flat fields from `trajectory` — fill-if-default, so a
 * flat field the study set explicitly is never overwritten. Mirrors
 * `Trajectory::project_into` (see the module header for the rules). */
export function projectInto(trajectory: Trajectory, transcript: Transcript): void {
  if (!transcript.final_response) {
    const text = finalAgentText(trajectory);
    if (text !== undefined) transcript.final_response = text;
  }
  if (!transcript.tool_calls?.length) {
    transcript.tool_calls = toolCallNames(trajectory);
  }
  if (!transcript.tool_calls_count) {
    transcript.tool_calls_count = transcript.tool_calls.length;
  }
  if (!transcript.iterations) {
    transcript.iterations = agentIterations(trajectory);
  }
  if (usageIsDefault(transcript.usage)) {
    transcript.usage = trajectoryUsage(trajectory);
  }
}

/** A transcript built from a trajectory alone — the zero-burden path: the flat
 * fields are projected automatically, and nothing else (not `events`) is
 * required of the study. */
export function fromTrajectory(trajectory: Trajectory): Transcript {
  const transcript: Transcript = {};
  projectInto(trajectory, transcript);
  transcript.trajectory = trajectory;
  return transcript;
}

/** Project `transcript.trajectory` (if any) into flat fields still at their
 * defaults. The serve loop applies this to every produced or received
 * transcript, so studies never call it themselves. */
export function normalizeTrajectory(transcript: Transcript): Transcript {
  if (transcript.trajectory != null) projectInto(transcript.trajectory, transcript);
  return transcript;
}
