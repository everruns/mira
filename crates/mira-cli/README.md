# mira-cli

The `mira` host CLI for [Mira](https://github.com/everruns/mira), a Rust-first,
code-first evaluation framework. It compiles + spawns an eval **server** (a
program built on [`mira-eval`](https://crates.io/crates/mira-eval)), plans the
run across the model matrix, executes each cell over the protocol, and reports.

```bash
cargo install mira-cli      # installs the `mira` binary

mira --example greet list
mira --example greet run                 # whole matrix (sim runs; keyed cells skip)
mira --example greet run greet           # selective (substring), like cargo test
mira --example greet run --tag smoke
mira --example greet run --models sim --format junit --out results.xml
mira --example greet run --checkpoint ck.json   # resumable
```

Point it at any server: `--bin NAME`, `--example NAME`, `--cmd "..."`, or another
package via `--package` / `--manifest-path`.

See the [docs](https://github.com/everruns/mira/tree/main/docs). Licensed under MIT.
