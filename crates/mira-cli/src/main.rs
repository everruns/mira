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
//! mira --bin greet run --save                   # archive run under ./results/<run_id>/
//! mira --bin greet run --execute-only --artifacts art/  # capture transcripts
//! mira --bin greet score --artifacts art/        # score (or re-score) them
//! ```
//!
//! Execution and scoring can be split: `run --execute-only` captures one
//! full-transcript artifact per cell (for long-running subjects), and `score`
//! (re-)scores those artifacts without re-executing the subject.
//!
//! Each Rust example is a crate exposing a like-named binary, so `--bin <name>`
//! resolves it across the workspace. Point it at any study: `--bin NAME`,
//! `--example NAME`, an arbitrary `--cmd "..."` (e.g. a Python study), or
//! another package with `--package` / `--manifest-path`.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use clap::{Args, CommandFactory, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::process::Command;

mod config;
mod env;

use mira::Host;
use mira::Trial;
use mira::exec::{self, CellSpec, Concurrency};
use mira::protocol::{
    ExecuteResult, InitializeResult, ListResult, RunResult, TranscriptSummary, capabilities,
};
use mira::report::{self, Format};
use mira::run::{RUN_META_FORMAT, RunMeta, RunSummary, new_run_id_at};
use mira::session::{self, Session, now_unix};

/// Repository, issue tracker, and docs — surfaced in `mira help --full` and the
/// `--help` footer so an agent (or human) can always find their way home.
const REPO_URL: &str = "https://github.com/everruns/mira";
const ISSUES_URL: &str = "https://github.com/everruns/mira/issues";
const DOCS_URL: &str = "https://github.com/everruns/mira/tree/main/docs";
const API_DOCS_URL: &str = "https://docs.rs/mira-eval";

/// Short tagline for `-h`/`--help`. The CLI is the *host*: it plans the run,
/// drives subjects over the protocol, scores, and reports — not just a runner.
const ABOUT: &str = "Run code-first evals for agents and tools across a model matrix — \
the Mira host CLI.";

/// Footer on every `--help`/no-args screen. The one breadcrumb an agent needs to
/// discover the long-form guide.
const HELP_HINT: &str = "Tip: run `mira help --full` for an overview, every flag, examples, \
and links (repo, issues, docs).";

#[derive(Parser)]
#[command(
    name = "mira",
    version,
    about = ABOUT,
    after_help = HELP_HINT,
    disable_help_subcommand = true,
)]
struct Cli {
    #[command(flatten)]
    target: Target,
    #[command(subcommand)]
    cmd: Option<Cmd>,
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
    /// Score (or re-score) previously captured execution artifacts.
    Score(ScoreArgs),
    /// Show help. Add `--full` for an overview, every flag, examples, and links.
    Help(HelpArgs),
}

#[derive(Args)]
struct HelpArgs {
    /// Render the full guide: high-level overview, all flags, examples, and
    /// contact links — written so an agent can self-orient in one read.
    #[arg(long)]
    full: bool,
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
    /// Run each cell this many times (trials/repetitions) for pass@k / variance.
    /// Overrides the eval's declared trials. The host groups the repetitions and
    /// reports pass-rate, pass@k, and score standard deviation per cell.
    #[arg(long)]
    trials: Option<usize>,
    /// Base seed for reproducible trials: trial `t` runs with seed `seed + t`.
    /// Overrides the eval's declared seed; the subject reads it via `cx.seed()`.
    #[arg(long)]
    seed: Option<u64>,
    /// Write a report file (see --format).
    #[arg(long)]
    out: Option<String>,
    /// Save a timestamped run folder (report.json/html + meta.json) under the
    /// results dir, so runs accumulate and can be compared later. With no value
    /// uses `[results].dir` from mira.toml, else `./results`; pass a dir to
    /// override.
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "DIR")]
    save: Option<String>,
    /// Report file format: json | junit | md | html.
    #[arg(long, default_value = "json")]
    format: String,
    /// Persist/resume results here; completed cells are skipped on re-run.
    #[arg(long)]
    checkpoint: Option<String>,
    /// Ignore an existing checkpoint/artifact and run everything fresh.
    #[arg(long)]
    fresh: bool,
    /// Max cells to run in parallel across all providers.
    #[arg(long, short = 'j', default_value_t = 8)]
    max_concurrent: usize,
    /// Per-provider concurrency ceilings, e.g. `anthropic=2,openai=4`. Caps a
    /// single provider below the global limit so it can't be flooded.
    #[arg(long)]
    provider_concurrency: Option<String>,
    /// Disable adaptive throttling (don't shrink a provider's concurrency or
    /// retry when it returns rate-limit / overload errors).
    #[arg(long)]
    no_adaptive: bool,
    /// Times a rate-limited cell is retried (after backoff) before it's failed.
    #[arg(long, default_value_t = 4)]
    max_retries: u32,
    /// Execute subjects only (no scoring), writing full transcripts to
    /// --artifacts for later `mira score`. For long-running subjects.
    /// --checkpoint/--out don't apply in this mode (no scores are produced).
    #[arg(long, requires = "artifacts", conflicts_with_all = ["checkpoint", "out", "save"])]
    execute_only: bool,
    /// Directory for full-transcript execution artifacts (one JSON per cell).
    /// Written when --execute-only; read by `mira score`.
    #[arg(long)]
    artifacts: Option<String>,
}

