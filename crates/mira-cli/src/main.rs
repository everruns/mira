//! `mira` — the host CLI. Compiles + spawns an eval **server** (a program that
//! calls `mira::serve`), enumerates its evals, plans the run (selection ×
//! matrix), executes each cell over the protocol, then aggregates, saves, and
//! checkpoints.
//!
//! ```bash
//! mira --example greet list
//! mira --example greet run                          # all cells (sim runs; keyed cells skip)
//! mira --example greet run greet                    # substring filter
//! mira --example greet run --tag smoke
//! mira --example greet run --models sim --format junit --out results.xml
//! mira --example greet run --checkpoint ck.json     # resumable
//! ```
//!
//! Point it at any server: `--bin NAME`, `--example NAME`, an arbitrary
//! `--cmd "..."`, or another package with `--package` / `--manifest-path`.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};
use tokio::process::Command;

use mira::Host;
use mira::protocol::{ListResult, RunResult};
use mira::report::{self, Format};

#[derive(Parser)]
#[command(name = "mira", version, about = "Host runner for code-defined evals")]
struct Cli {
    #[command(flatten)]
    target: Target,
    #[command(subcommand)]
    cmd: Cmd,
}

/// How to launch the eval server process.
#[derive(Args)]
struct Target {
    /// Run `cargo run -q --bin <NAME>`.
    #[arg(long, global = true)]
    bin: Option<String>,
    /// Run `cargo run -q --example <NAME>` (default: greet).
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
    /// List the evals, samples, scorers, and models the server advertises.
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
    /// Report file format: json | junit | md.
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
    let mut host = Host::spawn(build_command(&cli.target))
        .await?
        .on_event(|n| {
            if n.method == "event" {
                let p = &n.params;
                eprintln!(
                    "  · {}/{}@{} {}",
                    p["eval"].as_str().unwrap_or("?"),
                    p["sample"].as_str().unwrap_or("?"),
                    p["model"].as_str().unwrap_or("?"),
                    p["kind"].as_str().unwrap_or(""),
                );
            } else if n.method == "log"
                && let Some(msg) = n.params["message"].as_str()
            {
                eprintln!("  server: {msg}");
            }
        });

    let info = host.initialize("mira-cli").await?;
    eprintln!(
        "server {} · protocol {} · {} evals",
        info.server, info.protocol_version, info.evals
    );
    let listing = host.list().await?;

    match cli.cmd {
        Cmd::List => {
            print_listing(&listing);
            host.shutdown().await?;
            Ok(())
        }
        Cmd::Run(args) => run(host, listing, args).await,
    }
}

/// Build the server launch command from the target flags.
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
    } else {
        // Default to the bundled demo example.
        let example = target.example.as_deref().unwrap_or("greet");
        command.arg("--example").arg(example);
    }
    if let Some(manifest) = &target.manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    command
}

async fn run(
    mut host: Host,
    listing: ListResult,
    args: RunArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let format = Format::from_str(&args.format)?;
    let model_filter: Option<Vec<String>> = args
        .models
        .as_ref()
        .map(|m| m.split(',').map(|s| s.trim().to_string()).collect());

    // Plan the full grid, then apply selection. Done up front so the host owns
    // selection/matrix without the server re-running anything.
    let plan = plan_grid(&listing, &args, &model_filter);
    if plan.is_empty() {
        eprintln!("no cells matched the selection");
    }

    // Resume from a checkpoint unless --fresh.
    let mut done: BTreeMap<String, RunResult> = BTreeMap::new();
    if let Some(path) = &args.checkpoint
        && !args.fresh
    {
        done = load_checkpoint(path);
        if !done.is_empty() {
            eprintln!("resuming checkpoint: {} cells already done", done.len());
        }
    }

    for (eval, sample, model) in &plan {
        let key = format!("{eval}/{sample}@{model}");
        if done.contains_key(&key) {
            continue;
        }
        let result = host.run(eval, sample, model).await?;
        done.insert(key, result);
        if let Some(path) = &args.checkpoint {
            save_checkpoint(path, &done);
        }
    }
    host.shutdown().await?;

    // Report only the planned cells, in plan order.
    let results: Vec<RunResult> = plan
        .iter()
        .filter_map(|(e, s, m)| done.get(&format!("{e}/{s}@{m}")).cloned())
        .collect();

    report::print_results(&results);

    if let Some(path) = &args.out {
        std::fs::write(path, report::render(&results, format))?;
        eprintln!("\nwrote {path} ({:?})", format);
    }

    let failed = results.iter().any(|r| !r.skipped && !r.passed);
    std::process::exit(if failed { 1 } else { 0 });
}

/// Expand the advertised listing into an ordered, selected list of cells.
fn plan_grid(
    listing: &ListResult,
    args: &RunArgs,
    model_filter: &Option<Vec<String>>,
) -> Vec<(String, String, String)> {
    let mut plan = Vec::new();
    for eval in &listing.evals {
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
                let key = format!("{}/{}@{}", eval.name, sample.id, model.label);
                if let Some(f) = &args.filter
                    && !key.contains(f.as_str())
                {
                    continue;
                }
                plan.push((eval.name.clone(), sample.id.clone(), model.label.clone()));
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

fn load_checkpoint(path: &str) -> BTreeMap<String, RunResult> {
    let mut map = BTreeMap::new();
    if !Path::new(path).exists() {
        return map;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    if let Ok(results) = serde_json::from_str::<Vec<RunResult>>(&text) {
        for r in results {
            map.insert(r.key(), r);
        }
    }
    map
}

fn save_checkpoint(path: &str, done: &BTreeMap<String, RunResult>) {
    let results: Vec<&RunResult> = done.values().collect();
    if let Ok(text) = serde_json::to_string_pretty(&results) {
        let _ = std::fs::write(path, text);
    }
}
