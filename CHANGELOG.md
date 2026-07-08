# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **ATIF trajectories: the primary structured trajectory contract (protocol
  1.1).** `Transcript` gains an optional `trajectory` field carrying an
  [ATIF](https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md)
  document (new `mira::trajectory` module: steps with structured tool calls,
  arguments, correlated observations, per-step reasoning and metrics — emitted
  as `ATIF-v1.7`; any `ATIF-v1.x` parses, other prefixes are rejected
  gracefully). The flat fields (`final_response`, `tool_calls`, `iterations`,
  `usage`) are now *projections* of the trajectory: the framework derives them
  wherever a transcript is produced or received (fill-if-default, never
  overwriting explicit values), so a subject or polyglot study can return
  `{"trajectory": …}` alone and every existing scorer keeps working —
  `Transcript::from_trajectory` is the one-call constructor, and
  `Transcript::tool_invocations()` exposes names + arguments + observation
  content (trajectory-first, falling back to the legacy name list). `events` is
  repositioned as an advanced, producer-shaped debug channel — independent of
  and never required alongside the trajectory. The protocol bumps `1.0 → 1.1`
  (additive) with a new `trajectory` capability and
  `capability_params.trajectory = {format: "ATIF", version: "1.7"}`; both SDKs
  gain the generated `Trajectory` wire types, a hand-mirrored projection
  (`mira.trajectory` / `trajectory.ts`), and serve-loop normalization, all
  pinned by a new three-runner conformance fixture
  (`schema/v1/conformance/trajectory.json`).
- **Single-file studies (`--script study.rs`).** A study no longer needs a
  crate: write one `.rs` file with cargo-script frontmatter (RFC 3502) for its
  deps and run it with `mira --script study.rs`. `cargo -Zscript` is nightly-only,
  so the host **shims it onto stable** — it parses the frontmatter, materializes a
  content-hashed throwaway crate (re-anchoring relative `path` deps, adding a
  `[[bin]]` and an isolating `[workspace]`), and `cargo run`s it with a shared
  target dir. The file format matches native cargo-script, so the same study runs
  under `cargo -Zscript` unchanged; `MIRA_SCRIPT_NATIVE=1` opts into that today.
  Most bundled examples (`examples/<name>.rs`) are now single-file; multi-file /
  heavy-dep ones (`cli_subject`, `metrics`, `matrix`, `llmsim`) stay crates.
- **`mira doctor`.** One-command diagnosis of a Mira setup, in three layers:
  `mira.toml` (parse errors, unknown/misspelled keys with a "did you mean"
  suggestion, launcher mistakes, presets and timeouts that can't work), the
  study's advertised listing (duplicate sample ids / target labels / axis
  values that collide case keys, empty datasets and matrices, unavailable
  targets, presets and `[targets.LABEL]` sections that match nothing), and the
  saved-run store (interrupted runs, torn temp files, invalid case results,
  missing reports). `mira doctor --fix` applies the safe repairs: removing
  leftover `*.tmp` files and re-rendering a finished run's missing
  `report.json`/`report.html` from its stored results. Warnings never fail;
  errors exit non-zero, so doctor can gate CI.
- **Publish runs to everruns.** A new `mira-publish-everruns` crate plus
  `mira publish <run_id>` and `mira run --publish everruns` send a saved run's
  results to an [everruns](https://everruns.com) instance, which hosts and
  visualizes eval results it did not execute. Credentials reuse the everruns
  CLI: `--everruns-*` flags, then `EVERRUNS_API_KEY`/`EVERRUNS_API_URL`/
  `EVERRUNS_ORG_ID`, then `~/.config/everruns/credentials.json` — so a prior
  `everruns login` is enough. One Mira run becomes one everruns run group (one
  EvalRun per eval, idempotent on the run id); everruns trusts Mira's verdict
  and does not re-grade.

### Fixed

- **`mira-everruns` reported zero tool calls.** `RuntimeSubject` filled
  `Transcript.tool_calls` via Mira's generic `summarize_events`, which only
  recognizes `{ name, input }` tool objects — a shape the everruns event stream
  never emits (tool calls arrive as `tool.completed` events keyed by
  `data.tool_name`). Every tool-selection scorer silently saw zero calls while
  the tools actually ran. The adapter now extracts tool names from the
  `tool.completed` events it owns, with a regression test pinning that event
  shape against schema drift (EVE-676).
- The repository's own `mira.toml` placed `default_launcher` below the
  `[environment]` section header, which nests it inside that table — so the
  setting was silently ignored (found by `mira doctor`). It now sits at the
  top level.

## [0.3.0] - 2026-06-28

### Added

- **Self-describing results.** A `RunResult` (and the persisted
  `cases/<key>/result.json`) now carries the sample's `input` (the prompt turns
  sent) and `expected` (the reference value, when the dataset provides one), so a
  saved result can be read back without the original dataset. Both are optional
  on the wire — `input` omitted when empty, `expected` when absent.
