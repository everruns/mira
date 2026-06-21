## Coding-agent guidance

### Style

Telegraph. Drop filler/grammar. Min tokens.

### Critical thinking

Fix root cause. Unsure: read more code; if stuck, ask with short options.
Unrecognized changes: assume another agent; keep going. If it causes issues,
stop and ask.

### Principles

- Always work on top of the latest `main` from remote. In worktrees: fetch
  `origin/main`, then rebase before editing.
- Important decisions as comments on top of the relevant file/function.
- Code testable, smoke-testable, runnable locally.
- Small, incremental, PR-sized changes.
- No backward-compat needed pre-1.0 (internal code).
- Write a failing test before fixing a bug.
- Everything runnable and tested — no theoretical code. Don't stop until e2e
  works; verify before declaring done.

### Specs

`specs/` holds the design of record. New code complies with these or proposes a
change there.

| Spec | Description |
|------|-------------|
| architecture | Core model, crate architecture, execution model, metrics, migration |
| release-process | Versioning, crates.io publishing flow |

### Documentation

- **Public docs** live in `docs/` — user-facing guides and the protocol
  reference (`docs/protocol.md`). Keep them in sync with behaviour.
- **API docs** are rustdoc on the crates; `cargo doc --no-deps --open` to
  preview. CI builds docs with `-D warnings`.

### Architecture at a glance

```
crates/mira-eval     core library (lib name `mira`): types, traits, scorers,
                     subjects (subject_fn, CliSubject), protocol, study, host,
                     runner, report.  NO heavy deps.
crates/mira-cli      the `mira` host binary.
crates/mira-macros   the `#[eval]` proc-macro (re-exported as `mira::eval`).
crates/mira-everruns RuntimeSubject over the published everruns-runtime.
examples/            runnable, offline example studies (one folder each).
```

The core is **provider-agnostic**: `ModelSpec` carries `(provider, model)`
labels and no SDK types. Keep everruns (and any future provider SDK) out of
`mira-eval` — integrations are separate crates.

### Local dev

```bash
just --list     # all recipes
just build      # cargo build
just test       # cargo test (workspace)
just check      # fmt --check + clippy -D warnings + test
just pre-pr     # check + publish dry-run
just run-examples   # drive the bundled example servers via the CLI
```

`mira-everruns` pulls a large dependency tree (the everruns runtime); the first
build is slow. The core crates build in seconds.

### Cloud agent setup

`DOPPLER_TOKEN` is pre-configured. Fetch secrets via the API:

```bash
curl -s "https://api.doppler.com/v3/configs/config/secret?name=GITHUB_TOKEN" \
  -u "$DOPPLER_TOKEN:" | python3 -c "import sys,json; print(json.load(sys.stdin)['value']['raw'])"
```

`CARGO_REGISTRY_TOKEN` (crates.io) and `DOPPLER_TOKEN` are configured as GitHub
Actions secrets for publishing. The Homebrew tap push uses
`HOMEBREW_TAP_GITHUB_TOKEN` (fetched from Doppler in CI).

### Git and commits

- Conventional Commits: `type(scope): description`. Types: `feat`, `fix`,
  `docs`, `refactor`, `test`, `chore`. Use `chore` for `specs/`, `AGENTS.md`, or
  CI metadata.
- **Never add Claude/session/AI attribution** in commits, PRs, docs, or code
  comments (no `Co-Authored-By: Claude`, no "Generated with Claude Code").
- Commit attribution must be a real human user. `.claude/hooks/fix-git-identity.sh`
  (a SessionStart hook) sets it; if git identity is missing or agent-like, stop
  and ask before committing.
- Stage files explicitly by name. Avoid broad `git add .` / `git add -A`.

### Pre-PR checklist

- `just check` passes (fmt, clippy `-D warnings`, tests).
- New behaviour has tests; examples still run (`just run-examples`).
- Public-facing changes update `docs/` and `CHANGELOG.md`.
- Specs updated if a design decision changed.
