// The study side of the protocol: an eval registry plus the stdio serve loop.
//
// A `Study` answers `initialize`/`list`/`list_samples`/`run`/`execute`/`score`
// over newline-delimited JSON on stdio (see docs/protocol.md). stdout carries
// only protocol JSON; logs go to stderr.
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";

import { toWire } from "./codec.js";
import { PROTOCOL_VERSION, SERVED_METHODS } from "./meta.js";
import { MethodNotFound, dispatch, type StudyHandler } from "./protocol.js";
import { makeScore, type Scorer } from "./scorers.js";
import { ATIF_FORMAT, ATIF_VERSION, normalizeTrajectory } from "./trajectory.js";
import type {
  AxisInfo,
  CancelResult,
  EvalInfo,
  ExecuteResult,
  InitializeResult,
  ListResult,
  ListSamplesParams,
  ListSamplesResult,
  RunParams,
  RunResult,
  SampleInfo,
  Score,
  ScoreParams,
  TargetInfo,
  Transcript,
  TranscriptSummary,
} from "./wire.js";

export { PROTOCOL_VERSION };

// JSON-RPC error codes for the structured `error` object (mirrors the Rust
// `protocol::codes`). Every caller mistake here is non-retryable.
const CODE_METHOD_NOT_FOUND = -32601;
const CODE_INVALID_PARAMS = -32602;
const CODE_INTERNAL_ERROR = -32603;

// What `Study.handle` dispatches: the methods a host sends, from the generated
// `SERVED_METHODS`. It used to be a hand-written list here with a test asserting
// it covered the protocol; the list is now the declaration's, so a new method
// arrives in it by regenerating and a test only has to check that each one is
// answered.
export const HANDLED_METHODS = SERVED_METHODS;

// Samples-per-page when paginating `list`. Small studies fit in one page (`list`
// enumerates every sample inline); a huge/lazy dataset is chunked across `list` +
// `list_samples`. Mirrors the Rust `DEFAULT_PAGE_SIZE`.
export const DEFAULT_PAGE_SIZE = 500;

// ----- authoring types -------------------------------------------------------

/** One dataset row. `prompt` is convenience for a single input turn; `input`
 * holds multi-turn input. `text` joins them for the subject. */
export interface Sample {
  id: string;
  prompt?: string;
  input: string[];
  tags: string[];
  expected?: string;
  files: Record<string, string>;
  metadata: Record<string, unknown>;
  /** The prompt, or the input turns joined by newlines — what the subject reads. */
  readonly text: string;
}

export interface SampleOptions {
  prompt?: string;
  input?: string[];
  tags?: string[];
  expected?: string;
  files?: Record<string, string>;
  metadata?: Record<string, unknown>;
}

export function sample(id: string, opts: SampleOptions = {}): Sample {
  const input = opts.input ?? [];
  const prompt = opts.prompt;
  return {
    id,
    prompt,
    input,
    tags: opts.tags ?? [],
    expected: opts.expected,
    files: opts.files ?? {},
    metadata: opts.metadata ?? {},
    text: prompt ?? input.join("\n"),
  };
}

/** A matrix case: the model or harness under evaluation. An unavailable target
 * is reported as N/A (infra), not a failure. */
export interface Target {
  label: string;
  provider: string;
  available: boolean;
  metadata: Record<string, unknown>;
}

export interface TargetOptions {
  provider?: string;
  available?: boolean;
  metadata?: Record<string, unknown>;
}

export function target(label: string, opts: TargetOptions = {}): Target {
  return {
    label,
    provider: opts.provider ?? "",
    available: opts.available ?? true,
    metadata: opts.metadata ?? {},
  };
}

/** Per-case context handed to a subject: the matrix target, turn budget, and the
 * chosen axis values. */
export class RunCx {
  constructor(
    readonly target: string,
    readonly provider: string = "",
    readonly maxTurns: number = 0,
    readonly params: Record<string, string> = {},
  ) {}

