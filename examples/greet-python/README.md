# greet-python — a Mira eval study in Python

A non-Rust eval study. It has **no Mira dependency** — it just speaks the
[Mira eval protocol](../../docs/protocol.md) (newline-delimited JSON over stdio),
which any language can implement. It mirrors the Rust [`greet`](../greet)
example so you can compare them side by side.

The host drives it with `--cmd`:

```bash
mira --cmd "python3 examples/greet-python/study.py" list
mira --cmd "python3 examples/greet-python/study.py" run
```

`study.py` is a single file: a stdio loop that answers `initialize`, `list`,
and `run`. stdout carries only protocol JSON; logs go to stderr. Start here when
plugging a Python agent or harness (e.g. a SWE-bench runner) into Mira as a
first-class subject.
