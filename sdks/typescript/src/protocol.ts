// Typed study surface and dispatch — GENERATED, do not edit.
//
// Regenerate with `node codegen.mjs` from schema/v1/. CI runs
// `node codegen.mjs --check` to fail on drift.

import { toWire } from "./codec.js";
import type { CancelParams, CancelResult, ExecuteResult, InitializeParams, InitializeResult, ListResult, ListSamplesParams, ListSamplesResult, RunParams, RunResult, ScoreParams } from "./wire.js";

/** A method this study does not answer. Becomes a -32601 response. */
export class MethodNotFound extends Error {
  constructor(public readonly method: string) {
    super(`unknown method: ${method}`);
  }
}

/**
 * What a study answers: the methods the host sends.
 *
 * Every method is optional, so a study implements only what it actually
 * handles and an unimplemented one refuses politely. Adding a method to the
 * protocol adds it here, which is how an unhandled one shows up as a refusal
 * rather than as a silently missing branch in a hand-written switch.
 */
export interface StudyHandler {
  /** Announce the host and learn what the study is and can do. */
  initialize?(params: InitializeParams): InitializeResult | Promise<InitializeResult>;
  /** The eval catalogue, with the first page of each eval's samples inline. */
  list?(): ListResult | Promise<ListResult>;
  /** The next page of one eval's samples, for datasets too large (or too lazy) to enumerate in one `list`. */
  list_samples?(params: ListSamplesParams): ListSamplesResult | Promise<ListSamplesResult>;
  /** Execute and score one matrix case in a single call. */
  run?(params: RunParams): RunResult | Promise<RunResult>;
  /** Execute one case's subject without scoring, returning the full transcript, for run-now-score-later workflows. */
  execute?(params: RunParams): ExecuteResult | Promise<ExecuteResult>;
  /** Score a supplied transcript without re-executing the subject, for deferred scoring and re-scoring. */
  score?(params: ScoreParams): RunResult | Promise<RunResult>;
  /** Abort one in-flight `run`/`execute`/`score` by its request `id`.  A request rather than a notification, and acknowledged: the host learns whether the run was still in flight. That divergence from other protocols, where cancel is fire-and-forget, is why lanok's cancellation is a hook the protocol fills rather than a setting. */
  cancel?(params: CancelParams): CancelResult | Promise<CancelResult>;
}

/**
 * Call the handler for `method` and encode its result.
 *
 * Exhaustive over the declaration: a method that is not in it, or one the
 * handler leaves unimplemented, throws `MethodNotFound`.
 */
export async function dispatch(
  handler: StudyHandler,
  method: string,
  params: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  switch (method) {
    case "initialize":
      if (!handler.initialize) break;
      return toWire("InitializeResult", (await handler.initialize(params as unknown as InitializeParams)) as unknown as Record<string, unknown>);
    case "list":
      if (!handler.list) break;
      return toWire("ListResult", (await handler.list()) as unknown as Record<string, unknown>);
    case "list_samples":
      if (!handler.list_samples) break;
      return toWire("ListSamplesResult", (await handler.list_samples(params as unknown as ListSamplesParams)) as unknown as Record<string, unknown>);
    case "run":
      if (!handler.run) break;
      return toWire("RunResult", (await handler.run(params as unknown as RunParams)) as unknown as Record<string, unknown>);
    case "execute":
      if (!handler.execute) break;
      return toWire("ExecuteResult", (await handler.execute(params as unknown as RunParams)) as unknown as Record<string, unknown>);
    case "score":
      if (!handler.score) break;
      return toWire("RunResult", (await handler.score(params as unknown as ScoreParams)) as unknown as Record<string, unknown>);
    case "cancel":
      if (!handler.cancel) break;
      return toWire("CancelResult", (await handler.cancel(params as unknown as CancelParams)) as unknown as Record<string, unknown>);
  }
  throw new MethodNotFound(method);
}