  param(name: string, dflt = ""): string {
    return this.params[name] ?? dflt;
  }
}

export type Subject = (sample: Sample, cx: RunCx) => Transcript | Promise<Transcript>;

export interface EvalOptions {
  name: string;
  samples: Sample[];
  targets: Target[];
  run: Subject;
  scorers?: Scorer[];
  description?: string;
  axes?: AxisInfo[];
  maxTurns?: number;
  metadata?: Record<string, unknown>;
}

class Eval {
  readonly name: string;
  readonly subject: Subject;
  readonly samples: Sample[];
  readonly targets: Target[];
  readonly scorers: Scorer[];
  readonly description: string;
  readonly axes: AxisInfo[];
  readonly maxTurns: number;
  readonly metadata: Record<string, unknown>;

  constructor(opts: EvalOptions) {
    this.name = opts.name;
    this.subject = opts.run;
    this.samples = opts.samples;
    this.targets = opts.targets;
    this.scorers = opts.scorers ?? [];
    this.description = opts.description ?? "";
    this.axes = opts.axes ?? [];
    this.maxTurns = opts.maxTurns ?? 0;
    this.metadata = opts.metadata ?? {};
  }

  info(): EvalInfo {
    return {
      name: this.name,
      description: this.description,
      samples: this.samples.map(sampleInfo),
      scorers: this.scorers.map((s) => s.name),
      targets: this.targets.map(
        (m): TargetInfo => ({
          label: m.label,
          provider: m.provider,
          available: m.available,
          metadata: m.metadata,
        }),
      ),
      axes: this.axes.map((a): AxisInfo => ({ name: a.name, values: [...a.values] })),
      max_turns: this.maxTurns,
      metadata: this.metadata,
    };
  }

  sample(id: string): Sample {
    const s = this.samples.find((x) => x.id === id);
    if (!s) throw new Error(`no such sample: ${id}`);
    return s;
  }

  target(label: string): Target {
    return this.targets.find((m) => m.label === label) ?? target(label);
  }
}

function sampleInfo(s: Sample): SampleInfo {
  return { id: s.id, tags: [...s.tags], metadata: s.metadata };
}

// ----- scoring (mirrors crate::runner) ---------------------------------------

function scoreTranscript(ev: Eval, sample: Sample, t: Transcript): Score[] {
  // Infra failure short-circuits to a single N/A, like score_transcript().
  if (t.error != null && t.error_kind === "infra") {
    return [makeScore("infra", 0, false, t.error, true)];
  }
  return ev.scorers.map((sc) => sc.score(sample, t));
}

export function verdict(scores: Score[]): boolean {
  const applicable = scores.filter((s) => !s.na);
  return applicable.length > 0 && applicable.every((s) => s.pass);
}

export function aggregate(scores: Score[]): number {
  const values = scores.filter((s) => !s.na).map((s) => s.value);
  return values.length ? values.reduce((a, b) => a + b, 0) / values.length : 0;
}