#[derive(Args)]
struct ScoreArgs {
    /// Substring filter on the case key `eval/sample@model`.
    filter: Option<String>,
    /// Directory of execution artifacts written by `run --execute-only`.
    #[arg(long)]
    artifacts: String,
    /// Write a report file (see --format).
    #[arg(long)]
    out: Option<String>,
    /// Save a timestamped run folder under the results dir (see `run --save`).
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "DIR")]
    save: Option<String>,
    /// Report file format: json | junit | md | html.
    #[arg(long, default_value = "json")]
    format: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Help paths need no study process — handle them before spawning anything.
    // Bare `mira` (no subcommand) prints help too, so an agent landing here sees
    // the `mira help --full` breadcrumb instead of an opaque error.
    match &cli.cmd {
        None => {
            Cli::command().print_help()?;
            return Ok(());
        }
        Some(Cmd::Help(args)) => {
            if args.full {
                print_full_help()?;
            } else {
                Cli::command().print_help()?;
            }
            return Ok(());
        }
        _ => {}
    }

    // One progress bar, shared with the event handler so study `log` lines print
    // cleanly above the bar (via `suspend`) instead of corrupting it. It starts
    // hidden; `run` gives it a length and a draw target once the plan is known.
    let progress = Arc::new(ProgressBar::hidden());
    let progress_evt = progress.clone();
    let host = Host::spawn(build_command(&cli.target))
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
        Some(Cmd::List) => {
            print_listing(&listing);
            host.shutdown().await?;
            Ok(())
        }
        Some(Cmd::Run(args)) => run(host, info, listing, args, progress).await,
        Some(Cmd::Score(args)) => score(host, info, args).await,
        // Help/no-args returned earlier, before the host was spawned.
        None | Some(Cmd::Help(_)) => unreachable!("handled before host spawn"),
    }
}

