# CLAUDE.md

See [AGENTS.md](AGENTS.md) for coding-agent guidance: style, principles,
architecture, local dev commands, and the pre-PR checklist.

Quick reference:

- Build/test: `just build`, `just test`, `just check` (run before every PR).
- Layout: `crates/mira-eval` (core lib `mira`), `crates/mira-cli` (bin `mira`),
  `crates/mira-everruns` (everruns adapter).
- Keep provider SDKs out of `mira-eval` — it is provider-agnostic by design.
- Design of record is in `specs/`; public docs and the protocol reference are in
  `docs/`.