function summary(t: Transcript): TranscriptSummary {
  return {
    final_response: t.final_response ?? "",
    iterations: t.iterations ?? 0,
    tool_calls_count: t.tool_calls_count ?? 0,
    tool_calls: [...(t.tool_calls ?? [])],
    usage: t.usage ?? { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
    timing: t.timing,
    metrics: { ...(t.metrics ?? {}) },
    metadata: { ...(t.metadata ?? {}) },
    output: [...(t.output ?? [])],
    error: t.error,
    error_kind: t.error_kind,
  };
}

// ----- study + serve loop ----------------------------------------------------

export interface StudyOptions {
  version?: string;
  /** Max samples per `list`/`list_samples` page. `0` disables pagination. */
  pageSize?: number;
}

export interface ServeOptions {
  input?: Readable;
  output?: Writable;
}

export class Study implements StudyHandler {
  readonly name: string;
  readonly version?: string;
  private readonly pageSize: number | null;
  private readonly evals = new Map<string, Eval>();

  constructor(name: string, opts: StudyOptions = {}) {
    this.name = name;
    this.version = opts.version;
    const ps = opts.pageSize ?? DEFAULT_PAGE_SIZE;
    this.pageSize = ps > 0 ? ps : null;
  }

  /** Register a subject `run(sample, cx) -> Transcript` as an eval. */
  eval(opts: EvalOptions): this {
    this.evals.set(opts.name, new Eval(opts));
    return this;
  }

  private capabilities(): string[] {
    const caps = ["usage", "execute", "score", "paginate", "trajectory"];
    if ([...this.evals.values()].some((e) => e.axes.length)) caps.unshift("axes");
    return caps;
  }

  /** One page of `ev`'s samples from `offset`, plus the cursor for the page after
   * it (`null` once exhausted). Mirrors crate::study::Study::sample_page. */
  private samplePage(ev: Eval, offset: number): [SampleInfo[], string | null] {
    const start = Math.min(offset, ev.samples.length);
    const end =
      this.pageSize === null ? ev.samples.length : Math.min(start + this.pageSize, ev.samples.length);
    const page = ev.samples.slice(start, end).map(sampleInfo);
    return [page, end < ev.samples.length ? String(end) : null];
  }

  private evalInfo(ev: Eval): EvalInfo {
    const info = ev.info();
    const [samples, next] = this.samplePage(ev, 0);
    info.samples = samples;
    info.next_cursor = next;
    return info;
  }

  private getEval(name: string): Eval {
    const ev = this.evals.get(name);
    if (!ev) throw new Error(`no such eval: ${name}`);
    return ev;
  }

  list_samples(params: ListSamplesParams): ListSamplesResult {
    const ev = this.getEval(params.eval);
    const offset = Number(params.cursor);
    if (!Number.isInteger(offset)) throw new Error(`bad cursor: ${params.cursor}`);
    const [samples, next] = this.samplePage(ev, offset);
    return { samples, next_cursor: next } as ListSamplesResult;
  }

  /** Run one case's subject. Returns [transcript, skipped]; an unavailable target
   * is skipped with an infra-error transcript (scored N/A, not failed). */
  private async runSubject(params: RunParams): Promise<[Transcript, boolean]> {
    const ev = this.getEval(params.eval);
    const s = ev.sample(params.sample);
    const m = ev.target(params.target);
    if (!m.available) {
      return [
        {
          final_response: "",
          iterations: 0,
          tool_calls_count: 0,
          usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
          error: `target unavailable: ${m.label}`,
          error_kind: "infra",
        },
        true,
      ];
    }
    const cx = new RunCx(m.label, m.provider, ev.maxTurns, params.params ?? {});
    // Zero-burden trajectory contract: a subject may set only
    // `transcript.trajectory`; the flat fields are projected here
    // (fill-if-default — explicitly set fields win).
    return [normalizeTrajectory(await ev.subject(s, cx)), false];
  }

  initialize(): InitializeResult {
    return {
      protocol_version: PROTOCOL_VERSION,
      study: this.name,
      evals: this.evals.size,
      study_version: this.version,
      capabilities: this.capabilities(),
      capability_params: {
        // The trajectory representation this study emits (readers are
        // more lenient — any ATIF-v1.x parses).
        trajectory: { format: ATIF_FORMAT, version: ATIF_VERSION.replace(/^ATIF-v/, "") },
      },
    } as InitializeResult;
  }

  list(): ListResult {
    return { evals: [...this.evals.values()].map((e) => this.evalInfo(e)) } as ListResult;
  }

  cancel(): CancelResult {
    // The serve loop is synchronous per request: there is never a
    // concurrently in-flight run to abort, so cancel is a benign no-op
    // (best-effort, like the protocol allows). Answered so the method isn't
    // "unknown"; the `cancel` capability is left unadvertised.
    return { cancelled: false };
  }

  async execute(params: RunParams): Promise<ExecuteResult> {
    const [transcript, skipped] = await this.runSubject(params);
    return {
      eval: params.eval,
      sample: params.sample,
      target: params.target,
      params: params.params ?? {},
      transcript,
      skipped,
    } as ExecuteResult;
  }

  async run(params: RunParams): Promise<RunResult> {
    const [transcript, skipped] = await this.runSubject(params);
    return this.scored(params, transcript, skipped);
  }

  score(params: ScoreParams): RunResult {
    // Normalize on receipt: a replayed transcript may be trajectory-only;
    // name-based scorers then see the projections.
    return this.scored(params, normalizeTrajectory(params.transcript as Transcript), false);
  }

  /** Score a transcript for one case. Shared by `run` and `score`, whose params
   * types differ only in how the transcript was obtained. */
  private scored(
    params: RunParams | ScoreParams,
    transcript: Transcript,
    skipped: boolean,
  ): RunResult {
    const ev = this.getEval(params.eval);
    const s = ev.sample(params.sample);
    const scores = scoreTranscript(ev, s, transcript);
    return {
      eval: params.eval,
      sample: params.sample,
      target: params.target,
      params: params.params ?? {},
      passed: verdict(scores),
      aggregate: aggregate(scores),
      scores,
      transcript: summary(transcript),
      skipped,
    } as RunResult;
  }

  /**
   * Answer one request, as JSON in and JSON out.
   *
   * The cast/call/encode is the generated `dispatch`, which is exhaustive over
   * the declaration: this used to be a `switch` with its own `toWire("...")`
   * literal and `as string` casts per branch, and a method missing from it fell
   * through to a hand-written "unknown method".
   */
  async handle(
    method: string,
    params: Record<string, unknown> = {},
  ): Promise<Record<string, unknown>> {
    return dispatch(this, method, params);
  }

  /** Drive this study over newline-delimited JSON until stdin EOF. */
  serve(opts: ServeOptions = {}): Promise<void> {
    return serve(this, opts);
  }
}

function rpcError(err: unknown): { code: number; message: string } {
  const message = err instanceof Error ? err.message : String(err);
  // By type, not by sniffing the message text: the generated dispatch throws
  // `MethodNotFound` for anything outside the declaration.
  if (err instanceof MethodNotFound) return { code: CODE_METHOD_NOT_FOUND, message };
  if (message.startsWith("no such ") || message.startsWith("bad cursor")) {
    return { code: CODE_INVALID_PARAMS, message };
  }
  return { code: CODE_INTERNAL_ERROR, message };
}

export function log(msg: string): void {
  process.stderr.write(msg + "\n");
}

/**
 * Drive `study` over newline-delimited JSON. One object per line in; one
 * Response/Notification per line out. Resolves when the input stream ends.
 */
export async function serve(study: Study, opts: ServeOptions = {}): Promise<void> {
  const input = opts.input ?? process.stdin;
  const output = opts.output ?? process.stdout;
  const emit = (obj: unknown) => output.write(JSON.stringify(obj) + "\n");

  const rl = createInterface({ input, crlfDelay: Infinity });
  for await (const raw of rl) {
    const line = raw.trim();
    if (!line) continue;
    let msg: { id?: unknown; method?: unknown; params?: unknown };
    try {
      msg = JSON.parse(line);
    } catch {
      emit({ method: "log", params: { message: "bad json" } });
      continue;
    }
    const id = msg.id;
    try {
      const result = await study.handle(
        msg.method as string,
        (msg.params as Record<string, unknown>) ?? {},
      );
      emit({ id, result });
    } catch (err) {
      // Report, don't crash the loop.
      emit({ id, error: rpcError(err) });
    }
  }
  log(`${study.name}: stdin closed, exiting`);
}
