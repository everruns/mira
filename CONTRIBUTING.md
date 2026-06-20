# Contributing to Mira

Thanks for helping! Mira is part of the [Everruns](https://github.com/everruns)
ecosystem.

## Development

```bash
git clone https://github.com/everruns/mira
cd mira
just check        # fmt + clippy -D warnings + tests
just run-examples # drive the bundled example servers through the CLI
```

The core crates (`mira-eval`, `mira-cli`) build in seconds. `mira-everruns`
pulls the everruns runtime and is slow on first build.

## Ground rules

- **Tests with behaviour.** New scorers, subjects, or protocol changes ship with
  tests. Fix bugs by first writing a failing test.
- **Keep the core light.** `mira-eval` is provider-agnostic and has no heavy
  dependencies. Provider/runtime integrations are separate crates.
- **Docs in sync.** User-facing changes update `docs/`; wire-format changes
  update `docs/protocol.md`; design changes update `specs/`.
- **Conventional commits.** e.g. `feat(scorer): add cost_within`,
  `fix(cli): correct checkpoint key`.

## Pull requests

1. Branch off the latest `main`.
2. `just check` is green.
3. Update `CHANGELOG.md` under `## [Unreleased]`.
4. Open the PR with a clear description of the change and its motivation.

See [AGENTS.md](AGENTS.md) for the full checklist and architecture notes, and
[specs/release-process.md](specs/release-process.md) for how releases ship.

## License

By contributing you agree your contributions are licensed under the MIT License.
