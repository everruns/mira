# Mira examples

Runnable, **offline** example studies. Most are **single-file** studies; a few
stay full crates where they need to. Every one runs against the `sim` model with
no API keys, so they stay green in CI and cost nothing.

- **Single-file studies** (`examples/<name>.rs`) carry their dependencies in
  cargo-script frontmatter (RFC 3502) — no `Cargo.toml`. The host runs them with
  `--script <file>`, shimming cargo-script onto **stable** (it's otherwise the
  nightly-only `cargo -Zscript`). See
  [single-file studies](../docs/how-it-works.md#single-file-studies---script).
- **Crate examples** (`examples/<name>/`) are individual crates exposing a
  like-named binary; the host resolves them with `--bin <name>`. Kept as crates
  when they're multi-file or pull heavy provider deps.
- **Polyglot examples** (e.g. [`greet-python`](greet-python)) are plain folders —
  no Cargo.toml — that implement the [protocol](../docs/protocol.md) directly.
  The host launches them with `--cmd "..."` (or `--python3` / `--uv`).

```bash
# Single-file study (deps in frontmatter, run via the cargo-script shim):
cargo run -p mira-cli -- list --script examples/greet.rs
cargo run -p mira-cli -- run --script examples/greet.rs

# Crate example (a workspace bin):
cargo run -p mira-cli -- run --bin metrics

# Polyglot examples (studies in another language, no Mira dependency):
cargo run -p mira-cli -- run --cmd "python3 examples/greet-python/study.py"
cargo run -p mira-cli -- run --cmd "node examples/greet-typescript/study.mjs"
```

| Example | Form | Shows |
|---------|------|-------|
| [`greet`](greet.rs) | Rust · `--script` | The smallest eval: `#[eval]`, a closure subject, text + LLM-judge scorers. |
| [`coding`](coding.rs) | Rust · `--script` | Seeded files, a model matrix, structural + file-based scorers. |
| [`swe_bench`](swe_bench.rs) | Rust · `--script` | A SWE-bench-style bug-fix eval with a `FAIL_TO_PASS` gate scorer. |
| [`trials`](trials.rs) | Rust · `--script` | Trials/repetitions + seed for pass@k / pass-rate / variance. Intentionally flaky, so some trials fail. |
| [`multimodal`](multimodal.rs) | Rust · `--script` | Image/multimodal sample inputs and output. |
| [`interactive`](interactive.rs) | Rust · `--script` | A clarify-then-answer multi-turn dialog subject. |
| [`infra`](infra.rs) | Rust · `--script` | Infrastructure errors vs. failures: an N/A (retried) case vs. a real fail. |
| [`llm_judge`](llm_judge.rs) | Rust · `--script` | Provider-backed LLM-as-judge (`mira-judge`); the judge is N/A without a key, so it stays green offline. |
| [`cli_subject`](cli_subject) | Rust · `--bin` | The polyglot subject path — driving an external program ([`subject.sh`](cli_subject/subject.sh)); stays a crate for its sibling script. |
| [`metrics`](metrics) | Rust · `--bin` | Operational budgets: tokens, cost, latency, TTFT, exact/ordered tool use. Multi-file crate. |
| [`matrix`](matrix) | Rust · `--bin` | A multi-axis matrix: targets × a custom `effort` axis. Multi-file crate. |
| [`llmsim`](llmsim) | Rust · `--bin` | Driving a real `everruns-runtime` session against the offline `LlmSim` driver (heavy dep). |
| [`greet-python`](greet-python) | Python · `--cmd` | A whole eval **study** in another language, via the [Python SDK](../sdks/python). |
| [`greet-typescript`](greet-typescript) | TypeScript · `--cmd` | The same study via the [TypeScript SDK](../sdks/typescript). |

Render a self-contained HTML report from any of them:

```bash
cargo run -p mira-cli -- run --script examples/greet.rs --format html --out report.html
```
