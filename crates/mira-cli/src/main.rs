//! `mira` — the host CLI. Compiles + spawns an eval **study** (a program that
//! calls `mira::Study::registered().serve()`), enumerates its evals, plans the
//! run (selection × matrix), executes each cell over the protocol, then
//! aggregates, saves, and checkpoints.
//!
//! ```bash
//! mira --bin greet list
//! mira --bin greet run                          # all cells (sim runs; keyed cells skip)
//! mira --bin greet run greet                    # substring filter
//! mira --bin greet run --tag smoke
//! mira --bin greet run --models sim --format junit --out results.xml
//! mira --bin greet run --checkpoint ck.json     # resumable
//! ```
//!
//! Each Rust example is a crate exposing a like-named binary, so `--bin <name>`
//! resolves it across the workspace. Point it at any study: `--bin NAME`,
//! `--example NAME`, an arbitrary `--cmd "..."` (e.g. a Python study), or
//! another package with `--package` / `--manifest-path`.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::str::FromStr;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::process::Command;

use mira::Host;
use mira::protocol::{InitializeResult, ListResult, RunResult};
use mira::report::{self, Format};
use mira::session::{self, Session};

#[derive(Parser)]
#[command(name = "mira", version, about = "Host runner for code-defined evals")]
struct Cli {
    #[command(flatten)]
    target: Target,
    #[command(subcommand)]
    cmd: Cmd,
}

/// How to launch the eval study process.
#[derive(Args)]
struct Target {
    /// Run `cargo run -q --bin <NAME>` (defaults to `greet`).
    #[arg(long, global = true)]
    bin: Option<String>,
    /// Run `cargo run -q --example <NAME>`.
    #[arg(long, global = true)]
    example: Option<String>,
    /// Launch an arbitrary command (split on whitespace).
    #[arg(long, global = true)]
    cmd: Option<String>,
    /// Cargo package to run the bin/example from (`-p`).
    #[arg(long, global = true)]
    package: Option<String>,
    /// Passed through to cargo.
    #[arg(long, global = true)]
    manifest_path: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the evals, samples, scorers, and models the study advertises.
    List,
    /// Run selected cells and report.
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Substring filter on the case key `eval/sample@model`.
    filter: Option<String>,
    /// Only run samples carrying this tag.
    #[arg(long)]
    tag: Option<String>,
    /// Restrict the matrix to these model labels (comma-separated).
    #[arg(long)]
    models: Option<String>,
    /// Write a report file (see --format).
    #[arg(long)]
    out: Option<String>,
    /// Report file format: json | junit | md | html.
    #[arg(long, default_value = "json")]
    format: String,
    /// Persist/resume results here; completed cells are skipped on re-run.
    #[arg(long)]
    checkpoint: Option<String>,
    /// Ignore an existing checkpoint and run everything fresh.
    #[arg(long)]
    fresh: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // One progress bar, shared with the event handler so study `log` lines print
    // cleanly above the bar (via `suspend`) instead of corrupting it. It starts
    // hidden; `run` gives it a length and a draw target once the plan is known.
    let progress = Arc::new(ProgressBar::hidden());
    let progress_evt = progress.clone();
    let mut host = Host::spawn(build_command(&cli.target))
        .await?
        .on_event(move |n| {
            // Per-cell `event` notifications drive the bar's message from the run
            // loop, so they no longer spam stderr. Only study logs are surfaced.
            if n.method == "log"
                && let Some(msg) = n.params["message"].as_str()
            {
                progress_evt.suspend(|| eprintln!("  study: {msg}"));
            }
        });

    let info = host.initialize("mira-cli").await?;
    eprintln!(
        "study {} · protocol {} · {} evals",
        info.study, info.protocol_version, info.evals
    );
    let listing = host.list().await?;

    match cli.cmd {
        Cmd::List => {
            print_listing(&listing);
            host.shutdown().await?;
            Ok(())
        }
        Cmd::Run(args) => run(host, info, listing, args, progress).await,
    }
}

