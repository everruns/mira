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
    cargo run -q -p mira-cli -- --script examples/greet.rs run
    cargo run -q -p mira-cli -- --script examples/coding.rs run
    cargo run -q -p mira-cli -- --script examples/swe_bench.rs run
    cargo run -q -p mira-cli -- --script examples/multimodal.rs run
    cargo run -q -p mira-cli -- --script examples/interactive.rs run
    # Crate examples (multi-file / heavy deps) via --bin.
    cargo run -q -p mira-cli -- --bin cli_subject run
    cargo run -q -p mira-cli -- --bin metrics run
    cargo run -q -p mira-cli -- --bin matrix run
    cargo run -q -p mira-cli -- --bin llmsim run
    # Polyglot studies via the SDKs.
    cargo run -q -p mira-cli -- --python3 examples/greet-python/study.py run
    cargo run -q -p mira-cli -- --cmd "node examples/greet-typescript/study.mjs" run

# === Release ===

# Verify every publishable crate can be packaged (files, version drift).
# Leaf crates (no internal deps) get a full verify — that's where packaging
# bugs (missing files, metadata drift) actually bite. The dependents are
# packaged with --no-verify: their verification build would resolve mira-eval
# from crates.io (path is stripped on publish), compiling against the stale
# published version instead of the workspace, so it fails whenever they use
# unpublished mira-eval APIs. The real, order-aware publish (mira-eval first,
# then dependents) lives in .github/workflows/publish.yml; here we only check
# that the dependents package cleanly.
publish-dry-run:
    cargo publish --dry-run -p mira-macros
    cargo publish --dry-run -p mira-eval
    cargo publish --dry-run -p mira-cli --no-verify
    cargo publish --dry-run -p mira-everruns --no-verify
    cargo publish --dry-run -p mira-judge --no-verify

# Pre-PR gate: fmt, clippy, tests. The publish dry-run is a release-time
# concern (it guards packaging, which only matters when cutting a release), so
# it's kept out of the per-PR path — run `just publish-dry-run` before a
# release instead.
pre-pr: check
    @echo "Pre-PR checks passed"
