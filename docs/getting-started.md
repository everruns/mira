# Getting started

This walks you from zero to a passing eval run.

## 1. Install

The framework is a library (`mira-eval`, imported as `mira`); the runner is a
binary (`mira-cli`, installed as `mira`).

```bash
cargo add mira-eval
brew install everruns/tap/mira      # or: cargo install mira-cli
cargo binstall mira-cli             # prebuilt binary, no compile (installs `mira`)
```

## 2. Write an eval study

An eval **study** is just a program that defines evals and calls
`mira::Study::registered().serve()`. Put it anywhere `cargo run` can reach it — a
binary, or (handy for libraries) an example:

```rust
// examples/my_evals.rs
use mira::scorer::{contains, succeeded, tool_called};
use mira::subject::subject_fn;
use mira::{eval, Eval, Target, Sample, Transcript};

#[eval]
fn capital() -> Eval {
    Eval::new("capital")
        .describe("Knows world capitals")
        .sample(Sample::new("france", "What is the capital of France?").expected("Paris"))
        .sample(Sample::new("japan", "What is the capital of Japan?").expected("Tokyo"))
        .subject(subject_fn(|sample, cx| async move {
            // Replace this with a real model call keyed on `cx.target`.
            let answer = match sample.id.as_str() {
                "france" => "The capital of France is Paris.",
                _ => "The capital of Japan is Tokyo.",
            };
            let _ = cx; // model is available as cx.target
            Transcript::response(answer)
        }))
        .scorer(succeeded())
        .scorer(mira::scorer::matches_expected()) // compares to Sample.expected
        .targets([Target::sim(), Target::anthropic("claude-opus-4-8")])
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    mira::Study::registered().serve().await
}
```

## 3. Run it

```bash
mira --example my_evals list
```

```text
capital — Knows world capitals  (max_turns=12)
  samples: france, japan
  scorers: succeeded, matches_expected
  targets:  sim, anthropic/claude-opus-4-8 (unavailable)
```

The cloud cell is **unavailable** because `ANTHROPIC_API_KEY` isn't set — it will
be skipped, not failed. Run the matrix:

```bash
mira --example my_evals run
```

```text
── matrix (passed/ran) ──
  eval         sim  anthropic/claude-opus-4-8
  capital      2/2                          —

2 passed / 2 scored (0 failed, 0 n/a, 2 skipped)
```

A cell that *does* run but hits **infrastructure** trouble (out of budget,
rate-limited, a provider outage) is scored **N/A** rather than failed — it's not
the model's fault. N/A cells are excluded from the pass-rate and retried; see
[Infrastructure errors vs. failures](authoring.md#infrastructure-errors-vs-failures).

Set `ANTHROPIC_API_KEY` and the cloud column lights up too.

## 4. Select, report, resume

```bash
mira --example my_evals run france                 # substring filter on the case key
mira --example my_evals run --tag smoke            # by sample tag
mira --example my_evals run --targets sim           # restrict the matrix
mira --example my_evals run --format junit --out results.xml   # CI artifact
mira --example my_evals run --format html  --out report.html   # self-contained viewer
mira --example my_evals run --checkpoint ck.json   # resumable; re-run skips done cells
mira --example my_evals run --save                 # archive this run under ./results/<run_id>/
```

Tired of retyping `--example my_evals`? Save it as a **named launcher** in
`mira.toml` and select it with `--launcher`, or set a `default_launcher` so a
bare `mira run` just works:

```toml
[launchers.evals]
example = "my_evals"   # or bin = "…" / cmd = "python study.py"

default_launcher = "evals"
```

```bash
mira run               # uses default_launcher
mira --launcher evals run
```

Explicit launch flags still override the named launcher (handy for a one-off
`--bin other`).

The exit code is non-zero if any cell failed, so `mira ... run` drops straight
into CI. The HTML report is a single dependency-free file (summary, matrix, and
per-case scores/usage/timing) you can open straight from a CI artifact.

On an interactive terminal a live progress bar shows `done/total`, elapsed time,
and an ETA as cells complete; it's hidden under CI/non-TTY so it never pollutes
logs.

`--checkpoint` writes a **session** record (run metadata + per-cell results),
saved after every cell. Re-running with the same path resumes: completed cells
are skipped and the progress bar starts at the right `done/total`. The session
fingerprints each eval's definition, so if you change an eval's scorers, axes,
targets, or metadata, a resume **warns that the cached cells are stale** — re-run
with `--fresh` to recompute from scratch.

`--save` **archives a run** into a timestamped folder so runs accumulate in a
stable place and can be compared later. Each run lands in
`<results_dir>/<run_id>/` (run id is `YYYYMMDDThhmmssZ-xxxx`, sortable by time)
with three files:

- `report.json` — the canonical machine-readable record (summary + per case),
- `report.html` — the self-contained transcript viewer,
- `meta.json` — run identity: id, study, start/finish timestamps, summary, and
  the **environment** the run came from (see below).

With no value, `--save` writes under `./results` (or `[results].dir` from the
nearest `mira.toml`); pass `--save <dir>` to override. A `mira.toml` at the repo
root sets the default for everyone:

```toml
[results]
dir = "./results"   # where `mira run --save` archives runs
```

`mira score --save` archives a re-score the same way. (Listing and diffing past
runs from these records is a planned follow-up.)

### Environment metadata

Every saved run records the context it was produced in, so a result can be
interpreted and compared later — which commit, which box, which host version.
`meta.json` carries an `environment` block:

- **git** — `HEAD` commit, branch, and a `dirty` flag for uncommitted edits,
- **box** — `os`, `arch`, `hostname`, `cpus`, `mem_total_mib`,
- **mira_version** — the host binary that produced the run,
- **labels** — auto-detected CI context (`ci.*`) plus anything you configure.

Capture is **on by default** and best-effort (anything it can't determine is
omitted; it never fails a run). Control it under `[environment]`:

```toml
[environment]
enabled = true            # set false to record no environment block

[environment.labels]      # static labels stamped on every run, for later filtering
team = "search"
region = "us-east-1"
```

Configured labels override auto-detected ones on a key collision.

## Next steps

- [Authoring evals](authoring.md) — datasets, the matrix, extra axes, metadata.
- [Scorers](scorers.md) — the built-ins (incl. metric budgets) and writing your own.
- [Metrics](metrics.md) — tokens/cost/latency, and how to add a custom metric.
- [Subjects](subjects.md) — in-process, CLI/polyglot, and runtime sessions.
- [Extensibility](extensibility.md) — the map of every seam: custom subjects,
  scorers, metrics, events, and protocol-level extension.
- [The protocol](protocol.md) — what flows over the wire, and its versioning.