/// Build the study launch command from the target flags.
fn build_command(target: &Target) -> Command {
    if let Some(raw) = &target.cmd {
        let mut parts = raw.split_whitespace();
        let program = parts.next().unwrap_or("false");
        let mut command = Command::new(program);
        command.args(parts);
        return command;
    }

    let mut command = Command::new("cargo");
    command.arg("run").arg("-q");
    if let Some(pkg) = &target.package {
        command.arg("-p").arg(pkg);
    }
    if let Some(bin) = &target.bin {
        command.arg("--bin").arg(bin);
    } else if let Some(example) = &target.example {
        command.arg("--example").arg(example);
    } else {
        // Default to the bundled `greet` example crate's binary.
        command.arg("--bin").arg("greet");
    }
    if let Some(manifest) = &target.manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    command
}

async fn run(
    mut host: Host,
    info: InitializeResult,
    listing: ListResult,
    args: RunArgs,
    progress: Arc<ProgressBar>,
) -> Result<(), Box<dyn std::error::Error>> {
    let format = Format::from_str(&args.format)?;
    let model_filter: Option<Vec<String>> = args
        .models
        .as_ref()
        .map(|m| m.split(',').map(|s| s.trim().to_string()).collect());

    // Plan the full grid, then apply selection. Done up front so the host owns
    // selection/matrix without the study re-running anything.
    let plan = plan_grid(&listing, &args, &model_filter);
    if plan.is_empty() {
        eprintln!("no cells matched the selection");
    }

    // Fingerprint the advertised definitions so a resume can detect stale caches.
    let fingerprints = session::fingerprints(&listing);

    // Resume from a session checkpoint unless --fresh. The session carries the
    // planned `total` and per-eval fingerprints, so we can report accurate
    // progress and warn when a cached cell's eval definition has changed.
    let mut done: BTreeMap<String, RunResult> = BTreeMap::new();
    let mut session = Session::new(
        info.study.clone(),
        info.study_version.clone(),
        plan.len(),
        fingerprints.clone(),
    );
    if let Some(path) = &args.checkpoint
        && !args.fresh
    {
        match Session::load(path) {
            Ok(Some(prev)) => {
                let stale = prev.stale_keys(&fingerprints);
                for r in prev.results {
                    done.insert(r.key(), r);
                }
                session.created_unix = prev.created_unix;
                let resumable = plan.iter().filter(|c| done.contains_key(&c.key())).count();
                eprintln!(
                    "resuming checkpoint: {resumable}/{} cells already done",
                    plan.len()
                );
                if !stale.is_empty() {
                    eprintln!(
                        "warning: {} cached cell(s) are stale — their eval definition changed \
                         since they were recorded. They'll be reused as-is; re-run with --fresh \
                         to recompute. e.g. {}",
                        stale.len(),
                        stale.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
                    );
                }
            }
            // No checkpoint yet — a normal first run.
            Ok(None) => {}
            // The file exists but can't be used; don't silently discard it.
            Err(e) => eprintln!("warning: ignoring checkpoint {path}: {e}; starting fresh"),
        }
    }

    // Configure the progress bar now the plan is known. Drawn only when stderr is
    // a terminal, so it stays hidden when piped or redirected (e.g. under CI) and
    // never pollutes logs; the final report still prints.
    let resumable = plan.iter().filter(|c| done.contains_key(&c.key())).count();
    if !plan.is_empty() && std::io::stderr().is_terminal() {
        progress.set_draw_target(ProgressDrawTarget::stderr());
        progress.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] \
                 {pos}/{len} (eta {eta}) {msg}",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
    }
    progress.set_length(plan.len() as u64);
    progress.set_position(resumable as u64);

    for cell in &plan {
        let key = cell.key();
        if done.contains_key(&key) {
            continue;
        }
        progress.set_message(key.clone());
        let result = host
            .run(&cell.eval, &cell.sample, &cell.model, &cell.params)
            .await?;
        done.insert(key, result);
        progress.inc(1);
        if let Some(path) = &args.checkpoint {
            // Persist only the planned cells, in plan order — so the file stays
            // deterministic and doesn't accumulate cells dropped by a narrower
            // selection on resume.
            let results = plan
                .iter()
                .filter_map(|c| done.get(&c.key()).cloned())
                .collect();
            session.update(plan.len(), fingerprints.clone(), results);
            if let Err(e) = session.save(path) {
                progress.suspend(|| eprintln!("warning: failed to write checkpoint: {e}"));
            }
        }
    }
    progress.finish_and_clear();
    host.shutdown().await?;

    // Report only the planned cells, in plan order.
    let results: Vec<RunResult> = plan
        .iter()
        .filter_map(|cell| done.get(&cell.key()).cloned())
        .collect();

    report::print_results(&results);

    if let Some(path) = &args.out {
        std::fs::write(path, report::render(&results, format))?;
        eprintln!("\nwrote {path} ({:?})", format);
    }

    let failed = results.iter().any(|r| !r.skipped && !r.passed);
    std::process::exit(if failed { 1 } else { 0 });
}