/// `mira help --full`: the self-orienting guide. High-level description, the full
/// flag set (clap's own rendering, so it never drifts from the parser), worked
/// examples, and contact links — one read to get an agent productive.
fn print_full_help() -> std::io::Result<()> {
    use std::io::Write;

    let overview = "\
OVERVIEW
  Mira is a Rust-first, code-first evaluation framework for agents and tools —
  built for multi-turn, tool-using, long-running trajectories.

  You write evals as code (in Rust, or any language that speaks the protocol);
  this binary is the HOST. It owns the run end to end: it launches your eval
  program (the `study`), enumerates what it advertises, plans the grid
  (selection x model matrix x axes), executes each cell over the protocol,
  scores the results, then aggregates, reports, and checkpoints. Execution and
  scoring can be split for long runs (`run --execute-only` then `score`).

  Point it at any study: `--bin NAME`, `--example NAME`, an arbitrary
  `--cmd \"...\"` (e.g. a Python study), or `--package` / `--manifest-path`.";

    let examples = "\
EXAMPLES
  mira --bin greet list                 # what the study advertises
  mira --bin greet run                  # run the whole matrix
  mira --bin greet run greet            # selective (substring), like cargo test
  mira --bin greet run --tag smoke      # only samples carrying a tag
  mira --bin greet run --models sim --format junit --out results.xml
  mira --bin greet run --format html --out report.html   # self-contained viewer
  mira --bin greet run --checkpoint ck.json              # resumable long runs
  mira --bin greet run --save                            # archive run under ./results/<run_id>/
  mira --bin greet run --execute-only --artifacts art/   # capture transcripts
  mira --bin greet score --artifacts art/                # score (or re-score) them
  mira --cmd \"python study.py\" run     # drive a non-Rust (polyglot) study";

    let links = format!(
        "\
LINKS
  Repository:  {REPO_URL}
  Issues:      {ISSUES_URL}
  Docs:        {DOCS_URL}
  API docs:    {API_DOCS_URL}"
    );

    // The full flag set, straight from the parser so it never drifts. Strip the
    // about header and after_help footer — we frame those ourselves here.
    let flags = Cli::command()
        .about(None)
        .after_help(None)
        .render_long_help();

    let mut out = std::io::stdout().lock();
    writeln!(out, "{ABOUT}\n")?;
    writeln!(out, "{overview}\n")?;
    write!(out, "{flags}")?;
    writeln!(out, "\n{examples}\n")?;
    writeln!(out, "{links}")?;
    Ok(())
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
    host: Host,
    info: InitializeResult,
    listing: ListResult,
    args: RunArgs,
    progress: Arc<ProgressBar>,
) -> Result<(), Box<dyn std::error::Error>> {
    let format = Format::from_str(&args.format)?;
    // Capture run identity at invocation start so the sortable run-id timestamp
    // matches `started_unix` (not the later finish time).
    let started_unix = now_unix();
    let run_id = new_run_id_at(started_unix);
    let save_dir = config::resolve_save_dir(&args.save);
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

    // Execute-only: run subjects, persist full transcripts, defer scoring.
    if args.execute_only {
        require_capability(&info, capabilities::EXECUTE, "--execute-only")?;
        let dir = args.artifacts.as_ref().expect("clap requires artifacts");
        return execute_only(host, &plan, dir, args.fresh).await;
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

    // Only run cells not already checkpointed.
    let todo: Vec<CellSpec> = plan
        .iter()
        .filter(|cell| !done.contains_key(&cell.key()))
        .cloned()
        .collect();

    let cfg = concurrency(&args);

    // Run cells concurrently under the bounded, provider-aware policy. Each
    // finished cell advances the bar and is persisted to the session checkpoint
    // as it lands, so a long run stays resumable.
    {
        let handle = host.handle();
        exec::run_cells(
            todo,
            &cfg,
            |cell| {
                let handle = handle.clone();
                async move {
                    handle
                        .run(
                            &cell.eval,
                            &cell.sample,
                            &cell.model,
                            &cell.params,
                            cell.trial,
                        )
                        .await
                }
            },
            |cell, result| {
                progress.set_message(cell.key());
                done.insert(cell.key(), result);
                progress.inc(1);
                if let Some(path) = &args.checkpoint {
                    // Persist only the planned cells, in plan order — so the file
                    // stays deterministic and doesn't accumulate cells dropped by a
                    // narrower selection on resume.
                    let results = plan
                        .iter()
                        .filter_map(|c| done.get(&c.key()).cloned())
                        .collect();
                    session.update(plan.len(), fingerprints.clone(), results);
                    if let Err(e) = session.save(path) {
                        progress.suspend(|| eprintln!("warning: failed to write checkpoint: {e}"));
                    }
                }
            },
        )
        .await;
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

    if let Some(base) = &save_dir {
        save_results(base, &run_id, &info, started_unix, &results)?;
    }

    // A cell that's N/A (all scores N/A — e.g. an infra failure) is neither
    // passed nor failed, so it doesn't make CI red.
    let failed = results
        .iter()
        .any(|r| !r.skipped && !report::is_na(r) && !r.passed);
    std::process::exit(if failed { 1 } else { 0 });
}

/// Write a timestamped run folder under `base` and report where it landed. The
/// `run_id` is captured at invocation start so it sorts by start time.
fn save_results(
    base: &str,
    run_id: &str,
    info: &InitializeResult,
    started_unix: u64,
    results: &[RunResult],
) -> Result<(), Box<dyn std::error::Error>> {
    // Capture environment context (commit, box, host version, labels) unless
    // disabled in mira.toml. Best-effort: never fails the save.
    let cfg = config::Config::load();
    let environment = cfg
        .environment
        .enabled
        .then(|| env::collect(&cfg.environment.labels))
        .flatten();
    let meta = RunMeta {
        format: RUN_META_FORMAT,
        run_id: run_id.to_string(),
        study: info.study.clone(),
        study_version: info.study_version.clone(),
        started_unix,
        finished_unix: now_unix(),
        environment,
        summary: RunSummary::of(results),
    };
    let dir = config::save_run(base, &meta, results)?;
    eprintln!("\nsaved run {} to {}", meta.run_id, dir.display());
    Ok(())
}

/// Build the concurrency policy from the run flags.
fn concurrency(args: &RunArgs) -> Concurrency {
    let mut cfg = Concurrency::new(args.max_concurrent);
    cfg.adaptive = !args.no_adaptive;
    cfg.max_retries = args.max_retries;
    if let Some(spec) = &args.provider_concurrency {
        for entry in spec.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some((provider, n)) = entry.split_once('=')
                && let Ok(limit) = n.trim().parse::<usize>()
            {
                cfg = cfg.provider(provider.trim(), limit);
            } else {
                eprintln!("ignoring malformed --provider-concurrency entry: {entry:?}");
            }
        }
    }
    cfg
}

/// Error out if the study didn't advertise `cap`, naming the `feature` that needs
/// it — so a `1.0`/`run`-only study fails fast with a clear message rather than a
/// generic RPC error mid-run.
fn require_capability(
    info: &InitializeResult,
    cap: &str,
    feature: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if info.capabilities.iter().any(|c| c == cap) {
        return Ok(());
    }
    Err(format!(
        "study {} doesn't support {feature}: it doesn't advertise the `{cap}` capability \
         (needs protocol >= 1.1)",
        info.study
    )
    .into())
}

/// `run --execute-only`: run each cell's subject, persist the full transcript as
/// an artifact, and skip scoring. Resumable — a cell whose artifact already
/// exists is skipped unless `--fresh`.
async fn execute_only(
    host: Host,
    plan: &[CellSpec],
    dir: &str,
    fresh: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let mut wrote = 0usize;
    for cell in plan {
        let path = artifact_path(dir, &cell.key());
        if !fresh && path.exists() {
            continue;
        }
        let result = host
            .execute(
                &cell.eval,
                &cell.sample,
                &cell.model,
                &cell.params,
                cell.trial,
            )
            .await?;
        std::fs::write(&path, serde_json::to_string_pretty(&result)?)?;
        wrote += 1;
    }
    host.shutdown().await?;
    eprintln!("executed {wrote} cell(s); artifacts in {dir}");
    eprintln!("score them with: mira score --artifacts {dir}");
    Ok(())
}

/// `score`: load execution artifacts, (re-)score each via the study, and report.
/// Re-running this over the same artifacts is a re-score (e.g. after a scorer
/// change) — no subject is re-executed.
async fn score(
    host: Host,
    info: InitializeResult,
    args: ScoreArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    require_capability(&info, capabilities::SCORE, "mira score")?;
    let format = Format::from_str(&args.format)?;
    let started_unix = now_unix();
    let run_id = new_run_id_at(started_unix);
    let save_dir = config::resolve_save_dir(&args.save);
    let mut artifacts = load_artifacts(&args.artifacts);
    if let Some(f) = &args.filter {
        artifacts.retain(|a| a.key().contains(f.as_str()));
    }
    if artifacts.is_empty() {
        eprintln!("no artifacts in {}", args.artifacts);
    }

    let mut results = Vec::with_capacity(artifacts.len());
    for artifact in &artifacts {
        // A skipped (unexecuted) cell has no transcript to score; pass it through.
        if artifact.skipped {
            results.push(skipped_result(artifact));
        } else {
            results.push(host.score(artifact).await?);
        }
    }
    host.shutdown().await?;

    report::print_results(&results);

    if let Some(path) = &args.out {
        std::fs::write(path, report::render(&results, format))?;
        eprintln!("\nwrote {path} ({:?})", format);
    }

    if let Some(base) = &save_dir {
        save_results(base, &run_id, &info, started_unix, &results)?;
    }

    // A cell that's N/A (all scores N/A — e.g. an infra failure) is neither
    // passed nor failed, so it doesn't make CI red.
    let failed = results
        .iter()
        .any(|r| !r.skipped && !report::is_na(r) && !r.passed);
    std::process::exit(if failed { 1 } else { 0 });
}

/// A skipped (unexecuted) artifact carried straight to a `RunResult`.
fn skipped_result(a: &ExecuteResult) -> RunResult {
    RunResult {
        eval: a.eval.clone(),
        sample: a.sample.clone(),
        model: a.model.clone(),
        params: a.params.clone(),
        trial: a.trial,
        trials: a.trials,
        seed: a.seed,
        passed: false,
        aggregate: 0.0,
        scores: Vec::new(),
        transcript: TranscriptSummary::of(&a.transcript),
        skipped: true,
    }
}

/// Filesystem path for a cell's artifact under `dir`. The cell key is encoded
/// reversibly — `[A-Za-z0-9]` kept verbatim, every other byte escaped as `_HH`
/// (hex) — so distinct keys can never collide onto the same filename (which would
/// overwrite an artifact or wrongly skip execution on resume).
fn artifact_path(dir: &str, key: &str) -> std::path::PathBuf {
    let mut safe = String::with_capacity(key.len());
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() {
            safe.push(b as char);
        } else {
            safe.push('_');
            safe.push_str(&format!("{b:02x}"));
        }
    }
    Path::new(dir).join(format!("{safe}.json"))
}

