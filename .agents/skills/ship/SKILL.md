---
name: ship
description: Run the full ship flow — verify quality, ensure test coverage, update artifacts, smoke test, push, create PR, and merge when CI is green. Trigger when user says "ship", "ship it", "fix and ship", or asks to push and merge a branch.
user_invocable: true
metadata:
  internal: true
---

Run the full ship flow: verify quality, ensure test coverage, update artifacts, smoke test, then push, create PR, and merge when CI is green.

This skill implements the Pre-PR checklist from AGENTS.md. When the user says "ship" or "fix and ship", execute ALL phases below — not just the push/merge steps.

## Arguments

- `$ARGUMENTS` - Optional: description of what is being shipped (used for PR title/body context and to scope the quality checks)

## Instructions

### Phase 1: Pre-flight

1. Confirm we're NOT on `main` or `master`
2. If `HEAD` is detached and the current task has local changes ready to ship, create a branch first instead of stopping
3. Confirm whether uncommitted changes belong to the task being shipped
4. If the worktree is dirty only because of the current task, keep going: validate, commit, and ship those changes
5. If unrelated uncommitted changes exist, stop and tell the user

### Phase 2: Test Coverage

First refresh your view of `main` so the comparison is accurate:

```bash
git fetch origin main
```

Then review the changes on this branch (use `git diff origin/main...HEAD` and `git log origin/main..HEAD`) and ensure comprehensive test coverage:

1. **Identify all changed code paths** — every new/modified type, trait, scorer, subject, macro, CLI command
2. **Verify existing tests cover the changes** — run `just test` and check for failures
3. **Write missing tests** for any uncovered code paths:
   - **Positive tests**: happy path, valid inputs, expected scoring/report output
   - **Negative tests**: invalid inputs, error conditions, boundary cases, missing resources
   - **Protocol/host tests**: if the change touches the protocol, runner, study, or host wiring, add tests that exercise the end-to-end path
4. **Write a failing test before fixing a bug** (per AGENTS.md), then make it pass
5. **Run all tests** to confirm green: `just test`. The core crates run fast via `just test-core`
6. If any test fails, fix the code or test until green

### Phase 3: Artifact Updates

Review the changes and update project artifacts where applicable. Skip items that aren't affected.

1. **Specs** (`specs/`): if the change alters a design decision covered by `architecture.md` or `release-process.md`, update the relevant spec to stay in sync — or propose a spec change before the code
2. **Public docs** (`docs/`): if the change affects user-facing behaviour or the protocol, update the relevant guide and `docs/protocol.md`
3. **API docs** (rustdoc): keep rustdoc on changed public items accurate; CI builds docs with `-D warnings`
4. **CHANGELOG.md**: add an entry for any public-facing change
5. **AGENTS.md**: if the change adds new recipes, specs, or modifies the development workflow — update the relevant section

### Phase 3b: Code Simplification

Review all changed code for opportunities to simplify:

1. **Identify duplication** — look for repeated patterns that could share a helper or be consolidated
2. **Reduce complexity** — simplify nested logic, long match arms, deeply indented blocks
3. **Remove dead code** — unused functions, unreachable branches, commented-out code
4. **Check naming** — ensure functions, variables, and types have clear, descriptive names
5. **Verify no over-engineering** — remove unnecessary abstractions, feature flags, or indirection that don't serve the current change. Keep heavy provider deps out of `mira-eval` (see AGENTS.md)

If simplification changes are made, loop back to Phase 2 to verify tests still pass.

### Phase 3c: Security Review

Analyze all changed code for security and robustness issues:

1. **Input validation** — check that external data (study config, subject responses, CLI arguments, file paths) is validated before use
2. **Injection risks** — for `CliSubject` and any process/command execution, look for command injection, path traversal, or shell metacharacter issues
3. **Error handling** — ensure errors don't leak internal state or sensitive information from model providers
4. **Resource limits** — check for unbounded loops, unbounded allocations, or missing limits on user-controlled sizes (large studies, many models)
5. **Unsafe code** — review any `unsafe` blocks for soundness; prefer safe alternatives

