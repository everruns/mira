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

# Build the API docs with warnings denied (as CI does).
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# === Examples ===

# Drive each bundled example server through the host CLI (offline, sim only).
run-examples:
    cargo run -q -p mira-cli -- --package mira-examples --example greet run
    cargo run -q -p mira-cli -- --package mira-examples --example coding run
    cargo run -q -p mira-cli -- --package mira-examples --example cli_subject run
    cargo run -q -p mira-cli -- --package mira-examples --example metrics run
    cargo run -q -p mira-cli -- --package mira-examples --example matrix run
    cargo run -q -p mira-cli -- --package mira-examples --example swe_bench run
    cargo run -q -p mira-cli -- --package mira-examples --example llmsim run

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
