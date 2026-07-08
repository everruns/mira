/**
 * Mira eval SDK for TypeScript.
 *
 * Author an eval *study* in TypeScript and run it with the `mira` host CLI — no
 * Rust dependency, just the protocol (newline-delimited JSON over stdio). The
 * wire types in `./wire` are generated from the canonical JSON Schema under
 * `schema/v1/`, so they never drift from the Rust host.
 *
 * ```ts
 * import { Study, sample, target, succeeded, contains, transcript } from "@everruns/mira-eval";
 *
 * const study = new Study("my-evals", { version: "0.1.0" });
 *
 * study.eval({
 *   name: "greet",
 *   samples: [sample("hi", { prompt: "Say hi and the answer to life." })],
 *   targets: [target("sim")],
 *   scorers: [succeeded(), contains("42")],
 *   run: (s, cx) => transcript(`Hi! The answer is 42. (${s.text})`),
 * });
 *
 * study.serve();
 * ```
 */
import type {
  AxisInfo,
  ErrorKind,
  Part,
  Timing,
  Trajectory,
  Transcript,
  Usage,
} from "./wire.js";

export {
  DEFAULT_PAGE_SIZE,
  HANDLED_METHODS,
  PROTOCOL_VERSION,
  RunCx,
  Study,
  aggregate,
  log,
  sample,
  serve,
  target,
  verdict,
} from "./study.js";
export type {
  EvalOptions,
  Sample,
  SampleOptions,
  ServeOptions,
  StudyOptions,
  Subject,
  Target,
  TargetOptions,
} from "./study.js";

export {
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
  makeScore,
  matchesExpected,
  metricAtLeast,
  metricWithin,
  nonEmpty,
  not,
  notContains,
  outputTokensWithin,
  producedModality,
  regex,
  scorer,
  succeeded,
  tokensWithin,
  toolCalled,
  toolCalledBefore,
  toolCallsWithin,
  toolNotCalled,
  toolsUsedExactly,
  ttftWithin,
  turnsWithin,
} from "./scorers.js";
export type { Scorer } from "./scorers.js";

export { toWire } from "./codec.js";

export {
  ATIF_FORMAT,
  ATIF_VERSION,
  fromTrajectory,
  isSupportedSchemaVersion,
  parseTrajectory,
  projectInto,
  trajectoryUsage,
} from "./trajectory.js";

export type {
  AxisInfo,
  ContentPart,
  ErrorKind,
  EvalInfo,
  ExecuteResult,
  InitializeResult,
  ListResult,
  ListSamplesResult,
  Observation,
  ObservationResult,
  Part,
  RunResult,
  SampleInfo,
  Score,
  Step,
  StepContent,
  StepMetrics,
  TargetInfo,
  Timing,
  ToolCall,
  Trajectory,
  Transcript,
  TranscriptSummary,
  Usage,
} from "./wire.js";

/** Build a `Usage` from camelCase fields (defaults to zeros). */
export function usage(
  opts: {
    inputTokens?: number;
    outputTokens?: number;
    costUsd?: number;
    cacheReadTokens?: number;
    reasoningTokens?: number;
  } = {},
): Usage {
  return {
    input_tokens: opts.inputTokens ?? 0,
    output_tokens: opts.outputTokens ?? 0,
    cost_usd: opts.costUsd ?? 0,
    cache_read_tokens: opts.cacheReadTokens ?? 0,
    reasoning_tokens: opts.reasoningTokens ?? 0,
  };
}

/** Build a `Timing` from camelCase fields. */
export function timing(opts: { durationMs?: number; timeToFirstTokenMs?: number } = {}): Timing {
  return {
    duration_ms: opts.durationMs ?? 0,
    time_to_first_token_ms: opts.timeToFirstTokenMs,
  };
}

export interface TranscriptOptions {
  usage?: Usage;
  timing?: Timing;
  iterations?: number;
  toolCalls?: string[];
  toolCallsCount?: number;
  metrics?: Record<string, number>;
  metadata?: Record<string, unknown>;
  error?: string;
  errorKind?: ErrorKind;
  files?: Record<string, string>;
  output?: Part[];
  /** Structured ATIF trajectory — the primary structured trajectory contract.
   * Flat fields left unset here are projected from it automatically by the
   * serve loop (see `fromTrajectory` for the trajectory-only shortcut). */
  trajectory?: Trajectory;
}

/** Convenience builder for a `Transcript` (a subject's return value). */
export function transcript(finalResponse = "", opts: TranscriptOptions = {}): Transcript {
  const toolCalls = opts.toolCalls ?? [];
  return {
    final_response: finalResponse,
    iterations: opts.iterations ?? 0,
    tool_calls: toolCalls,
    tool_calls_count: opts.toolCallsCount ?? toolCalls.length,
    usage: opts.usage ?? usage(),
    timing: opts.timing,
    metrics: opts.metrics ?? {},
    metadata: opts.metadata ?? {},
    error: opts.error,
    error_kind: opts.errorKind,
    files: opts.files ?? {},
    output: opts.output ?? [],
    trajectory: opts.trajectory,
  };
}

/** Declare an extra matrix axis (crossed with the target matrix). */
export function axis(name: string, values: Iterable<string>): AxisInfo {
  return { name, values: [...values] };
}
