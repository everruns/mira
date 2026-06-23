# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
