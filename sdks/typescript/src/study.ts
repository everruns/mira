// The study side of the protocol: an eval registry plus the stdio serve loop.
//
// A `Study` answers `initialize`/`list`/`list_samples`/`run`/`execute`/`score`
// over newline-delimited JSON on stdio (see docs/protocol.md). stdout carries
// only protocol JSON; logs go to stderr.
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";

import { toWire } from "./codec.js";
import { PROTOCOL_VERSION } from "./meta.js";
import { makeScore, type Scorer } from "./scorers.js";
import { ATIF_FORMAT, ATIF_VERSION, normalizeTrajectory } from "./trajectory.js";
import type {
  AxisInfo,
  EvalInfo,
  SampleInfo,
  Score,
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

// The protocol methods this SDK dispatches in `Study.handle`. Kept explicit so a
// test can assert it covers every method in the generated `METHODS` — a new
// protocol method then fails CI until the serve loop handles it.
export const HANDLED_METHODS = [
  "initialize",
  "list",
  "list_samples",
  "run",
  "execute",
  "score",
  "cancel",
] as const;

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

export class Study {
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

  private listSamples(params: Record<string, unknown>): Record<string, unknown> {
    const ev = this.getEval(params.eval as string);
    const offset = Number(params.cursor);
    if (!Number.isInteger(offset)) throw new Error(`bad cursor: ${params.cursor}`);
    const [samples, next] = this.samplePage(ev, offset);
    return toWire("ListSamplesResult", { samples, next_cursor: next });
  }

  /** Run one case's subject. Returns [transcript, skipped]; an unavailable target
   * is skipped with an infra-error transcript (scored N/A, not failed). */
  private async execute(params: Record<string, unknown>): Promise<[Transcript, boolean]> {
    const ev = this.getEval(params.eval as string);
    const s = ev.sample(params.sample as string);
    const m = ev.target(params.target as string);
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
    const cx = new RunCx(
      m.label,
      m.provider,
      ev.maxTurns,
      (params.params as Record<string, string>) ?? {},
    );
    // Zero-burden trajectory contract: a subject may set only
    // `transcript.trajectory`; the flat fields are projected here
    // (fill-if-default — explicitly set fields win).
    return [normalizeTrajectory(await ev.subject(s, cx)), false];
  }

  /** Dispatch one protocol request, returning the JSON-ready result. */
  async handle(method: string, params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    switch (method) {
      case "initialize":
        return toWire("InitializeResult", {
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
        });
      case "list":
        return toWire("ListResult", {
          evals: [...this.evals.values()].map((e) => this.evalInfo(e)),
        });
      case "list_samples":
        return this.listSamples(params);
      case "cancel":
        // The serve loop is synchronous per request: there is never a
        // concurrently in-flight run to abort, so cancel is a benign no-op
        // (best-effort, like the protocol allows). Handled so the method isn't
        // "unknown"; the `cancel` capability is left unadvertised.
        return toWire("CancelResult", { cancelled: false });
      case "execute": {
        const [transcript, skipped] = await this.execute(params);
        return toWire("ExecuteResult", {
          eval: params.eval,
          sample: params.sample,
          target: params.target,
          params: params.params ?? {},
          transcript,
          skipped,
        });
      }
      case "run":
      case "score": {
        const ev = this.getEval(params.eval as string);
        const s = ev.sample(params.sample as string);
        let transcript: Transcript;
        let skipped: boolean;
        if (method === "score") {
          // Normalize on receipt: a replayed transcript may be
          // trajectory-only; name-based scorers then see the projections.
          transcript = normalizeTrajectory(params.transcript as Transcript);
          skipped = false;
        } else {
          [transcript, skipped] = await this.execute(params);
        }
        const scores = scoreTranscript(ev, s, transcript);
        return toWire("RunResult", {
          eval: params.eval,
          sample: params.sample,
          target: params.target,
          params: params.params ?? {},
          passed: verdict(scores),
          aggregate: aggregate(scores),
          scores,
          transcript: summary(transcript),
          skipped,
        });
      }
      default:
        throw new Error(`unknown method: ${method}`);
    }
  }

  /** Drive this study over newline-delimited JSON until stdin EOF. */
  serve(opts: ServeOptions = {}): Promise<void> {
    return serve(this, opts);
  }
}

function rpcError(err: unknown): { code: number; message: string } {
  const message = err instanceof Error ? err.message : String(err);
  if (message.startsWith("unknown method")) return { code: CODE_METHOD_NOT_FOUND, message };
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
