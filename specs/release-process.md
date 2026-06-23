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

| Artifact | Registry | Installs as |
|----------|----------|-------------|
| `mira-macros` | crates.io | proc-macro (re-exported by `mira-eval`) |
| `mira-eval` | crates.io | library `mira` |
| `mira-cli` | crates.io | binary `mira` |
| `mira-everruns` | crates.io | library |
| `mira-judge` | crates.io | library |
| `mira-eval` (Python SDK, `sdks/python`) | PyPI | `pip install mira-eval` |
| `mira-eval` (TypeScript SDK, `sdks/typescript`) | npm | `npm install mira-eval` |

Publish order matters: `mira-macros` first, then `mira-eval` (re-exports it),
then `mira-cli`, `mira-everruns`, and `mira-judge` (all depend on `mira-eval`).
The `mira-examples` crate is `publish = false`. The Python and TypeScript SDKs are
native libraries (not bindings), so they publish independently of the crate order;
each shares the workspace version and CI verifies the match. `mira-eval` is the
same string across crates.io, PyPI, and npm — by design, all three are "the Mira
eval library" for their language.

### Python SDK (PyPI)

The Python SDK publishes to PyPI as `mira-eval` via **OIDC Trusted Publishing**
(no API token). The trusted publisher must be registered once on PyPI: project
`mira-eval`, owner `everruns`, repository `mira`, workflow `publish.yml`,
environment `release`. The `publish-python` job in `publish.yml` builds the
sdist + wheel from `sdks/python/` and uploads them on the release tag. Crate name
`mira-eval` is the same string as the PyPI project — by design, both are "the
Mira eval library" for their language.

### TypeScript SDK (npm)

The TypeScript SDK publishes to npm as `mira-eval` via **OIDC trusted publishing**
(no `NODE_AUTH_TOKEN`), mirroring the Python flow. The trusted publisher must be
registered once on npmjs.com: package `mira-eval`, owner `everruns`, repository
`mira`, workflow `publish.yml`, environment `release`. The `publish-typescript`
job in `publish.yml` verifies `sdks/typescript/package.json`'s version matches the
workspace, builds `dist/` (`npm ci` + the package's `build`), and runs
`npm publish --provenance` on the release tag. It upgrades to a trusted-publishing-
capable npm (`npm install -g npm@latest`) and guards on `npm view mira-eval@<v>` so
a re-dispatch to finish a partial release no-ops on an already-published version
(the npm dual of PyPI `skip-existing`). Independent of the crates and the other
SDK, so a hiccup in one never blocks another.

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
   mira-cli -p mira-everruns -p mira-judge`). Path-dep pins reference the same
   version. Bump `sdks/python/pyproject.toml` to match (CI fails the SDK publish
   if it drifts).
4. **Local verification** — `just check` (`cargo fmt --check`, `cargo clippy
   --all-targets -- -D warnings`, `cargo test`) and `just test-py` (SDK codegen
   drift + pytest).
5. **Verify publish-readiness** — `just publish-dry-run` (`cargo publish
   --dry-run` for `mira-macros`, `mira-eval`, `mira-cli`, `mira-everruns`,
   `mira-judge`). This catches packaging problems local builds don't (missing
   `readme`, files outside the crate dir, version drift). Confirm the new version
   is greater than the latest on crates.io for each crate. Fix root cause and
   re-run before opening the PR. **First-release caveat:** dependents of an
   unpublished crate (everything depending on `mira-eval`) can't fully dry-run
   until the base is on crates.io — verify their file lists with `cargo package
   --list -p <crate>` instead; CI publishes in dependency order with index waits.
   Also confirm the Python SDK builds (`python -m build sdks/python`).
6. **Commit and push** — `chore(release): prepare vX.Y.Z` on a feature branch.
7. **Create PR** — same title, changelog excerpt + publish-readiness report.
8. **Monitor post-merge** — watch `release.yml` create the Release + tag, then
   `publish.yml` publish each crate (each step verifies the published version) and
   the Python SDK. Declare "shipped" only when crates.io reports the new version
   for all five crates and PyPI shows the SDK. `publish.yml` is **idempotent**
   (each crate step skips a version already on crates.io via
   `scripts/cargo_publish_if_needed.sh`; the PyPI step uses `skip-existing`), so a
   partial release caused by a transient crates.io blip is recovered by
   re-dispatching it — `workflow_dispatch` from `main` fills only the missing
   artifacts.

## CI automation

- On merge to `main`, `release.yml` detects the `chore(release): prepare vX.Y.Z`
  commit, extracts notes from `CHANGELOG.md`, creates the GitHub Release + tag,
  and dispatches `publish.yml`.
- On the release tag, `publish.yml` publishes `mira-macros` then `mira-eval`,
  then `mira-cli`, `mira-everruns`, and `mira-judge`, waiting for the crates.io
  index between dependent publishes, and verifies the published versions. A
  parallel `publish-python` job builds and publishes the Python SDK to PyPI via
  OIDC Trusted Publishing.
- `release.yml` also dispatches `cli-binaries.yml`, which builds the prebuilt
  `mira` binaries, attaches them to the Release, and mirrors the generated
  `Formula/mira.rb` to the Homebrew tap.

## Pre-release checklist

CI green on `main`; `CHANGELOG.md` has entries since the last release; the version
is consistent across the workspace, `Cargo.lock`, and the Python SDK
`pyproject.toml`; all five `cargo publish --dry-run`s succeed (or, on a first
release, dependents' `cargo package --list` is clean); the Python SDK builds; the
new version > latest on crates.io / PyPI.
