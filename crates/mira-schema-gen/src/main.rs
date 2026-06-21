//! Writes (or, with `--check`, verifies) the committed protocol schema.
//!
//! * `cargo run -p mira-schema-gen` — regenerate `schema/v<major>/`.
//! * `cargo run -p mira-schema-gen -- --check` — fail (exit 1) if the committed
//!   files are stale. CI and `just check` run this so a protocol change can't
//!   merge without a regenerated schema.

use std::process::ExitCode;

use mira_schema_gen::{artifacts, schema_dir};

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let dir = schema_dir();
    let files = artifacts();

    if check {
        let stale: Vec<String> = files
            .iter()
            .filter(|(name, want)| {
                std::fs::read_to_string(dir.join(name)).unwrap_or_default() != *want
            })
            .map(|(name, _)| dir.join(name).display().to_string())
            .collect();
        if !stale.is_empty() {
            eprintln!(
                "schema artifacts are stale: {}\nrun `just schema` (or `cargo run -p \
                 mira-schema-gen`) and commit the result.",
                stale.join(", "),
            );
            return ExitCode::FAILURE;
        }
        eprintln!("schema artifacts up to date");
        return ExitCode::SUCCESS;
    }

    std::fs::create_dir_all(&dir).expect("create schema dir");
    for (name, body) in &files {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write schema artifact");
        eprintln!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
