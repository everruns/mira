# mira-cli

The `mira` host CLI for [Mira](https://github.com/everruns/mira), a Rust-first,
code-first evaluation framework. It compiles + spawns an eval **study** (a
program built on [`mira-eval`](https://crates.io/crates/mira-eval)), plans the
run across the model matrix, executes each case over the protocol, and reports.

```bash
cargo install mira-cli      # installs the `mira` binary

mira --bin greet list
mira --bin greet run                 # whole matrix (sim runs; keyed cases skip)
mira --bin greet run greet           # selective (substring), like cargo test
mira --bin greet run --tag smoke
mira --bin greet run --targets sim --format junit --out results.xml
mira --bin greet run                        # saves a run folder by default
mira --bin greet run --resume <run_id>      # resume; run only the missing cases
mira report <run_id>                        # re-render a saved run's reports
```

Point it at any study: `--bin NAME` (a Rust eval crate), `--cmd "..."` (e.g. a
Python study), `--example NAME`, or another package via `--package` /
`--manifest-path`.

See the [docs](https://github.com/everruns/mira/tree/main/docs). Licensed under MIT.