If security issues are found, fix them and add regression tests.

### Phase 3d: Design Quality Review

Review all changed code for shortcuts, lazy abstractions, and premature compromises. This is a pre-1.0 internal project — correctness and clean design matter more than backward compatibility (none needed pre-1.0). Take the time to find better abstractions.

1. **No shortcut abstractions** — reject copy-paste patterns disguised as "good enough". If two things are *actually* the same concept, unify properly. If not, keep them separate with clear names — don't force a bad shared interface.
2. **No lazy wrappers** — every abstraction must earn its place. A wrapper that just forwards calls adds indirection without value. If a layer doesn't add meaning, remove it.
3. **Right abstraction level** — check that traits, types, and module boundaries model the actual domain, not implementation accidents. Keep the core provider-agnostic: `ModelSpec` carries labels, not SDK types.
4. **No stringly-typed interfaces** — look for magic strings, string matching on variant names, ad-hoc parsing of structured data. Replace with enums, newtypes, or proper typed APIs.
5. **No premature generics** — generalize only when there are (or will immediately be) multiple real callers.
6. **No compatibility shims** — pre-1.0 internal code. If an interface is wrong, change it and fix the call sites.
7. **Error types are first-class** — error enums should be specific and actionable, not catch-all `Other(String)` buckets.
8. **Module boundaries enforce invariants** — tighten visibility where a `pub` field or function lets outside code break a module's assumptions.

If design issues are found, refactor, update tests (loop back to Phase 2), and update specs if the change alters documented behavior.

### Phase 4: Smoke Testing

Smoke test impacted functionality to verify it works end-to-end:

1. **Examples**: run `just run-examples` to drive the bundled example servers via the CLI and verify they still work
2. **CLI changes**: run the `mira` host binary with relevant commands and verify output
3. **Library changes**: if a public API changed, confirm the examples that use it still compile and run

If smoke testing reveals issues, fix them and loop back to Phase 2 (tests must still pass).

### Phase 5: Quality Gates

```bash
git fetch origin main && git rebase origin/main
```

- If rebase fails with conflicts, abort and tell the user to resolve manually

```bash
just pre-pr
```

- This runs `just check` (fmt `--check` + clippy `-D warnings` + test) and the publish dry-run
- If fmt fails, run `just fmt` to auto-fix, then retry once
- If still failing, stop and report

### Phase 6: Push and PR

```bash
git push -u origin <current-branch>
```

Check for existing PR:

```bash
gh pr view --json url 2>/dev/null
```

If no PR exists, create one:

- **Title**: Conventional Commit style from the branch commits (`feat`, `fix`, `docs`, `refactor`, `test`, `chore`)
- **Body**: summary of What, Why, How, and what tests were added/verified
- Use `gh pr create`

If a PR already exists, update it if needed and report its URL.

**Resolve addressed review comments**: if this ship is updating an existing PR
that has review comments, check each unresolved review thread. For every comment
whose feedback the pushed changes now address, resolve that thread (mark it as
resolved). Leave threads open only when the feedback is still outstanding or
needs the reviewer's input — when in doubt, reply explaining the status instead
of resolving silently.

### Phase 7: Wait for CI and Merge

- Check CI status with `gh pr checks` (poll every 30s, up to 15 minutes)
- If CI is green, merge with `gh pr merge --squash --auto`
- If CI fails, report the failing checks and stop
- **NEVER** merge when CI is red

### Phase 8: Post-merge

After successful merge:

- Report the merged PR URL
- Done

## Rules

- Phases 2-4 (tests, artifacts, simplification, security review, smoke testing) are the quality core — do NOT skip them.
- The `$ARGUMENTS` context helps scope which tests, specs, and smoke tests are relevant.
- For "fix and ship" requests: implement the fix first, then run `/ship` to validate and merge.
- **Never close a half-done issue.** If the PR only covers a subset of the issue's tasks/checkboxes, use `Part of #N` instead of `Closes #N` or `Fixes #N`. Only use closing keywords when every task in the issue is complete.
- **Never add Claude/session/AI attribution** in commits, PRs, docs, or code comments (per AGENTS.md).
