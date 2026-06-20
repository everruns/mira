# Release Process

## Status

Implemented.

## Abstract

Releases are initiated by asking a coding agent to prepare the release; CI
automation handles tagging and publishing. Flow: **prepare → verify-can-publish
→ merge → monitor-published**.

## Versioning

[Semantic Versioning](https://semver.org/): MAJOR = breaking API changes, MINOR =
new features, PATCH = bug fixes/docs. All workspace crates share one version
(`workspace.package.version`).

## Published artifacts

| Crate | Registry | Installs as |
|-------|----------|-------------|
| `mira-eval` | crates.io | library `mira` |
| `mira-cli` | crates.io | binary `mira` |
| `mira-everruns` | crates.io | library |

Publish order matters: `mira-eval` first (others depend on it), then `mira-cli`
and `mira-everruns`.

## Human steps

1. Ask the agent to create a release ("Create release v0.2.0").
2. Review the PR — including the agent's publish-readiness report.
3. Merge to `main` — CI creates the GitHub Release + tag and publishes.
4. Ask the agent to monitor publishing until crates.io shows the new version.

## Agent steps (automated)

0. **Ensure full git history** — cloud sandboxes are often shallow-cloned. Run
   `git fetch --unshallow origin main 2>/dev/null || git fetch origin main` and
   cross-check the GitHub compare API before trusting the changelog.
1. **Determine version** — human-specified, or suggested from changes.
2. **Update `CHANGELOG.md`** (Keep a Changelog format).
3. **Bump the version** in workspace `Cargo.toml` (`workspace.package.version`)
   and refresh `Cargo.lock` (`cargo update -p mira-eval -p mira-cli -p
   mira-everruns`). Path-dep pins reference the same version.
4. **Local verification** — `just check` (`cargo fmt --check`, `cargo clippy
   --all-targets -- -D warnings`, `cargo test`).
5. **Verify publish-readiness** — `cargo publish --dry-run -p mira-eval`, then
   `-p mira-cli` and `-p mira-everruns`. This catches packaging problems local
   builds don't (missing `readme`, files outside the crate dir, version drift).
   Confirm the new version is greater than the latest on crates.io for each
   crate. Fix root cause and re-run before opening the PR.
6. **Commit and push** — `chore(release): prepare vX.Y.Z` on a feature branch.
7. **Create PR** — same title, changelog excerpt + publish-readiness report.
8. **Monitor post-merge** — watch `release.yml` create the Release + tag, then
   `publish.yml` publish each crate (each step verifies the published version).
   Declare "shipped" only when crates.io reports the new version for all three.
   On failure, open a hotfix PR rather than leaving the release half-shipped.

## CI automation

- On merge to `main`, `release.yml` detects the `chore(release): prepare vX.Y.Z`
  commit, extracts notes from `CHANGELOG.md`, creates the GitHub Release + tag,
  and dispatches `publish.yml`.
- On the release tag, `publish.yml` publishes `mira-eval`, then `mira-cli` and
  `mira-everruns`, waiting for the crates.io index between dependent publishes,
  and verifies the published versions.

## Pre-release checklist

CI green on `main`; `CHANGELOG.md` has entries since the last release; the
version is consistent across the workspace and `Cargo.lock`; all three
`cargo publish --dry-run`s succeed; the new version > latest on crates.io.
