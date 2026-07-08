// Wire types for the Mira eval protocol — GENERATED, do not edit.
//
// Regenerate with `node codegen.mjs` from schema/v1/schema.json (the same
// language-neutral contract the Rust host is generated from). CI runs
// `node codegen.mjs --check` to fail on drift.

export interface Agent {
  extra?: Record<string, unknown>;
  model_name?: string | null;
  name: string;
  tool_definitions?: unknown[];
  version: string;
}

export interface AxisInfo {
  name: string;
  values: string[];
}

export interface CancelParams {
  id: number;
}

export interface CancelResult {
  cancelled: boolean;
}

export interface ContentPart {
  extra?: Record<string, unknown>;
  source?: ImageSource | null;
  text?: string | null;
  type: string;
}

export type ErrorKind = "subject" | "infra";

export interface EvalInfo {
  axes?: AxisInfo[];
  description?: string;
  max_turns?: number;
  metadata?: Record<string, unknown>;
  name: string;
  next_cursor?: string | null;
  samples: SampleInfo[];
  scorers: string[];
  seed?: number | null;
  targets: TargetInfo[];
  trials?: number;
}

export interface EventParams {
  eval: string;
  kind: string;
  params?: Record<string, string>;
  request_id?: number;
  sample: string;
  target: string;
  text?: string | null;
  tool?: string | null;
  turn?: number | null;
}

export interface ExecuteResult {
  eval: string;
  params?: Record<string, string>;
  sample: string;
  seed?: number | null;
  skipped?: boolean;
  target: string;
  transcript: Transcript;
  trial?: number;
  trials?: number;
}

export interface FinalMetrics {
  extra?: Record<string, unknown>;
  total_cached_tokens?: number | null;
  total_completion_tokens?: number | null;
  total_cost_usd?: number | null;
  total_prompt_tokens?: number | null;
  total_steps?: number | null;
}

export interface ImageSource {
  media_type: string;
  path: string;
}

export interface InitializeResult {
  capabilities?: string[];
  capability_params?: Record<string, unknown>;
  evals: number;
  protocol_version: string;
  study: string;
  study_version?: string | null;
}

export interface ListResult {
  evals: EvalInfo[];
}

export interface ListSamplesParams {
  cursor: string;
  eval: string;
}

export interface ListSamplesResult {
  next_cursor?: string | null;
  samples: SampleInfo[];
}

export interface LogParams {
  message: string;
  request_id?: number;
}

export interface Notification {
  method: string;
  params?: unknown;
}

export interface Observation {
  results: ObservationResult[];
}

export interface ObservationResult {
  content?: StepContent | null;
  extra?: Record<string, unknown>;
  source_call_id?: string | null;
  subagent_trajectory_ref?: SubagentTrajectoryRef[];
}

export type Part = Record<string, unknown>; // tagged union: kind in (text, image, audio, file, json)

export interface Request {
  id: number;
  method: string;
  params?: unknown;
}

export interface Response {
  error?: RpcError | null;
  id: number;
  result?: unknown;
}

export interface RpcError {
  code?: number;
  data?: unknown;
  message: string;
  retryable?: boolean;
}

export interface RunParams {
  eval: string;
  params?: Record<string, string>;
  sample: string;
  seed?: number | null;
  target: string;
  trial?: number;
  trials?: number;
}

export interface RunResult {
  aggregate: number;
  eval: string;
  expected?: unknown;
  input?: string[];
  params?: Record<string, string>;
  passed: boolean;
  sample: string;
  scores: Score[];
  seed?: number | null;
  skipped?: boolean;
  target: string;
  transcript: TranscriptSummary;
  trial?: number;
  trials?: number;
}

export interface SampleInfo {
  id: string;
  metadata?: Record<string, unknown>;
  tags?: string[];
}

export interface Score {
  na?: boolean;
  pass: boolean;
  reason: string;
  scorer: string;
  value: number;
}

export interface ScoreParams {
  eval: string;
  params?: Record<string, string>;
  sample: string;
  seed?: number | null;
  target: string;
  transcript: Transcript;
  trial?: number;
  trials?: number;
}

export type Source = Record<string, unknown>; // union of objects

export interface Step {
  extra?: Record<string, unknown>;
  is_copied_context?: boolean | null;
  llm_call_count?: number | null;
  message?: StepContent;
  metrics?: StepMetrics | null;
  model_name?: string | null;
  observation?: Observation | null;
  reasoning_content?: string | null;
  reasoning_effort?: unknown;
  source: string;
  step_id: number;
  timestamp?: string | null;
  tool_calls?: ToolCall[];
}

export type StepContent = string | ContentPart[];

export interface StepMetrics {
  cached_tokens?: number | null;
  completion_token_ids?: number[] | null;
  completion_tokens?: number | null;
  cost_usd?: number | null;
  extra?: Record<string, unknown>;
  logprobs?: number[] | null;
  prompt_token_ids?: number[] | null;
  prompt_tokens?: number | null;
}

