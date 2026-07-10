# greet-python — a Mira eval study in Python

A non-Rust eval study, written with the [Mira Python SDK](../../sdks/python). It
has **no Rust dependency** — the SDK is a native Python library over the
[Mira eval protocol](../../docs/protocol.md) (newline-delimited JSON over stdio),
whose wire types are generated from the protocol JSON Schema. It mirrors the Rust
[`greet`](../greet) example so you can compare them side by side.

The host drives it with the `--python3` launcher (or `--python` / `--uv`, or an
explicit `--cmd "..."`):

```bash
mira list --python3 examples/greet-python/study.py
mira run --python3 examples/greet-python/study.py
```

`study.py` declares one eval with `@study.eval(...)` and a subject that returns a
`mira.Transcript`, then `study.serve()` runs the stdio loop (handling
`initialize`/`list`/`run`/`execute`/`score`). stdout carries only protocol JSON;
logs go to stderr. The example adds the in-repo SDK to `sys.path` so it runs
without an install; a real project would `pip install mira-eval`.

Start here when plugging a Python agent or harness (e.g. a SWE-bench runner)
into Mira as a first-class study.
