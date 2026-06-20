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
| `mira-macros` | crates.io | proc-macro (re-exported by `mira-eval`) |
| `mira-eval` | crates.io | library `mira` |
| `mira-cli` | crates.io | binary `mira` |
| `mira-everruns` | crates.io | library |

Publish order matters: `mira-macros` first, then `mira-eval` (re-exports it),
then `mira-cli` and `mira-everruns` (both depend on `mira-eval`). The
`mira-examples` crate is `publish = false`.

### Homebrew

The `mira` CLI is distributed via Homebrew as the default install method
(`brew install everruns/tap/mira`), using the org-wide central tap
`everruns/homebrew-tap` (same as yolop). There is **no formula checked into this
repo**. On release, `release.yml` dispatches `cli-binaries.yml`, which:

1. builds the `mira` binary for `aarch64-apple-darwin`, `x86_64-apple-darwin`,
   and `x86_64-unknown-linux-gnu`;
2. uploads the tarballs + `.sha256` checksums to the GitHub Release;
3. generates a multi-platform formula (`on_macos`/`on_linux`, version scanned
   from the URL) and pushes it to `everruns/homebrew-tap` as `Formula/mira.rb`.

The tap push authenticates with `HOMEBREW_TAP_GITHUB_TOKEN` — a fine-grained PAT
scoped to `everruns/homebrew-tap` only — fetched from Doppler via the Doppler CLI
action.

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
   and refresh `Cargo.lock` (`cargo update -p mira-macros -p mira-eval -p
   mira-cli -p mira-everruns`). Path-dep pins reference the same version.
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
- On the release tag, `publish.yml` publishes `mira-macros` then `mira-eval`,
  then `mira-cli` and `mira-everruns`, waiting for the crates.io index between
  dependent publishes, and verifies the published versions.
- On the published release, `homebrew.yml` bumps `Formula/mira.rb` and mirrors it
  to the Homebrew tap.

## Pre-release checklist

CI green on `main`; `CHANGELOG.md` has entries since the last release; the
version is consistent across the workspace and `Cargo.lock`; all three
`cargo publish --dry-run`s succeed; the new version > latest on crates.io.
