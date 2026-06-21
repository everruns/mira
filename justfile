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

# Build the API docs with warnings denied (as CI does).
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# === Examples ===

# Drive each bundled example study through the host CLI (offline, sim only).
run-examples:
    cargo run -q -p mira-cli -- --bin greet run
    cargo run -q -p mira-cli -- --bin coding run
    cargo run -q -p mira-cli -- --bin cli_subject run
    cargo run -q -p mira-cli -- --bin metrics run
    cargo run -q -p mira-cli -- --bin matrix run
    cargo run -q -p mira-cli -- --bin swe_bench run
    cargo run -q -p mira-cli -- --bin llmsim run
    cargo run -q -p mira-cli -- --bin multimodal run
    cargo run -q -p mira-cli -- --cmd "python3 examples/greet-python/study.py" run

# === Release ===

# Verify every publishable crate can be packaged (files, version drift).
publish-dry-run:
    cargo publish --dry-run -p mira-macros
    cargo publish --dry-run -p mira-eval
    cargo publish --dry-run -p mira-cli
    cargo publish --dry-run -p mira-everruns

# All pre-PR checks plus the publish dry-run.
pre-pr: check publish-dry-run
    @echo "Pre-PR checks passed"
