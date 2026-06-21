# Mira examples

Runnable, **offline** example studies. Each example is a **self-contained
folder**, in any language:

- **Rust examples** are individual crates (`examples/<name>/`) exposing a
  like-named binary. The host resolves them with `--bin <name>`.
- **Polyglot examples** (e.g. [`greet-python`](greet-python)) are plain folders —
  no Cargo.toml — that implement the [protocol](../docs/protocol.md) directly.
  The host launches them with `--cmd "..."`.

They run against the `sim` model with no API keys, so they stay green in CI and
cost nothing.

```bash
# Rust example (a crate exposing the `greet` binary):
cargo run -p mira-cli -- --bin greet list
cargo run -p mira-cli -- --bin greet run

# Polyglot example (a Python study, no Mira dependency):
cargo run -p mira-cli -- --cmd "python3 examples/greet-python/study.py" run
```

| Example | Lang | Shows |
|---------|------|-------|
| [`greet`](greet) | Rust | The smallest eval: `#[eval]`, a closure subject, text + LLM-judge scorers. |
| [`coding`](coding) | Rust | Seeded files, a model matrix, structural + file-based scorers. |
| [`cli_subject`](cli_subject) | Rust | The polyglot subject path — driving an external program ([`subject.sh`](cli_subject/subject.sh)). |
| [`metrics`](metrics) | Rust | Operational budgets: tokens, cost, latency, TTFT, exact/ordered tool use. |
| [`matrix`](matrix) | Rust | A multi-axis matrix: models × a custom `effort` axis (cross-product). |
| [`swe_bench`](swe_bench) | Rust | A SWE-bench-style bug-fix eval with a `FAIL_TO_PASS` gate scorer. |
| [`llmsim`](llmsim) | Rust | Driving a real `everruns-runtime` session against the offline `LlmSim` driver. |
| [`llm_judge`](llm_judge) | Rust | Provider-backed LLM-as-judge (`mira-judge`); the judge is N/A without a key, so it stays green offline. |
| [`greet-python`](greet-python) | Python | A whole eval **study** in another language — the protocol seam, no Mira dependency. |

Render a self-contained HTML report from any of them:

```bash
cargo run -p mira-cli -- --bin metrics run --format html --out report.html
```
