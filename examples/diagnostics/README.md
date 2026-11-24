# Diagnostic and acceptance workflow

This offline example exercises correlated tool evidence, tool order, retrieval
ranking, absolute slice policies, grading resources, coverage categories, and a
human-label review artifact. All outputs and usage are deterministic fixtures;
they do not measure a real model or invoice.

From the repository root:

```bash
cargo build -p mira-cli
python3 examples/diagnostics/smoke.py --mira target/debug/mira --out /tmp/mira-diagnostics
```

The script prints the artifact directory. Open its `review.html` to compare the
baseline and candidate, select a scorer, review disagreements, and export human
labels. Use those labels with `mira calibrate` against the candidate run in that
directory. The script tests that a mismatched execution fingerprint is rejected.

To explore individual commands, run in the printed artifact directory:

```bash
mira run
mira assess RUN --policy /absolute/path/to/mira/examples/diagnostics/policy.toml
mira review CANDIDATE --baseline BASELINE --out review.html
mira calibrate CANDIDATE --labels human-labels.jsonl --scorer judge:fixture
```

Replace `RUN`, `CANDIDATE`, and `BASELINE` with IDs under its `results/` folder.
The generated `smoke.log` contains the exact commands and outputs. `assessment.json`
archives the complete acceptance policy and its measurements; `missing.toml`
introduces a missing robustness category; `budget.toml` demonstrates a failed
post-run grading-spend gate. No acceptance gate cancels live calls.

`study.py` uses the Python SDK's native scorers. The same scorers ship in Rust
and TypeScript, verified against shared conformance vectors. Replace the fixture
subjects with real agents and `judge:fixture` with a calibrated judge; retain the
explicit retrieval IDs, references, scenario/dimension metadata, and measured
resource records. Grounding and conversation-retention judge recipes are described
in [the diagnostic guide](../../docs/diagnostics.md).
