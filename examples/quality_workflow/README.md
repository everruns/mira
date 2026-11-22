# Evaluation workflow, end to end

This offline example demonstrates the host workflows using a deterministic
subject and reference-checking judge. It does not measure a real agent or
validate a real LLM judge. Replace those fixtures with your system and a
calibrated grader when applying the workflow to a product.

From the repository root:

```bash
cargo build -p mira-cli
python3 examples/quality_workflow/smoke.py --mira target/debug/mira --out /tmp/mira-smoke
```

The smoke script creates an isolated working directory, runs the real CLI and
Python study, checks expected failures as well as successes, and preserves logs
and reports when `--out` is supplied. It covers:

- A failing baseline followed by a candidate and paired comparison gates.
- Input fingerprints and immutable sample references during rescoring.
- Calibration against the included human-label fixtures and grader-version flips.
- Failure review records that cannot be loaded directly as dataset samples.
- Incomplete coverage in terminal, JSON, HTML, Markdown, JUnit, JSONL, and CSV.
- ATIF export from a saved run.

`experiment.toml`, `labels.jsonl`, and `study.py` are independent inputs you can
inspect and adapt. See [Evaluation quality](../../docs/evaluation-quality.md)
for the API, policies, report contracts, and the support matrix.
