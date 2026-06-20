# Mira examples

Runnable, **offline** example eval servers. Each lives in its own subfolder
(`examples/<name>/main.rs`) and is a standalone program that defines evals and
calls `mira::serve_registered()`; drive them with the host CLI. They run against
the `sim` model with no API keys, so they stay green in CI and cost nothing.

```bash
# list / run any example (replace `greet` with any below)
cargo run -p mira-cli -- --package mira-examples --example greet list
cargo run -p mira-cli -- --package mira-examples --example greet run
```

| Example | Shows |
|---------|-------|
| `greet` | The smallest eval: `#[eval]`, a closure subject, text + LLM-judge scorers. |
| `coding` | Seeded files, a model matrix, structural + file-based scorers. |
| `cli_subject` | The polyglot path — evaluating an external binary. |
| `metrics` | Operational budgets: tokens, cost, latency, TTFT, exact/ordered tool use. |
| `matrix` | A multi-axis matrix: models × a custom `effort` axis (cross-product). |
| `swe_bench` | A SWE-bench-style bug-fix eval with a `FAIL_TO_PASS` gate scorer. |
| `llmsim` | Driving a real `everruns-runtime` session against the offline `LlmSim` driver. |

Render a self-contained HTML report from any of them:

```bash
cargo run -p mira-cli -- --package mira-examples --example metrics \
  run --format html --out report.html
```