- **Docs diagrams.** Five new committed SVGs visualise the model, each in its
  topical guide: the **end-to-end workflow** (`mira-workflow.svg` — author →
  plan → execute → score → report) in [`getting-started.md`](docs/getting-started.md),
  the **entity hierarchy** (`mira-entities.svg` — study ▸ eval ▸
  dataset/subject/scorers/targets/axes, expanded into cases · trials ·
  transcripts · scores) in [`authoring.md`](docs/authoring.md), the **host ⇄
  study run lifecycle** (`mira-run-lifecycle.svg` — the protocol sequence for one
  run) in [`how-it-works.md`](docs/how-it-works.md), the **subject fan-in**
  (`mira-subjects.svg` — the three subject shapes normalising into one
  `Transcript`) in [`subjects.md`](docs/subjects.md), and the **scoring flow**
  (`mira-scoring.svg` — transcript surfaces → scorers → case verdict) in
  [`scorers.md`](docs/scorers.md). Indexed in
  [`docs/README.md`](docs/README.md#diagrams).
- **JSONL and CSV report formats** (`--format jsonl` / `--format csv`) for
  un-aggregated, analysis-ready exports. `jsonl` writes one `RunResult` per line
  (lossless — the line-delimited dual of `json`); `csv` is long-format, one row
  per (case × score) with the case columns repeated and open-vocabulary
  `metrics`/`metadata` flattened into stable `metric.*`/`meta.*` columns. Both
  work anywhere `--out`/`--format` do (`run`, `report`, `score`); a `--group-by`
  view is intentionally not folded in — the consumer aggregates the rows.
- Per-case **wall-clock timeout**: give up on a case after a budget of seconds,
  cancelling the in-flight run (best-effort `cancel` over the protocol) and
  recording it as a failed case. Set it on the CLI (`mira run --timeout SECONDS`,
  all targets), per target in `mira.toml` (`[targets.LABEL].timeout`), or as a
  preset default (`[presets.NAME].timeout`). Precedence, first set wins:
  `--timeout` > per-target > preset; unset ⇒ no limit. A timeout is non-retryable
  (retrying would burn the same budget) and counts as a target failure.
- **Glob case selection.** `--targets`, `--samples` (new), and `--evals` (new)
  match the target label / sample id / eval name by glob (`*`, `?`, `[set]`,
  `{a,b}`); a literal value stays an exact match. `--axis` values are globbed
  too. A small dep-free matcher (`mira::glob_match`) backs both the host and the
  in-process `Runner` (`Runner::samples(…)`, glob-aware `Runner::targets(…)`).

### Changed

- **BREAKING (preset):** the preset `filter` key is replaced by per-dimension
  `samples` (glob on sample id). `targets`/`samples`/`evals` in `[presets.NAME]`
  now glob-match and accept either a single string or a list. The cross-cutting
  case-key substring stays available as the positional `mira run [filter]`.

## [0.2.0] - 2026-06-24

### Added

- `skills.sh` — install the Mira agent skill into a Claude Code skills directory
  so an agent can author and run evals. `--global` targets `~/.claude/skills/mira`,
  `--local` (the default) targets `./.claude/skills/mira`. It copies from a local
  checkout when present, else fetches from GitHub raw (`--ref`), so
  `curl -fsSL .../skills.sh | sh` works on a box that only has the prebuilt
  binary. Each run is a clean replace, so it also serves as the upgrade path.
- **Native TypeScript SDK** (`sdks/typescript`, `mira-eval`) — author
  eval studies in TypeScript/Node with no Rust dependency: a zero-runtime-dep
  library over the protocol, with wire types and protocol metadata generated from
  `schema/v1/` (a self-contained `codegen.mjs --check` drift guard, the TS dual of
  the Rust/Python guards), a `serve()` loop (incl. the `execute`/`score` split and
  `list_samples` pagination), a parity authoring API, and conformance + behaviour
  tests. Worked example: `examples/greet-typescript`. Publishes to npm as
  `mira-eval` via OIDC trusted publishing (`publish-typescript` in `publish.yml`),
  mirroring the Python PyPI flow.
- Named launchers in `mira.toml`: `[launchers.NAME]` saves a study invocation
  (`bin`/`example`/`cmd`/`uv`/`python`/`python3` + `package`/`manifest_path`),
  selected with `--launcher NAME`. `default_launcher` makes a bare `mira run`
  work; explicit launch flags override the named launcher, mirroring `--preset`.
- `cargo binstall mira-cli` support: `[package.metadata.binstall]` points binstall
  at the prebuilt release tarballs, so the `mira` binary installs without a compile.
- **Polyglot launcher flags** — `mira --uv` / `--python` / `--python3 SCRIPT`
  drive a non-Rust study directly (e.g. `mira --python3 study.py run`), replacing
  the verbose `--cmd "python3 study.py"`. `--cmd` still works for an arbitrary
  command line.
- `mira help --full` now surfaces a `GUIDES` section (each `docs/` guide with a
  one-line scope, for progressive disclosure) and a link to the `mira` agent skill
  in `LINKS`, so an agent can self-orient to the docs and skill in one read. A
  drift guard keeps the guide list in sync with `docs/README.md`.
- **Run folders, save-by-default, and resume.** Every `mira run`/`mira score` now
  saves a run folder under the results dir by default — `<run_id>/` with
  `meta.json`, `report.json`/`report.html`, and one `cases/<key>/result.json` per
  finished case (written atomically as it lands). `--dry-run` opts out.
- `mira run --resume <run_id>` reopens an interrupted run's folder, skips the cases
  already recorded under `cases/`, and runs only what's missing.
- `mira report <run_id>` — new subcommand that re-renders a saved run's reports
  from its stored per-case results, with no study process and no re-execution.

### Changed

- The execution unit (one `eval × sample × target × axis × trial`) is now called a
  **case** (was "cell"): `Cell`/`CellSpec` → `Case`/`CaseSpec`, `run_cells` →
  `run_cases`, etc. The dataset-row builder `.case(id, prompt)` → `.sample(id,
  prompt)`, and the prebuilt-`Sample` adder `.sample(Sample)` → `.add_sample(Sample)`.
  The `pub type Case = Sample` alias is removed.

### Removed

- `--checkpoint`, `--fresh`, and `--save` on `mira run`/`mira score`, plus the
  `mira::session::Session` type. The single-file checkpoint is superseded by the
  always-saved run folder; resume is now explicit via `--resume <run_id>` (a fresh
  run mints a new id and reuses nothing, so there is no silent stale-result reuse).
  Configure the results dir via `[results].dir` in `mira.toml` (the `--save <dir>`
  override is gone).

## [0.1.0] - 2026-06-22

Initial public release. The crates, the Python SDK, and the protocol all start at
this version.

### Highlights

- **Code-first eval framework** — `Eval = Dataset(Sample…) + Subject + [Scorer…]` crossed with a provider-agnostic `Target` matrix, a broad built-in scorer vocabulary (text, tools, budgets, files, combinators, LLM-judge), and an in-process runner ([#2](https://github.com/everruns/mira/pull/2)).
- **The eval protocol (1.0)** — newline-delimited JSON over stdio between the study and the host, with `MAJOR.MINOR` versioning, capability negotiation, and a machine-readable JSON Schema generated from the wire types ([#16](https://github.com/everruns/mira/pull/16)).
- **Native Python SDK** — author studies in pure-stdlib Python (no Rust dependency); wire types and protocol metadata are generated from the schema with a drift guard ([#22](https://github.com/everruns/mira/pull/22), [#25](https://github.com/everruns/mira/pull/25)).
- **Trials, pass@k, and seeds** — first-class N-sampling for pass-rate and variance with an unbiased pass@k estimator and reproducible per-trial seeds ([#24](https://github.com/everruns/mira/pull/24)).
- **Multimodal & interactive evals** — typed multimodal content (input attachments + graded output) and simulated-user multi-turn dialogs folded into one transcript ([#28](https://github.com/everruns/mira/pull/28)).
- **Provider-backed LLM judge + N/A semantics** — `LlmJudge` scorers over OpenAI/Anthropic and a third "couldn't evaluate" state, so infra failures degrade to N/A instead of a false fail ([#6](https://github.com/everruns/mira/pull/6), [#8](https://github.com/everruns/mira/pull/8)).
- **Adaptive matrix concurrency** — bounded, provider-aware throttling that multiplexes runs over one pipe and backs off on rate limits ([#4](https://github.com/everruns/mira/pull/4)).

### What's Changed

- Targets, not models: rename ModelSpec→Target + --axis/--preset selection ([#34](https://github.com/everruns/mira/pull/34)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): reserve the study→host reverse-request channel seam ([#32](https://github.com/everruns/mira/pull/32)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): cursor-paginated sample listing (1.10) ([#31](https://github.com/everruns/mira/pull/31)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): promote multimodal output + capability params to the wire (1.11) ([#30](https://github.com/everruns/mira/pull/30)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): cancel an in-flight run by id (protocol 1.8) ([#29](https://github.com/everruns/mira/pull/29)) by [@chaliy](https://github.com/chaliy)
- feat: multimodality, interactive multi-turn evals, and structured capability params ([#28](https://github.com/everruns/mira/pull/28)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): typed, correlated event/log notifications (1.9) ([#27](https://github.com/everruns/mira/pull/27)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): metadata columns for samples/models + report --group-by ([#26](https://github.com/everruns/mira/pull/26)) by [@chaliy](https://github.com/chaliy)
- feat(sdks): generate protocol metadata for the Python SDK drift guard ([#25](https://github.com/everruns/mira/pull/25)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): trials/repetitions + seed with pass@k aggregation ([#24](https://github.com/everruns/mira/pull/24)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): structured RPC errors (protocol 1.5) ([#23](https://github.com/everruns/mira/pull/23)) by [@chaliy](https://github.com/chaliy)
- feat(sdks): native Python SDK for authoring eval studies ([#22](https://github.com/everruns/mira/pull/22)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): make metadata open-ended (string → JSON) ([#21](https://github.com/everruns/mira/pull/21)) by [@chaliy](https://github.com/chaliy)
- feat(cli): record environment metadata in saved runs ([#20](https://github.com/everruns/mira/pull/20)) by [@chaliy](https://github.com/chaliy)
- feat(cli): add AI-friendly `mira help --full` and reword tagline ([#18](https://github.com/everruns/mira/pull/18)) by [@chaliy](https://github.com/chaliy)
- feat(protocol): machine-readable JSON Schema generated from wire types ([#16](https://github.com/everruns/mira/pull/16)) by [@chaliy](https://github.com/chaliy)
- feat(cli): --save run archive with run ids, timestamps, and mira.toml ([#15](https://github.com/everruns/mira/pull/15)) by [@chaliy](https://github.com/chaliy)
- feat: split subject execution from scoring (execute/score, rescore) ([#11](https://github.com/everruns/mira/pull/11)) by [@chaliy](https://github.com/chaliy)
- feat(metrics): extensible numeric metrics map + generic budget scorers ([#10](https://github.com/everruns/mira/pull/10)) by [@chaliy](https://github.com/chaliy)
- feat: surface infrastructure errors as N/A (not failures), retryable ([#8](https://github.com/everruns/mira/pull/8)) by [@chaliy](https://github.com/chaliy)
- feat(scorer): N/A score state + provider-backed LLM judge ([#6](https://github.com/everruns/mira/pull/6)) by [@chaliy](https://github.com/chaliy)
- feat(exec): bounded, provider-aware, adaptive matrix concurrency ([#4](https://github.com/everruns/mira/pull/4)) by [@chaliy](https://github.com/chaliy)
- feat: live progress bar and session-backed checkpoints for `mira run` ([#3](https://github.com/everruns/mira/pull/3)) by [@chaliy](https://github.com/chaliy)
- Productionize the Mira eval-framework PoC into a published workspace ([#2](https://github.com/everruns/mira/pull/2)) by [@chaliy](https://github.com/chaliy)
- chore(protocol): reset protocol version to the 1.0 baseline ([#33](https://github.com/everruns/mira/pull/33)) by [@chaliy](https://github.com/chaliy)
- chore(just): add install recipe ([#17](https://github.com/everruns/mira/pull/17)) by [@chaliy](https://github.com/chaliy)
- chore(ship): resolve addressed PR review comments ([#13](https://github.com/everruns/mira/pull/13)) by [@chaliy](https://github.com/chaliy)
- chore(skills): adopt ship skill and split public/internal skill layout ([#9](https://github.com/everruns/mira/pull/9)) by [@chaliy](https://github.com/chaliy)
- docs: finish Target/expected rename in docs and examples (follow-up to #34) ([#35](https://github.com/everruns/mira/pull/35)) by [@chaliy](https://github.com/chaliy)
- docs: add docs index + public-docs spec, reconcile drift ([#19](https://github.com/everruns/mira/pull/19)) by [@chaliy](https://github.com/chaliy)
- docs(contributing): document main branch-protection gate ([#14](https://github.com/everruns/mira/pull/14)) by [@chaliy](https://github.com/chaliy)
- docs(readme): reframe as evals toolkit with overview diagram ([#12](https://github.com/everruns/mira/pull/12)) by [@chaliy](https://github.com/chaliy)
- docs: surface agentic-trajectory eval as a headline strength ([#7](https://github.com/everruns/mira/pull/7)) by [@chaliy](https://github.com/chaliy)
- docs: extensibility guide + custom-subject example ([#5](https://github.com/everruns/mira/pull/5)) by [@chaliy](https://github.com/chaliy)