/// One planned matrix cell: an eval/sample/model plus a chosen value per extra
/// axis. Mirrors the in-process runner's cell expansion, but driven entirely
/// from the study's advertised `list` so the host owns the plan.
struct Cell {
    eval: String,
    sample: String,
    model: String,
    params: BTreeMap<String, String>,
}

impl Cell {
    fn key(&self) -> String {
        mira::cell_key(&self.eval, &self.sample, &self.model, &self.params)
    }
}

/// Every combination of axis values for an eval, as `params` maps (cross
/// product), always including at least one empty map.
fn axis_combinations(eval: &mira::protocol::EvalInfo) -> Vec<BTreeMap<String, String>> {
    let mut combos = vec![BTreeMap::new()];
    for axis in &eval.axes {
        let mut next = Vec::new();
        for combo in &combos {
            for value in &axis.values {
                let mut c = combo.clone();
                c.insert(axis.name.clone(), value.clone());
                next.push(c);
            }
        }
        if !next.is_empty() {
            combos = next;
        }
    }
    combos
}

/// Expand the advertised listing into an ordered, selected list of cells.
fn plan_grid(
    listing: &ListResult,
    args: &RunArgs,
    model_filter: &Option<Vec<String>>,
) -> Vec<Cell> {
    let mut plan = Vec::new();
    for eval in &listing.evals {
        let combos = axis_combinations(eval);
        for sample in &eval.samples {
            if let Some(tag) = &args.tag
                && !sample.tags.contains(tag)
            {
                continue;
            }
            for model in &eval.models {
                if let Some(allow) = model_filter
                    && !allow.contains(&model.label)
                {
                    continue;
                }
                for params in &combos {
                    let key = mira::cell_key(&eval.name, &sample.id, &model.label, params);
                    if let Some(f) = &args.filter
                        && !key.contains(f.as_str())
                    {
                        continue;
                    }
                    plan.push(Cell {
                        eval: eval.name.clone(),
                        sample: sample.id.clone(),
                        model: model.label.clone(),
                        params: params.clone(),
                    });
                }
            }
        }
    }
    plan
}

fn print_listing(listing: &ListResult) {
    for eval in &listing.evals {
        let desc = if eval.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", eval.description)
        };
        println!("{}{desc}  (max_turns={})", eval.name, eval.max_turns);
        println!(
            "  samples: {}",
            eval.samples
                .iter()
                .map(|s| if s.tags.is_empty() {
                    s.id.clone()
                } else {
                    format!("{} [{}]", s.id, s.tags.join(","))
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  scorers: {}", eval.scorers.join(", "));
        println!(
            "  models:  {}",
            eval.models
                .iter()
                .map(|m| if m.available {
                    m.label.clone()
                } else {
                    format!("{} (unavailable)", m.label)
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !eval.axes.is_empty() {
            let axes: Vec<String> = eval
                .axes
                .iter()
                .map(|a| format!("{}=[{}]", a.name, a.values.join(",")))
                .collect();
            println!("  axes:    {}", axes.join(", "));
        }
        if !eval.metadata.is_empty() {
            let meta: Vec<String> = eval
                .metadata
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            println!("  meta:    {}", meta.join(", "));
        }
    }
}