export interface SubagentTrajectoryRef {
  extra?: Record<string, unknown>;
  session_id?: string | null;
  trajectory_id?: string | null;
  trajectory_path?: string | null;
}

export interface TargetInfo {
  available: boolean;
  label: string;
  metadata?: Record<string, unknown>;
  provider?: string;
}

export interface Timing {
  duration_ms?: number;
  time_to_first_token_ms?: number | null;
}

export interface ToolCall {
  arguments: unknown;
  extra?: Record<string, unknown>;
  function_name: string;
  tool_call_id: string;
}

export interface Trajectory {
  agent: Agent;
  continued_trajectory_ref?: string | null;
  extra?: Record<string, unknown>;
  final_metrics?: FinalMetrics | null;
  notes?: string | null;
  schema_version: string;
  session_id?: string | null;
  steps: Step[];
  subagent_trajectories?: Trajectory[];
  trajectory_id?: string | null;
}

export interface Transcript {
  error?: string | null;
  error_kind?: ErrorKind;
  events?: unknown[];
  files?: Record<string, string>;
  final_response?: string;
  iterations?: number;
  metadata?: Record<string, unknown>;
  metrics?: Record<string, number>;
  output?: Part[];
  timing?: Timing;
  tool_calls?: string[];
  tool_calls_count?: number;
  trajectory?: Trajectory | null;
  usage?: Usage;
}

export interface TranscriptSummary {
  error?: string | null;
  error_kind?: ErrorKind;
  final_response: string;
  iterations: number;
  metadata?: Record<string, unknown>;
  metrics?: Record<string, number>;
  output?: Part[];
  timing?: Timing;
  tool_calls?: string[];
  tool_calls_count: number;
  usage: Usage;
}

export interface Usage {
  cache_read_tokens?: number;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens?: number;
}

/** Per-type field metadata the codec uses to clean wire objects on encode:
 * required fields survive even when empty; nested object defs are recursed.
 * GENERATED alongside the interfaces above. */
export interface FieldMeta {
  required: boolean;
  ref?: string;
  arrayRef?: string;
}

