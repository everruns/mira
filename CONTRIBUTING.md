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
  update `docs/protocol.md`; design changes update `specs/`. Doc conventions
  (structure, the SVG-diagram rule) live in [`specs/docs.md`](specs/docs.md).
- **Conventional commits.** e.g. `feat(scorer): add cost_within`,
  `fix(cli): correct case key encoding`.

## Pull requests

1. Branch off the latest `main`.
2. `just check` is green.
3. Update `CHANGELOG.md` under `## [Unreleased]`.
4. Open the PR with a clear description of the change and its motivation.

See [AGENTS.md](AGENTS.md) for the full checklist and architecture notes, and
[specs/release-process.md](specs/release-process.md) for how releases ship.

## Branch protection (the merge gate)

`main` must stay green and merge-gated. CI (`.github/workflows/ci.yml`) runs
`lint`, `audit`, `test`, and `examples`, and rolls them up into a single
`Check` job (`needs: [lint, audit, test, examples]`, `if: always()`) that fails
unless every job succeeds. That one job is the status check to require.

Configure once, in **Settings → Branches → Branch protection rules** for
`main` (needs admin):

- **Require a pull request before merging** — no direct pushes to `main`.
- **Require status checks to pass before merging**, then search for and select
  the `Check` status check. GitHub lists Actions checks as
  `<workflow> / <job>`, so it appears as **`CI / Check`** (the `Check` job in
  the `CI` workflow) — pick that entry, not a bare `Check`. Keep "Require
  branches to be up to date before merging" on so the gate runs against the
  post-merge tree.
- **Do not allow bypassing the above settings** (applies the rule to admins).

The job is named `Check` deliberately — keep that name stable so the required
check never silently detaches. Without this rule the `Check` job is advisory
only: a red run can still be merged.

## License

By contributing you agree your contributions are licensed under the MIT License.