/// Load every execution artifact in `dir`, sorted by cell key for stable order.
/// Unreadable or invalid files are skipped with a warning, so a corrupted or
/// partially-written artifact is visible rather than silently dropped.
fn load_artifacts(dir: &str) -> Vec<ExecuteResult> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("warning: cannot read artifacts dir {dir}: {e}");
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<ExecuteResult>(&text) {
                Ok(result) => out.push(result),
                Err(e) => {
                    eprintln!(
                        "warning: skipping {}: invalid artifact JSON: {e}",
                        path.display()
                    )
                }
            },
            Err(e) => eprintln!("warning: skipping {}: {e}", path.display()),
        }
    }
    out.sort_by_key(|a| a.key());
    out
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

/// Expand the advertised listing into an ordered, selected list of cells. Each
/// cell carries its model's provider so the executor can bucket concurrency.
fn plan_grid(
    listing: &ListResult,
    args: &RunArgs,
    model_filter: &Option<Vec<String>>,
) -> Vec<CellSpec> {
    let mut plan = Vec::new();
    for eval in &listing.evals {
        let combos = axis_combinations(eval);
        // Trials: --trials overrides the eval's declared count (0/1 → single).
        // Seed base: --seed overrides the eval's declared seed; trial t uses
        // `base + t` so the repetition set replays deterministically.
        let trials = args.trials.unwrap_or(eval.trials).max(1);
        let seed_base = args.seed.or(eval.seed);
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
                    // Filter on the logical key, so `--filter` keeps or drops all
                    // trials of a cell together (a stable group to aggregate).
                    if let Some(f) = &args.filter
                        && !key.contains(f.as_str())
                    {
                        continue;
                    }
                    for index in 0..trials {
                        plan.push(CellSpec {
                            eval: eval.name.clone(),
                            sample: sample.id.clone(),
                            model: model.label.clone(),
                            provider: model.provider.clone(),
                            params: params.clone(),
                            trial: Trial {
                                index,
                                count: trials,
                                // wrapping_add so a huge --seed can't panic
                                // (debug) or differ by build mode (release).
                                seed: seed_base.map(|s| s.wrapping_add(index as u64)),
                            },
                        });
                    }
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
        let trials = if eval.trials > 1 {
            let seed = eval.seed.map(|s| format!(", seed={s}")).unwrap_or_default();
            format!(", trials={}{seed}", eval.trials)
        } else {
            String::new()
        };
        println!(
            "{}{desc}  (max_turns={}{trials})",
            eval.name, eval.max_turns
        );
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
                .map(|(k, v)| format!("{k}={}", mira::metadata_display(v)))
                .collect();
            println!("  meta:    {}", meta.join(", "));
        }
    }
}