export const WIRE_FIELDS: Record<string, Record<string, FieldMeta>> = {
  Agent: {
    "extra": { required: false },
    "model_name": { required: false },
    "name": { required: true },
    "tool_definitions": { required: false },
    "version": { required: true },
  },
  AxisInfo: {
    "name": { required: true },
    "values": { required: true },
  },
  CancelParams: {
    "id": { required: true },
  },
  CancelResult: {
    "cancelled": { required: true },
  },
  ContentPart: {
    "extra": { required: false },
    "source": { required: false },
    "text": { required: false },
    "type": { required: true },
  },
  EvalInfo: {
    "axes": { required: false, arrayRef: "AxisInfo" },
    "description": { required: false },
    "max_turns": { required: false },
    "metadata": { required: false },
    "name": { required: true },
    "next_cursor": { required: false },
    "samples": { required: true, arrayRef: "SampleInfo" },
    "scorers": { required: true },
    "seed": { required: false },
    "targets": { required: true, arrayRef: "TargetInfo" },
    "trials": { required: false },
  },
  EventParams: {
    "eval": { required: true },
    "kind": { required: true },
    "params": { required: false },
    "request_id": { required: false },
    "sample": { required: true },
    "target": { required: true },
    "text": { required: false },
    "tool": { required: false },
    "turn": { required: false },
  },
  ExecuteResult: {
    "eval": { required: true },
    "params": { required: false },
    "sample": { required: true },
    "seed": { required: false },
    "skipped": { required: false },
    "target": { required: true },
    "transcript": { required: true, ref: "Transcript" },
    "trial": { required: false },
    "trials": { required: false },
  },
  FinalMetrics: {
    "extra": { required: false },
    "total_cached_tokens": { required: false },
    "total_completion_tokens": { required: false },
    "total_cost_usd": { required: false },
    "total_prompt_tokens": { required: false },
    "total_steps": { required: false },
  },
  ImageSource: {
    "media_type": { required: true },
    "path": { required: true },
  },
  InitializeResult: {
    "capabilities": { required: false },
    "capability_params": { required: false },
    "evals": { required: true },
    "protocol_version": { required: true },
    "study": { required: true },
    "study_version": { required: false },
  },
  ListResult: {
    "evals": { required: true, arrayRef: "EvalInfo" },
  },
  ListSamplesParams: {
    "cursor": { required: true },
    "eval": { required: true },
  },
  ListSamplesResult: {
    "next_cursor": { required: false },
    "samples": { required: true, arrayRef: "SampleInfo" },
  },
  LogParams: {
    "message": { required: true },
    "request_id": { required: false },
  },
  Notification: {
    "method": { required: true },
    "params": { required: false },
  },
  Observation: {
    "results": { required: true, arrayRef: "ObservationResult" },
  },
  ObservationResult: {
    "content": { required: false },
    "extra": { required: false },
    "source_call_id": { required: false },
    "subagent_trajectory_ref": { required: false, arrayRef: "SubagentTrajectoryRef" },
  },
  Request: {
    "id": { required: true },
    "method": { required: true },
    "params": { required: false },
  },
  Response: {
    "error": { required: false },
    "id": { required: true },
    "result": { required: false },
  },
  RpcError: {
    "code": { required: false },
    "data": { required: false },
    "message": { required: true },
    "retryable": { required: false },
  },
  RunParams: {
    "eval": { required: true },
    "params": { required: false },
    "sample": { required: true },
    "seed": { required: false },
    "target": { required: true },
    "trial": { required: false },
    "trials": { required: false },
  },
  RunResult: {
    "aggregate": { required: true },
    "eval": { required: true },
    "expected": { required: false },
    "input": { required: false },
    "params": { required: false },
    "passed": { required: true },
    "sample": { required: true },
    "scores": { required: true, arrayRef: "Score" },
    "seed": { required: false },
    "skipped": { required: false },
    "target": { required: true },
    "transcript": { required: true, ref: "TranscriptSummary" },
    "trial": { required: false },
    "trials": { required: false },
  },
  SampleInfo: {
    "id": { required: true },
    "metadata": { required: false },
    "tags": { required: false },
  },
  Score: {
    "na": { required: false },
    "pass": { required: true },
    "reason": { required: true },
    "scorer": { required: true },
    "value": { required: true },
  },
  ScoreParams: {
    "eval": { required: true },
    "params": { required: false },
    "sample": { required: true },
    "seed": { required: false },
    "target": { required: true },
    "transcript": { required: true, ref: "Transcript" },
    "trial": { required: false },
    "trials": { required: false },
  },
  Step: {
    "extra": { required: false },
    "is_copied_context": { required: false },
    "llm_call_count": { required: false },
    "message": { required: false },
    "metrics": { required: false },
    "model_name": { required: false },
    "observation": { required: false },
    "reasoning_content": { required: false },
    "reasoning_effort": { required: false },
    "source": { required: true },
    "step_id": { required: true },
    "timestamp": { required: false },
    "tool_calls": { required: false, arrayRef: "ToolCall" },
  },
  StepMetrics: {
    "cached_tokens": { required: false },
    "completion_token_ids": { required: false },
    "completion_tokens": { required: false },
    "cost_usd": { required: false },
    "extra": { required: false },
    "logprobs": { required: false },
    "prompt_token_ids": { required: false },
    "prompt_tokens": { required: false },
  },
  SubagentTrajectoryRef: {
    "extra": { required: false },
    "session_id": { required: false },
    "trajectory_id": { required: false },
    "trajectory_path": { required: false },
  },
  TargetInfo: {
    "available": { required: true },
    "label": { required: true },
    "metadata": { required: false },
    "provider": { required: false },
  },
  Timing: {
    "duration_ms": { required: false },
    "time_to_first_token_ms": { required: false },
  },
  ToolCall: {
    "arguments": { required: true },
    "extra": { required: false },
    "function_name": { required: true },
    "tool_call_id": { required: true },
  },
  Trajectory: {
    "agent": { required: true, ref: "Agent" },
    "continued_trajectory_ref": { required: false },
    "extra": { required: false },
    "final_metrics": { required: false },
    "notes": { required: false },
    "schema_version": { required: true },
    "session_id": { required: false },
    "steps": { required: true, arrayRef: "Step" },
    "subagent_trajectories": { required: false, arrayRef: "Trajectory" },
    "trajectory_id": { required: false },
  },
  Transcript: {
    "error": { required: false },
    "error_kind": { required: false },
    "events": { required: false },
    "files": { required: false },
    "final_response": { required: false },
    "iterations": { required: false },
    "metadata": { required: false },
    "metrics": { required: false },
    "output": { required: false },
    "timing": { required: false, ref: "Timing" },
    "tool_calls": { required: false },
    "tool_calls_count": { required: false },
    "trajectory": { required: false },
    "usage": { required: false, ref: "Usage" },
  },
  TranscriptSummary: {
    "error": { required: false },
    "error_kind": { required: false },
    "final_response": { required: true },
    "iterations": { required: true },
    "metadata": { required: false },
    "metrics": { required: false },
    "output": { required: false },
    "timing": { required: false, ref: "Timing" },
    "tool_calls": { required: false },
    "tool_calls_count": { required: true },
    "usage": { required: true, ref: "Usage" },
  },
  Usage: {
    "cache_read_tokens": { required: false },
    "cost_usd": { required: true },
    "input_tokens": { required: true },
    "output_tokens": { required: true },
    "reasoning_tokens": { required: false },
  },
};