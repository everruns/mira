# mira-cli

The `mira` **host CLI** for [Mira](https://github.com/everruns/mira), a
Rust-first, code-first evaluation framework for agents and tools. It compiles +
spawns an eval **study** (a program built on
[`mira-eval`](https://crates.io/crates/mira-eval)), plans the run across the
model matrix, executes each case over the protocol, scores it, and reports.

[![crates.io](https://img.shields.io/crates/v/mira-cli.svg)](https://crates.io/crates/mira-cli)
[![docs.rs](https://img.shields.io/docsrs/mira-eval)](https://docs.rs/mira-eval)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

## Install

Via Homebrew (recommended) — installs the `mira` binary:

```bash
brew install everruns/tap/mira
```

Works on macOS (arm64 / x86_64) and Linux (x86_64). If your Homebrew enforces
tap trust checks, trust the tap once first:

```bash
brew trust --tap everruns/tap   # only if your Homebrew requires it
```

Other ways to install:

```bash
cargo binstall mira-cli         # prebuilt release tarball, no compile
cargo install mira-cli --locked # build from source
```

All three install the same `mira` binary. Verify with `mira --version`.

## How it works

The host and your study run as **two processes talking newline-delimited JSON
over stdio** (MCP-style). Your study owns the evals, subjects, and scoring — and
your provider API keys, which never cross the wire. The `mira` CLI owns
everything operational: selection, the model matrix, concurrency, saved runs,
and reporting.

```text
        you author                       the `mira` CLI does the rest
  ┌────────────────────────┐
  │  study (mira-eval)     │
  │  evals + subjects      │   mira run --study study.rs --targets … --axis … --tag …
  │  + scorers             │                       │
  │  Rust · Python · TS    │                       ▼
  └───────────┬────────────┘   ┌───────────────────────────────────────────────┐
              │                 │ 1. spawn      compile + launch the study      │
   compiles + │ spawns          │ 2. initialize protocol_version · capabilities │
              └────────────────▶│ 3. list       evals · samples · axes · targets│
                                │ 4. plan grid  cases = evals × targets         │
        ┌───────── stdio ──────▶│               × axes × samples                │
        │  run · event · log    │ 5. execute    run each case, concurrent,      │
        │  RunResult · scores   │               retry · throttle                │
        ▼                       │ 6. score      scorers ⇒ pass / fail           │
  ┌────────────┐                │               missing API key ⇒ skipped (N/A) │
  │  study     │◀── per case ───│ 7. report     terminal · JSON · HTML · JUnit  │
  │  subject → │                │               · md  (non-zero exit ⇒ CI gate) │
  │  model     │                │ 8. save run   ./results/<run_id>/             │
  └────────────┘                └───────────────────────────────────────────────┘
                                       saved run ──▶ mira run --resume · mira report
```

A single run reads as a conversation over one pipe: the host handshakes
(`initialize`), enumerates (`list`), plans the grid, then drives a `run` per
case while the study streams `event`/`log` notifications back. Richer renderings
of these flows live in the docs:

- [Run lifecycle (sequence)](https://github.com/everruns/mira/blob/main/docs/assets/mira-run-lifecycle.svg)
  — host ↔ study over one stdio pipe.
- [Author → plan → execute → score → report](https://github.com/everruns/mira/blob/main/docs/assets/mira-workflow.svg)
  — what you write vs. what the host does for you.

## Usage

```bash
mira list --study study.rs                          # what the study advertises

mira run --study study.rs                           # whole matrix (sim runs; keyed cases skip)
mira run --study study.rs greet                     # selective (substring), like cargo test
mira run --study study.rs --tag smoke               # filter by tag
mira run --study study.rs --targets sim             # subset the matrix by target
mira run --study study.rs --axis effort=low         # subset an arbitrary axis

mira run --study study.rs --format junit --out results.xml   # CI-friendly output
mira run --study study.rs --format html  --out report.html   # self-contained viewer

mira run --study study.rs                           # saves ./results/<run_id>/ by default
mira run --study study.rs --dry-run                 # ephemeral; don't save a run folder
mira run --study study.rs --resume <run_id>         # finish an interrupted run (missing cases only)
mira report <run_id>                                # re-render a saved run's reports
```

Execution and scoring can be **split** — handy for long-running subjects whose
transcripts take minutes to play out:

```bash
mira run --study study.rs --execute-only --artifacts art/   # capture one transcript per case
mira score --study study.rs --artifacts art/                # score (or re-score) without re-running
```

## Pointing it at a study

`--study PATH` resolves the runner by extension: `study.rs` runs as a
single-file Rust study (deps in cargo-script frontmatter, no `Cargo.toml`),
`study.py` runs via `uv run`. When the extension isn't enough, name the runner:

| Flag | Study |
|------|-------|
| `--study PATH` | resolved by extension: `.rs` single-file Rust, `.py` via `uv run` |
| `--study-script PATH` | a single-file Rust study (cargo-script frontmatter, shimmed onto stable) |
| `--study-bin NAME` | a Rust eval crate exposing a like-named binary |
| `--study-example NAME` | a workspace example study |
| `--package` / `--manifest-path` | another Cargo package |
| `--study-cmd "..."` | an arbitrary command (any language) |
| `--study-uv` / `--study-python SCRIPT` | a non-Rust (e.g. Python) study |

Save a repeated invocation as `[launchers.NAME]` in `mira.toml` and select it
with `--launcher NAME` (or a `default_launcher`) instead of retyping the flags.
Run folders default to `./results/`; configure via `[results].dir` in
`mira.toml`.

## Learn more

- [Getting started](https://github.com/everruns/mira/blob/main/docs/getting-started.md)
- [How it works](https://github.com/everruns/mira/blob/main/docs/how-it-works.md)
- [The eval protocol](https://github.com/everruns/mira/blob/main/docs/protocol.md)

Licensed under MIT — see [LICENSE](../../LICENSE).
