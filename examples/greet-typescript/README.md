# greet-typescript — a Mira eval study in TypeScript

A non-Rust eval study, written with the [Mira TypeScript SDK](../../sdks/typescript).
It has **no Rust dependency** — the SDK is a native TypeScript/Node library over
the [Mira eval protocol](../../docs/protocol.md) (newline-delimited JSON over
stdio), whose wire types are generated from the protocol JSON Schema. It mirrors
the Rust [`greet`](../greet) example so you can compare them side by side.

The host drives it with `--cmd`:

```bash
mira --cmd "node examples/greet-typescript/study.mjs" list
mira --cmd "node examples/greet-typescript/study.mjs" run
```

`study.mjs` declares one eval with `study.eval({...})` and a subject that returns
a `Transcript`, then `study.serve()` runs the stdio loop (handling
`initialize`/`list`/`run`/`execute`/`score`). stdout carries only protocol JSON;
logs go to stderr. The example imports the in-repo SDK **build** directly so it
runs in CI without a publish; a real project would `npm install @everruns/mira-eval`
and import the package name. Build the SDK first:

```bash
npm --prefix sdks/typescript ci
npm --prefix sdks/typescript run build
```

Start here when plugging a TypeScript/Node agent or harness into Mira as a
first-class study.
