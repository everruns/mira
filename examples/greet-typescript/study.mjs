#!/usr/bin/env node
// A Mira eval study written in TypeScript-land with the Mira TypeScript SDK.
//
// This is the polyglot seam: Mira's host speaks newline-delimited JSON over
// stdio to a child process, so an eval study can be written in any language.
// This one mirrors the Rust `greet` example. Drive it with the host CLI:
//
//     mira --cmd "node examples/greet-typescript/study.mjs" list
//     mira --cmd "node examples/greet-typescript/study.mjs" run
//
// The SDK has no Rust dependency — its wire types are generated from the
// protocol JSON Schema (schema/v1/). stdout carries ONLY protocol JSON; logs go
// to stderr.
//
// It imports the in-repo SDK build directly so the example runs in CI without a
// publish; a real project would `npm install mira-eval` and import the
// package name. Build the SDK first: `npm --prefix sdks/typescript ci && npm
// --prefix sdks/typescript run build`.
import { Study, sample, target, succeeded, contains, transcript, usage, timing } from "../../sdks/typescript/dist/index.js";

const study = new Study("greet-typescript", { version: "0.1.0" });

study.eval({
  name: "greet",
  description: "Greets the user and reports the answer to life (TypeScript SDK study)",
  samples: [sample("hi", { prompt: "Say hi and tell me the answer to life.", tags: ["smoke"] })],
  targets: [target("sim")],
  scorers: [succeeded(), contains("42")],
  metadata: { suite: "smoke", lang: "typescript" },
  run: (s) => {
    // A real subject would call a model; this one fakes a good answer.
    const response = `Hi! In response to ${JSON.stringify(s.text)}: the answer is 42.`;
    const outTokens = response.split(/\s+/).length;
    return transcript(response, {
      iterations: 1,
      usage: usage({ inputTokens: 40 + outTokens * 3, outputTokens: outTokens }),
      timing: timing({ durationMs: 60 + outTokens * 4 }),
    });
  },
});

study.serve();
