# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-06-20

Initial release.

### Added

- **Core framework (`mira-eval`, library `mira`)**
  - `Eval = Dataset(Sample…) + Subject + [Scorer…] × model matrix` model.
  - `Sample` / `Dataset` with inline authoring and JSONL / JSON loaders;
    seeded files, tags, targets, and free-form metadata.
  - Provider-agnostic `ModelSpec` (sim, anthropic, openai, gemini, custom) with
    API-key availability gating — unavailable cells skip rather than fail.
  - `Subject` trait with `subject_fn` (in-process) and `CliSubject` (the
    polyglot path: stdout or canonical JSONL `Event` transcripts, file
    seed/capture).
  - `Scorer` trait: deterministic built-ins (`contains`, `not_contains`,
    `equals`, `regex`, `matches_target`, `tool_called`, `tool_calls_within`,
    `turns_within`, `cost_within`, `file_exists`, `file_contains`, `succeeded`),
    a `scorer(name, closure)` escape hatch, and `model_graded` LLM-as-judge.
  - `register_eval!` + `serve_registered()` for `cargo test`-style discovery.
  - The eval protocol (newline-delimited JSON over stdio): `serve` (server) and
    `Host` (host), with `initialize` / `list` / `run` and progress
    notifications.
  - In-process `Runner` with substring / tag / model selection.
  - Reporting: terminal matrix, canonical JSON, JUnit XML, Markdown.
- **Host CLI (`mira-cli`, binary `mira`)** — `list` / `run` with selection,
  `--models`, `--format json|junit|md`, `--out`, and resumable `--checkpoint`.
- **everruns adapter (`mira-everruns`)** — `RuntimeSubject` over the published
  `everruns-runtime`, plus `model_to_resolved`.
- **Docs** — getting started, authoring, scorers, subjects, and a full protocol
  reference (`docs/protocol.md`).
- **Examples** — `greet`, `coding`, `cli_subject`.

[Unreleased]: https://github.com/everruns/mira/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/everruns/mira/releases/tag/v0.1.0
