# Mira development commands.
# Install just: cargo install just  (or `cargo binstall just`)
# Usage: just <recipe>   (or: just --list)

# Default: show available recipes.
default:
    @just --list

# === Build & test ===

# Build the whole workspace.
build:
    cargo build --workspace

# Build only the light core crates (skips the heavy everruns adapter).
build-core:
    cargo build -p mira-eval -p mira-cli

# Build and install the local mira CLI binary.
install:
    cargo install --path crates/mira-cli --bin mira --locked --force

# Run all tests.
test:
    cargo test --workspace

# Run only the core crate tests (fast).
test-core:
    cargo test -p mira-eval -p mira-cli

# === Lint & format ===

# Auto-fix formatting and clippy lints.
fmt:
    cargo fmt --all
    cargo clippy --all-targets --fix --allow-dirty --allow-staged 2>/dev/null || true

# Format-check, clippy (deny warnings), and test — the CI gate.
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --workspace
    # Staged (unstable) protocol additions must still compile and round-trip.
    cargo test -p mira-eval --features protocol-unstable
    # The committed schema must match the protocol types.
    cargo run -q -p mira-schema-gen -- --check

# Regenerate the committed JSON Schema artifacts under schema/ from the
# protocol types. Run after changing crates/mira-eval/src/protocol.rs.
schema:
    cargo run -q -p mira-schema-gen

# Python SDK: wire types in sync with the schema + the test suite.
test-py:
    python3 sdks/python/codegen.py --check
    cd sdks/python && python3 -m pytest -q

# TypeScript SDK: wire types in sync with the schema, build, + the test suite.
test-ts:
    cd sdks/typescript && npm ci && npm test

# Build the TypeScript SDK (its dist/), so the greet-typescript example can run.
build-ts-sdk:
    cd sdks/typescript && npm ci && npm run build

# Build the API docs with warnings denied (as CI does).
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# === Examples ===

# Drive each bundled example study through the host CLI (offline, sim only).
# The TypeScript polyglot example needs the SDK built first (build-ts-sdk).
run-examples: build-ts-sdk
    # Single-file studies (examples/<name>.rs) via the cargo-script shim.
    cargo run -q -p mira-cli -- run --study examples/greet.rs
    cargo run -q -p mira-cli -- run --study examples/coding.rs
    cargo run -q -p mira-cli -- run --study examples/swe_bench.rs
    cargo run -q -p mira-cli -- run --study examples/multimodal.rs
    cargo run -q -p mira-cli -- run --study examples/interactive.rs
    # Crate examples (multi-file / heavy deps) via --study-bin.
    cargo run -q -p mira-cli -- run --study-bin cli_subject
    cargo run -q -p mira-cli -- run --study-bin metrics
    cargo run -q -p mira-cli -- run --study-bin matrix
    cargo run -q -p mira-cli -- run --study-bin llmsim
    # Polyglot studies via the SDKs.
    cargo run -q -p mira-cli -- run --study-python examples/greet-python/study.py
    cargo run -q -p mira-cli -- run --study-cmd "node examples/greet-typescript/study.mjs"

# === Release ===

# Verify every publishable crate can be packaged (files, version drift).
#
# One --workspace invocation, not six per-crate ones. Per-crate dry-runs cannot
# work at a version bump: each crate's generated manifest drops the `path` of
# its internal deps, so `cargo publish -p mira-eval` resolves
# `mira-macros = "^X.Y.Z"` against the crates.io index, where the new version
# does not exist yet — it fails while *packaging*, before any build, so
# --no-verify does not help either. With --workspace, cargo resolves the
# sibling crates being published together against the workspace, so all six
# package *and* fully verify. Publish order is cargo's job here and
# .github/workflows/publish.yml's job for the real, index-waiting publish.
publish-dry-run:
    cargo publish --dry-run --workspace

# Pre-PR gate: fmt, clippy, tests. The publish dry-run is a release-time
# concern (it guards packaging, which only matters when cutting a release), so
# it's kept out of the per-PR path — run `just publish-dry-run` before a
# release instead.
pre-pr: check
    @echo "Pre-PR checks passed"
