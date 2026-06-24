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
        .add_sample(Sample::new("france", "What is the capital of France?").expected("Paris"))
        .add_sample(Sample::new("japan", "What is the capital of Japan?").expected("Tokyo"))
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

The cloud case is **unavailable** because `ANTHROPIC_API_KEY` isn't set — it will
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

A case that *does* run but hits **infrastructure** trouble (out of budget,
rate-limited, a provider outage) is scored **N/A** rather than failed — it's not
the model's fault. N/A cases are excluded from the pass-rate and retried; see
[Infrastructure errors vs. failures](authoring.md#infrastructure-errors-vs-failures).

Set `ANTHROPIC_API_KEY` and the cloud column lights up too.

## 4. Select, report, resume

```bash
mira --example my_evals run france                 # substring grep on the case key
mira --example my_evals run --samples 'geo/*'      # glob on sample ids
mira --example my_evals run --tag smoke            # by sample tag
mira --example my_evals run --targets 'anthropic/*' # glob on target labels
mira --example my_evals run --format junit --out results.xml   # CI artifact
mira --example my_evals run --format html  --out report.html   # self-contained viewer
mira --example my_evals run                        # saves a run folder by default
mira --example my_evals run --dry-run              # ephemeral; don't save a run folder
mira --example my_evals run --resume <run_id>      # reopen a run; run only the missing cases
mira report <run_id>                               # re-render a saved run's reports
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

The exit code is non-zero if any case failed, so `mira ... run` drops straight
into CI. The HTML report is a single dependency-free file (summary, matrix, and
per-case scores/usage/timing) you can open straight from a CI artifact.

On an interactive terminal a live progress bar shows `done/total`, elapsed time,
and an ETA as cases complete; it's hidden under CI/non-TTY so it never pollutes
logs.

Every `mira run` (and `mira score`) **saves a run folder by default** under the
results dir, unless you pass `--dry-run`. Each run lands in
`<results_dir>/<run_id>/`:

- `meta.json` — run identity: id, study, start/finish timestamps, summary, and
  the **environment** the run came from (see below). Written as a header when
  the run starts, then rewritten at the end with the finish time and summary.
- `report.json` — the canonical machine-readable record (summary + per case),
- `report.html` — the self-contained transcript viewer,
- `cases/<encoded-key>/result.json` — one finished case
  (`eval/sample@target[…]#trial`), written atomically as that case completes.

A fresh `mira run` mints a new id and reuses nothing — no silent reuse of stale
results. To continue a run, name it explicitly: `--resume <run_id>` reopens that
run folder, skips the cases already recorded under `cases/`, and runs only
what's missing.

The results dir is `[results].dir` from the nearest `mira.toml`, else
`./results`. A `mira.toml` at the repo root sets the default for everyone:

```toml
[results]
dir = "./results"   # where saved run folders go
```

`mira report <run_id>` re-renders a saved run's reports from its stored
`cases/*/result.json` — no study process is spawned, nothing is re-executed.

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
