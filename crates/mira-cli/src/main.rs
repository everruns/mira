//! `mira` — the host CLI. Compiles + spawns an eval **study** (a program that
//! calls `mira::Study::registered().serve()`), enumerates its evals, plans the
//! run (selection × matrix), executes each case over the protocol, then
//! aggregates and saves the run.
//!
//! Every run is saved by default under the results dir as `<run_id>/` (per-case
//! results + report + meta), so it can be resumed and re-reported. `--dry-run`
//! opts out.
//!
//! ```bash
//! mira --script study.rs list
//! mira --script study.rs run                    # all cases (sim runs; keyed cases skip)
//! mira --script study.rs run greet              # substring filter
//! mira --script study.rs run --tag smoke
//! mira --script study.rs run --targets sim --format junit --out results.xml
//! mira --script study.rs run --dry-run          # don't save a run folder
//! mira --script study.rs run --resume <run_id>  # finish an interrupted run
//! mira --script study.rs report <run_id>        # re-render a saved run
//! mira --script study.rs run --execute-only --artifacts art/  # capture transcripts
//! mira --script study.rs score --artifacts art/  # score (or re-score) them
//! mira --script study.rs doctor --fix           # diagnose the setup; apply safe fixes
//! ```
//!
//! Execution and scoring can be split: `run --execute-only` captures one
//! full-transcript artifact per case (for long-running subjects), and `score`
//! (re-)scores those artifacts without re-executing the subject.
//!
//! Point it at any study: a single-file Rust study via `--script study.rs`
//! (cargo-script frontmatter, shimmed onto stable), a crate via `--bin NAME` /
//! `--example NAME`, an arbitrary `--cmd "..."`, a non-Rust study via
//! `--uv` / `--python` / `--python3 SCRIPT`, or another package with
//! `--package` / `--manifest-path`. Save a repo's invocation as
//! `[launchers.NAME]` in `mira.toml` and select it with `--launcher NAME` (or a
//! `default_launcher`) instead of retyping the flags.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, CommandFactory, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::process::Command;

mod config;
mod doctor;
mod env;

use mira::Host;
use mira::Trial;
use mira::exec::{self, CaseSpec, Concurrency};
use mira::protocol::{
    ExecuteResult, InitializeResult, ListResult, RunResult, TranscriptSummary, capabilities,
};
use mira::report::{self, Format};
use mira::run::{RUN_META_FORMAT, RunMeta, RunSummary, new_run_id_at, now_unix};

/// Repository, issue tracker, and docs — surfaced in `mira help --full` and the
/// `--help` footer so an agent (or human) can always find their way home.
const REPO_URL: &str = "https://github.com/everruns/mira";
const ISSUES_URL: &str = "https://github.com/everruns/mira/issues";
const DOCS_URL: &str = "https://github.com/everruns/mira/tree/main/docs";
const API_DOCS_URL: &str = "https://docs.rs/mira-eval";
/// The `mira` agent skill — the agent-facing entry point that teaches a coding
/// agent to author and run evals. Surfaced in `mira help --full` so an agent can
/// load it on demand. See `skills/mira/SKILL.md` (design of record: specs/docs.md).
const SKILL_URL: &str = "https://github.com/everruns/mira/tree/main/skills/mira";

/// The guides under `docs/`, mirrored here for progressive disclosure: an agent
/// reading `mira help --full` sees what each doc covers without fetching the
/// tree first. Keep in sync with docs/README.md (the index is the design of record).
const GUIDES: &[(&str, &str)] = &[
    (
        "how-it-works",
        "the core model and moving parts, end to end",
    ),
    ("getting-started", "zero to a passing run"),
    (
        "authoring",
        "datasets, the model matrix, axes, metadata, infra-errors vs failures",
    ),
    (
        "scorers",
        "built-ins, budgets, combinators, closures, LLM-judge",
    ),
    ("metrics", "tokens/cost/latency and custom numeric metrics"),
    ("subjects", "in-process, CLI/polyglot, and runtime sessions"),
    (
        "extensibility",
        "every seam: subjects, scorers, metrics, events, protocol",
    ),
    ("protocol", "the normative wire format and its versioning"),
];

/// Short tagline for `-h`/`--help`. The CLI is the *host*: it plans the run,
/// drives subjects over the protocol, scores, and reports — not just a runner.
const ABOUT: &str = "Run code-first evals for agents and tools across a target matrix — \
the Mira host CLI.";

/// Footer on every `--help`/no-args screen. The one breadcrumb an agent needs to
/// discover the long-form guide.
const HELP_HINT: &str = "Tip: run `mira help --full` for an overview, every flag, examples, \
the doc guides, the agent skill, and links.";

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
    launcher: Launcher,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// How to launch the eval study process (not to be confused with a
/// [`mira::Target`] — the model/harness under evaluation).
#[derive(Args)]
struct Launcher {
    /// Use a named launcher from `mira.toml` (`[launchers.NAME]`): a saved
    /// bin/example/cmd (+ package/manifest). The explicit launch flags below
    /// override its fields; `default_launcher` is used when neither a flag nor
    /// `--launcher` selects one.
    #[arg(long, global = true, value_name = "NAME")]
    launcher: Option<String>,
    /// Run `cargo run -q --bin <NAME>` (defaults to `greet`).
    #[arg(long, global = true)]
    bin: Option<String>,
    /// Run `cargo run -q --example <NAME>`.
    #[arg(long, global = true)]
    example: Option<String>,
    /// Launch an arbitrary command (split on whitespace).
    #[arg(long, global = true)]
    cmd: Option<String>,
    /// Run a single-file Rust study (`study.rs` with cargo-script frontmatter).
    /// Compiled on stable via a built-in shim; set MIRA_SCRIPT_NATIVE=1 to use
    /// `cargo -Zscript` on nightly instead.
    #[arg(long, global = true, value_name = "SCRIPT")]
    script: Option<String>,
    /// Run `uv run <SCRIPT...>` (e.g. a Python study; split on whitespace).
    #[arg(long, global = true, value_name = "SCRIPT")]
    uv: Option<String>,
    /// Run `python <SCRIPT...>` (split on whitespace).
    #[arg(long, global = true, value_name = "SCRIPT")]
    python: Option<String>,
    /// Run `python3 <SCRIPT...>` (split on whitespace).
    #[arg(long, global = true, value_name = "SCRIPT")]
    python3: Option<String>,
    /// Cargo package to run the bin/example from (`-p`).
    #[arg(long, global = true)]
    package: Option<String>,
    /// Passed through to cargo.
    #[arg(long, global = true)]
    manifest_path: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the evals, samples, scorers, and targets the study advertises.
    List,
    /// Run selected cases and report. Boxed: `RunArgs` is much larger than the
    /// other variants, so boxing keeps the enum small (clippy large_enum_variant).
    Run(Box<RunArgs>),
    /// Score (or re-score) previously captured execution artifacts.
    Score(ScoreArgs),
    /// Re-render a saved run's reports from its stored results (no study needed).
    Report(ReportArgs),
    /// Publish a saved run's results to a hosted viewer (everruns).
    Publish(PublishArgs),
    /// Diagnose the setup: mira.toml (keys, launchers, presets), the study's
    /// listing (samples/targets/axes/matrix), and saved runs. `--fix` repairs
    /// what's safe.
    Doctor(DoctorArgs),
    /// Show help. Add `--full` for an overview, every flag, examples, and links.
    Help(HelpArgs),
}

/// everruns connection flags, shared by `run --publish` and `publish`. Each
/// falls back to an env var, then the everruns CLI credentials file, so a prior
/// `everruns login` is enough.
#[derive(Args, Clone, Default)]
struct PublishConn {
    /// everruns API base URL (else $EVERRUNS_API_URL, else credentials file).
    #[arg(long = "everruns-url")]
    api_url: Option<String>,
    /// everruns API key / PAT (else $EVERRUNS_API_KEY, else credentials file).
    #[arg(long = "everruns-api-key")]
    api_key: Option<String>,
    /// everruns org id (else $EVERRUNS_ORG_ID, else credentials file).
    #[arg(long = "everruns-org")]
    org_id: Option<String>,
    /// everruns credentials profile (else its `current_profile`).
    #[arg(long = "everruns-profile")]
    profile: Option<String>,
}

impl PublishConn {
    fn to_options(&self) -> mira_publish_everruns::PublishOptions {
        mira_publish_everruns::PublishOptions {
            base_url: self.api_url.clone(),
            api_key: self.api_key.clone(),
            org_id: self.org_id.clone(),
            profile: self.profile.clone(),
        }
    }
}

/// `mira publish <run_id>`: send a saved run's results to a hosted viewer.
#[derive(Args)]
struct PublishArgs {
    /// The `<run_id>` of a saved run under the results dir.
    run_id: String,
    /// Substring filter on the case key `eval/sample@target`.
    filter: Option<String>,
    /// Target viewer. Currently only `everruns`.
    #[arg(long, default_value = "everruns")]
    to: String,
    #[command(flatten)]
    conn: PublishConn,
}

/// `mira doctor`: check config, study listing, and saved runs; report findings
/// (warnings never fail; errors exit non-zero) and optionally apply safe fixes.
#[derive(Args)]
struct DoctorArgs {
    /// Apply the fixes doctor knows are safe (remove leftover temp files from
    /// interrupted writes, re-render a finished run's missing reports). Without
    /// it, fixable findings are only listed.
    #[arg(long)]
    fix: bool,
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
    /// Substring filter on the case key `eval/sample@target` (the `cargo test
    /// PAT` convenience). For precise, per-dimension selection use the glob
    /// flags `--targets` / `--samples` / `--evals`.
    filter: Option<String>,
    /// Only run samples carrying this tag.
    #[arg(long)]
    tag: Option<String>,
    /// Restrict the primary (target) axis to labels matching these globs
    /// (comma-separated), e.g. `--targets 'anthropic/*'` or `--targets sim`.
    /// Sugar for `--axis target=…`.
    #[arg(long)]
    targets: Option<String>,
    /// Restrict to sample ids matching these globs (comma-separated), e.g.
    /// `--samples 'geo/*'` or `--samples france,spain`.
    #[arg(long)]
    samples: Option<String>,
    /// Restrict to evals whose name matches these globs (comma-separated),
    /// hence which subjects run.
    #[arg(long)]
    evals: Option<String>,
    /// Restrict a matrix axis to a subset, e.g. `--axis effort=high,low`
    /// (repeatable). `NAME` is `target` (the primary axis) or any declared axis;
    /// values OR within one flag, multiple `--axis` flags AND (intersect).
    #[arg(long = "axis", value_name = "NAME=V1,V2")]
    axes: Vec<String>,
    /// Apply a named selection preset from `mira.toml` (`[presets.NAME]`): a saved
    /// bundle of targets / samples / evals / axes / tag. Explicit flags override
    /// the preset.
    #[arg(long)]
    preset: Option<String>,
    /// Run each case this many times (trials/repetitions) for pass@k / variance.
    /// Overrides the eval's declared trials. The host groups the repetitions and
    /// reports pass-rate, pass@k, and score standard deviation per case.
    #[arg(long)]
    trials: Option<usize>,
    /// Base seed for reproducible trials: trial `t` runs with seed `seed + t`.
    /// Overrides the eval's declared seed; the subject reads it via `cx.seed()`.
    #[arg(long)]
    seed: Option<u64>,
    /// Break resolve-rate down by a metadata key (e.g. `repo`, `difficulty`,
    /// `agent`). The value is resolved per case from, in order: axis params,
    /// sample metadata, target metadata, then transcript metadata.
    #[arg(long)]
    group_by: Option<String>,
    /// Also write a standalone report file here (see --format). Independent of the
    /// saved run folder, which is always written unless --dry-run.
    #[arg(long)]
    out: Option<String>,
    /// Report file format for --out: json | jsonl | csv | junit | md | html.
    #[arg(long, default_value = "json")]
    format: String,
    /// Don't save a run folder (ephemeral run). By default every run is saved
    /// under the results dir (`[results].dir` in mira.toml, else `./results`) as
    /// `<run_id>/` with per-case results, so it can be resumed and re-reported.
    #[arg(long)]
    dry_run: bool,
    /// Resume an interrupted run by its `<run_id>`: reopen that run folder, skip
    /// the cases already recorded under `cases/`, and run only what's missing.
    #[arg(long, value_name = "RUN_ID", conflicts_with = "dry_run")]
    resume: Option<String>,
    /// Max cases to run in parallel across all providers.
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
    /// Times a rate-limited case is retried (after backoff) before it's failed.
    #[arg(long, default_value_t = 4)]
    max_retries: u32,
    /// Give up on a case after this many wall-clock seconds: cancel the in-flight
    /// run and record the case failed (a timeout). Applies to every target this
    /// run; set a per-target default in `mira.toml` (`[targets.LABEL].timeout`) or
    /// a preset (`[presets.NAME].timeout`). Unset ⇒ no time limit.
    #[arg(long, value_name = "SECONDS")]
    timeout: Option<u64>,
    /// Execute subjects only (no scoring), writing full transcripts to
    /// --artifacts for later `mira score`. For long-running subjects. No scores
    /// are produced, so no run folder is saved (use `mira score` to save one).
    #[arg(long, requires = "artifacts", conflicts_with_all = ["out", "resume"])]
    execute_only: bool,
    /// Directory for full-transcript execution artifacts (one JSON per case).
    /// Written when --execute-only; read by `mira score`.
    #[arg(long)]
    artifacts: Option<String>,
    /// After the run, publish results to a hosted viewer. Currently: `everruns`.
    /// Requires a saved run (incompatible with --dry-run / --execute-only).
    #[arg(long, value_name = "TARGET", conflicts_with_all = ["dry_run", "execute_only"])]
    publish: Option<String>,
    #[command(flatten)]
    publish_conn: PublishConn,
}

#[derive(Args)]
struct ScoreArgs {
    /// Substring filter on the case key `eval/sample@target`.
    filter: Option<String>,
    /// Directory of execution artifacts written by `run --execute-only`.
    #[arg(long)]
    artifacts: String,
    /// Break resolve-rate down by a metadata key (see `run --group-by`).
    #[arg(long)]
    group_by: Option<String>,
    /// Also write a standalone report file here (see --format). Independent of the
    /// saved run folder, which is always written unless --dry-run.
    #[arg(long)]
    out: Option<String>,
    /// Don't save a run folder (ephemeral). By default the scored results are
    /// saved as a run under the results dir, like `mira run`.
    #[arg(long)]
    dry_run: bool,
    /// Report file format for --out: json | jsonl | csv | junit | md | html.
    #[arg(long, default_value = "json")]
    format: String,
}

/// `mira report <run_id>`: re-render a saved run's reports from its stored
/// per-case results — no study process, no re-execution.
#[derive(Args)]
struct ReportArgs {
    /// The `<run_id>` of a saved run under the results dir.
    run_id: String,
    /// Substring filter on the case key `eval/sample@target`.
    filter: Option<String>,
    /// Break resolve-rate down by a metadata key (see `run --group-by`). Resolved
    /// from axis params and transcript metadata only (no study is consulted).
    #[arg(long)]
    group_by: Option<String>,
    /// Also write a standalone report file here (see --format).
    #[arg(long)]
    out: Option<String>,
    /// Report file format for --out: json | jsonl | csv | junit | md | html.
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
        // `report` re-renders a saved run from disk — no study process needed, so
        // handle it before spawning the host.
        Some(Cmd::Report(args)) => return report(args),
        // `publish` reads a saved run from disk and POSTs it — no study either.
        Some(Cmd::Publish(args)) => return publish_cmd(args).await,
        // `doctor` spawns (and tolerates a failing) study itself, so a broken
        // launcher or study is a finding rather than a hard error here.
        Some(Cmd::Doctor(args)) => {
            doctor::doctor(build_launch_command(&cli.launcher), args.fix).await
        }
        _ => {}
    }

    // One progress bar, shared with the event handler so study `log` lines print
    // cleanly above the bar (via `suspend`) instead of corrupting it. It starts
    // hidden; `run` gives it a length and a draw target once the plan is known.
    let progress = Arc::new(ProgressBar::hidden());
    let progress_evt = progress.clone();
    let command =
        build_launch_command(&cli.launcher).map_err(Box::<dyn std::error::Error>::from)?;
    let host = Host::spawn(command).await?.on_event(move |n| {
        // Per-case `event` notifications correlate to their `run` by
        // `request_id`; here only study `log`s are surfaced, so they no
        // longer spam stderr.
        if let Some(log) = n.as_log() {
            progress_evt.suspend(|| eprintln!("  study: {}", log.message));
        }
    });

    let info = host.initialize("mira-cli").await?;
    eprintln!(
        "study {} · protocol {} · {} evals",
        info.study, info.protocol_version, info.evals
    );
    // `list_complete` pages `list_samples` so the planner sees every sample even
    // when a study paginates a large/lazy dataset; small studies cost no extra
    // round-trips (no cursor ⇒ no follow-up calls).
    let listing = host.list_complete().await?;

    match cli.cmd {
        Some(Cmd::List) => {
            print_listing(&listing);
            host.shutdown().await?;
            Ok(())
        }
        Some(Cmd::Run(args)) => run(host, info, listing, *args, progress).await,
        Some(Cmd::Score(args)) => score(host, info, listing, args).await,
        // Help/report/publish/doctor/no-args returned earlier, before the host
        // spawned.
        None
        | Some(Cmd::Help(_))
        | Some(Cmd::Report(_))
        | Some(Cmd::Publish(_))
        | Some(Cmd::Doctor(_)) => {
            unreachable!("handled before host spawn")
        }
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
  (selection x target matrix x axes), executes each case over the protocol,
  scores the results, then aggregates, reports, and saves the run. Every run is
  saved by default under the results dir as `<run_id>/` (per-case results +
  report + meta), so it can be resumed (`run --resume <run_id>`) and re-rendered
  (`report <run_id>`); `--dry-run` opts out. Execution and scoring can be split
  for long runs (`run --execute-only` then `score`).

  Point it at any study: a single-file Rust study via `--script study.rs`
  (cargo-script frontmatter, shimmed onto stable), a crate via `--bin NAME` /
  `--example NAME`, an arbitrary `--cmd \"...\"`, a non-Rust study via
  `--uv` / `--python` / `--python3 SCRIPT`, or `--package` / `--manifest-path`.
  Save a repo's invocation as `[launchers.NAME]` in mira.toml and select it with
  `--launcher NAME` (or a `default_launcher`).";

    let examples = "\
EXAMPLES
  mira --script study.rs list           # what the study advertises
  mira --script study.rs run            # run the whole matrix
  mira --script study.rs run greet      # selective (substring), like cargo test
  mira --script study.rs run --tag smoke  # only samples carrying a tag
  mira --script study.rs run --targets sim --format junit --out results.xml
  mira --script study.rs run --format html --out report.html  # standalone viewer file
  mira --script study.rs run --dry-run                    # don't save a run folder
  mira --script study.rs run --resume <run_id>            # finish an interrupted run
  mira --script study.rs report <run_id>                  # re-render a saved run
  mira --script study.rs run --execute-only --artifacts art/  # capture transcripts
  mira --script study.rs score --artifacts art/           # score (or re-score) them
  mira --script study.rs doctor         # check config/study/saved runs (--fix repairs)
  mira --bin NAME run                   # drive a crate study (workspace bin)
  mira --python3 study.py run           # drive a non-Rust (polyglot) study
  mira --launcher greet run             # use [launchers.greet] from mira.toml
  mira run                              # use mira.toml's default_launcher";

    // Progressive disclosure of the docs: name + one-line scope per guide, so an
    // agent knows which to open before fetching the tree.
    let width = GUIDES.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut guides = format!("GUIDES ({DOCS_URL})\n");
    for (name, desc) in GUIDES {
        guides.push_str(&format!("  {name:<width$}  {desc}\n"));
    }
    let guides = guides.trim_end();

    let links = format!(
        "\
LINKS
  Repository:   {REPO_URL}
  Issues:       {ISSUES_URL}
  Docs:         {DOCS_URL}
  API docs:     {API_DOCS_URL}
  Agent skill:  {SKILL_URL}  (`mira` — teaches an agent to author/run evals)"
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
    writeln!(out, "{guides}\n")?;
    writeln!(out, "{links}")?;
    Ok(())
}

/// Resolve the effective launcher from the CLI flags overlaid on `mira.toml`,
/// then build the study launch command. Loads `mira.toml` only when it could
/// matter — `--launcher` is given, or no explicit launch mode is set (so a
/// `default_launcher` might apply) — so a plain `mira --script study.rs run` does
/// no config I/O.
fn build_launch_command(cli: &Launcher) -> Result<Command, String> {
    let needs_config = cli.launcher.is_some() || !cli_sets_mode(cli);
    let cfg = if needs_config {
        config::Config::load()
    } else {
        config::Config::default()
    };
    build_command(&resolve_launcher(cli, &cfg)?)
}

/// True when the CLI picked an explicit launch **mode** of its own — any of the
/// mutually-exclusive `cmd`/`script`/`bin`/`example`/`uv`/`python`/`python3` flags.
fn cli_sets_mode(cli: &Launcher) -> bool {
    cli.cmd.is_some()
        || cli.script.is_some()
        || cli.bin.is_some()
        || cli.example.is_some()
        || cli.uv.is_some()
        || cli.python.is_some()
        || cli.python3.is_some()
}

/// Merge a named launcher (`--launcher`, else `default_launcher`) with the
/// explicit launch flags. Flags win, mirroring `--preset`: an explicit launch
/// **mode** (`--cmd`/`--script`/`--bin`/`--example`/`--uv`/`--python`/`--python3`) replaces
/// the named launcher's mode entirely (the modes are mutually exclusive), and
/// `--package`/`--manifest-path` overlay on top.
fn resolve_launcher(
    cli: &Launcher,
    cfg: &config::Config,
) -> Result<config::LauncherConfig, String> {
    // Base: an explicit `--launcher` always loads; otherwise the configured
    // default only applies when the CLI didn't pick a launch mode of its own.
    let mut base = match &cli.launcher {
        Some(name) => cfg.launcher(name)?,
        None if !cli_sets_mode(cli) => match &cfg.default_launcher {
            Some(name) => cfg.launcher(name)?,
            None => config::LauncherConfig::default(),
        },
        None => config::LauncherConfig::default(),
    };

    if cli_sets_mode(cli) {
        base.cmd = cli.cmd.clone();
        base.script = cli.script.clone();
        base.bin = cli.bin.clone();
        base.example = cli.example.clone();
        base.uv = cli.uv.clone();
        base.python = cli.python.clone();
        base.python3 = cli.python3.clone();
    }
    base.package = cli.package.clone().or(base.package);
    base.manifest_path = cli.manifest_path.clone().or(base.manifest_path);
    Ok(base)
}

/// Build the study launch command from a resolved launcher. Fallible because the
/// single-file `--script` mode materializes a crate on disk before it can run.
fn build_command(launcher: &config::LauncherConfig) -> Result<Command, String> {
    if let Some(raw) = &launcher.cmd {
        let mut parts = raw.split_whitespace();
        let program = parts.next().unwrap_or("false");
        let mut command = Command::new(program);
        command.args(parts);
        return Ok(command);
    }

    // Single-file Rust study: `--script study.rs`. cargo-script (`cargo -Zscript`)
    // is nightly-only, so by default we shim it on stable — materialize a crate
    // from the file's frontmatter and `cargo run --manifest-path` it. The same
    // file runs natively under `cargo -Zscript` once it stabilizes; opt in early
    // with MIRA_SCRIPT_NATIVE=1.
    if let Some(script) = &launcher.script {
        let path = script.split_whitespace().next().unwrap_or(script);
        if std::env::var_os("MIRA_SCRIPT_NATIVE").is_some() {
            let mut command = Command::new("cargo");
            command.arg("-Zscript").arg(path);
            return Ok(command);
        }
        let (manifest, target_dir) = materialize_script(Path::new(path))?;
        let mut command = Command::new("cargo");
        command
            .arg("run")
            .arg("-q")
            .arg("--manifest-path")
            .arg(manifest)
            .arg("--target-dir")
            .arg(target_dir);
        return Ok(command);
    }

    // Convenience launchers for non-Rust studies: `--uv`/`--python`/`--python3
    // study.py` instead of the verbose `--cmd "python3 study.py"`. `uv` gets a
    // `run` subcommand; the rest take the script (and any args) directly.
    if let Some(script) = &launcher.uv {
        let mut command = Command::new("uv");
        command.arg("run").args(script.split_whitespace());
        return Ok(command);
    }
    if let Some(script) = &launcher.python {
        let mut command = Command::new("python");
        command.args(script.split_whitespace());
        return Ok(command);
    }
    if let Some(script) = &launcher.python3 {
        let mut command = Command::new("python3");
        command.args(script.split_whitespace());
        return Ok(command);
    }

    let mut command = Command::new("cargo");
    command.arg("run").arg("-q");
    if let Some(pkg) = &launcher.package {
        command.arg("-p").arg(pkg);
    }
    if let Some(bin) = &launcher.bin {
        command.arg("--bin").arg(bin);
    } else if let Some(example) = &launcher.example {
        command.arg("--example").arg(example);
    } else {
        // Default to the bundled `greet` example crate's binary.
        command.arg("--bin").arg("greet");
    }
    if let Some(manifest) = &launcher.manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    Ok(command)
}

/// Turn a single-file cargo-script study into a runnable throwaway crate,
/// returning `(manifest_path, shared_target_dir)` for `cargo run`.
///
/// cargo-script (RFC 3502, `cargo -Zscript`) is nightly-only; this shim gives the
/// same single-file ergonomics on stable. It parses the leading `---` TOML
/// frontmatter (after an optional `#!` shebang), then writes a content-hashed
/// crate under the temp dir: a `Cargo.toml` (frontmatter deps, with relative
/// `path` deps re-anchored to the script's directory, plus a `[[bin]]` and an
/// empty `[workspace]` so it never gets adopted by an enclosing workspace) and a
/// `src/main.rs` holding the script body. A shared `--target-dir` lets the study
/// deps (e.g. `mira-eval`) compile once across scripts. The file format matches
/// native cargo-script, so the same study runs under `cargo -Zscript` unchanged.
fn materialize_script(script: &Path) -> Result<(PathBuf, PathBuf), String> {
    let src = std::fs::read_to_string(script)
        .map_err(|e| format!("cannot read study script {}: {e}", script.display()))?;
    let (manifest_toml, body, skipped_lines) = parse_script(&src)
        .map_err(|e| format!("invalid study script {}: {e}", script.display()))?;

    let script_dir = script
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let script_dir = std::fs::canonicalize(&script_dir)
        .map_err(|e| format!("cannot resolve script directory: {e}"))?;

    let stem = script
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("study");
    let crate_name = sanitize_crate_name(stem);

    // Build the generated manifest, re-anchoring relative path deps.
    let manifest = render_manifest(&manifest_toml, &crate_name, &script_dir)?;

    // Preserve original line numbers in compiler diagnostics: pad the body with
    // one blank line per stripped (shebang/frontmatter) line.
    let main_rs = format!("{}{}", "\n".repeat(skipped_lines), body);

    // Content-hash everything that affects the build so edits invalidate the cache.
    let hash = short_hash(&[&manifest, &main_rs, script_dir.to_string_lossy().as_ref()]);
    let base = std::env::temp_dir().join("mira-script");
    let crate_dir = base.join(format!("{crate_name}-{hash}"));
    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir)
        .map_err(|e| format!("cannot create script cache {}: {e}", src_dir.display()))?;
    write_if_changed(&crate_dir.join("Cargo.toml"), &manifest)?;
    write_if_changed(&src_dir.join("main.rs"), &main_rs)?;

    Ok((crate_dir.join("Cargo.toml"), base.join("target")))
}

/// Split a single-file study into `(frontmatter_toml, body, stripped_line_count)`.
/// Frontmatter is a `---` fenced TOML block (RFC 3502), allowed after an optional
/// `#!` shebang. A file with no frontmatter is valid — it just has no deps.
fn parse_script(src: &str) -> Result<(String, String, usize), String> {
    let mut lines = src.lines().peekable();
    let mut skipped = 0usize;

    // Optional shebang.
    if lines.peek().is_some_and(|l| l.starts_with("#!")) {
        lines.next();
        skipped += 1;
    }
    // Skip blank lines before the fence.
    while lines.peek().is_some_and(|l| l.trim().is_empty()) {
        lines.next();
        skipped += 1;
    }

    if lines.peek().map(|l| l.trim_end()) != Some("---") {
        // No frontmatter: whole (post-shebang) remainder is the body.
        let body = src.lines().skip(skipped).collect::<Vec<_>>().join("\n");
        return Ok((String::new(), body, skipped));
    }
    lines.next();
    skipped += 1;

    let mut manifest = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        skipped += 1;
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        manifest.push_str(line);
        manifest.push('\n');
    }
    if !closed {
        return Err("frontmatter opened with `---` but never closed".into());
    }
    let body = src.lines().skip(skipped).collect::<Vec<_>>().join("\n");
    Ok((manifest, body, skipped))
}

/// Render the generated `Cargo.toml` from the script's frontmatter: fill in
/// `[package]` defaults, re-anchor relative `path` deps to the script dir, and
/// append a `[[bin]]` plus an isolating empty `[workspace]`.
fn render_manifest(
    frontmatter: &str,
    crate_name: &str,
    script_dir: &Path,
) -> Result<String, String> {
    let mut doc: toml::Table =
        toml::from_str(frontmatter).map_err(|e| format!("frontmatter is not valid TOML: {e}"))?;

    // [package] defaults (edition 2024, matching the workspace).
    let pkg = doc
        .entry("package".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if let Some(pkg) = pkg.as_table_mut() {
        pkg.entry("name".to_string())
            .or_insert_with(|| toml::Value::String(crate_name.to_string()));
        pkg.entry("version".to_string())
            .or_insert_with(|| toml::Value::String("0.0.0".to_string()));
        pkg.entry("edition".to_string())
            .or_insert_with(|| toml::Value::String("2024".to_string()));
        pkg.insert("publish".to_string(), toml::Value::Boolean(false));
    }

    // Re-anchor relative `path` deps against the script's directory so the
    // generated crate (which lives in a temp dir) still resolves them.
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get_mut(key).and_then(|v| v.as_table_mut()) {
            for (_name, dep) in table.iter_mut() {
                if let Some(dep) = dep.as_table_mut()
                    && let Some(toml::Value::String(p)) = dep.get("path").cloned()
                {
                    let anchored = script_dir.join(&p);
                    let abs = std::fs::canonicalize(&anchored).unwrap_or(anchored);
                    dep.insert(
                        "path".to_string(),
                        toml::Value::String(abs.to_string_lossy().into_owned()),
                    );
                }
            }
        }
    }

    let mut manifest =
        toml::to_string(&doc).map_err(|e| format!("cannot serialize generated manifest: {e}"))?;
    // A [[bin]] pointing at our src/main.rs, and an empty [workspace] so an
    // enclosing workspace (e.g. the repo's) never tries to adopt this crate.
    manifest.push_str(&format!(
        "\n[[bin]]\nname = \"{crate_name}\"\npath = \"src/main.rs\"\n\n[workspace]\n"
    ));
    Ok(manifest)
}

/// Cargo crate names allow only `[A-Za-z0-9_-]`; map anything else to `_`.
fn sanitize_crate_name(stem: &str) -> String {
    let mut name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name.insert_str(0, "study-");
    }
    name
}

/// A short, stable hex digest of the inputs — enough to key the script cache.
fn short_hash(parts: &[&str]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
        0u8.hash(&mut h); // separator so ["a","b"] != ["ab"]
    }
    format!("{:016x}", h.finish())
}

/// Write only when the content differs, so an unchanged script doesn't touch the
/// file mtime and force cargo to rebuild.
fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
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
    let started_now = now_unix();

    // Resolve selection (preset + flags), validate it against the advertised
    // axes/targets, then plan the full grid up front — so the host owns
    // selection/matrix without the study re-running anything.
    let selection = resolve_selection(&args).map_err(Box::<dyn std::error::Error>::from)?;
    validate_selection(&selection, &listing).map_err(Box::<dyn std::error::Error>::from)?;
    let timeouts = resolve_timeouts(&args, &selection, &listing);
    let plan = plan_grid(&listing, &args, &selection, &timeouts);
    if plan.is_empty() {
        eprintln!("no cases matched the selection");
    }

    // Execute-only: run subjects, persist full transcripts, defer scoring.
    if args.execute_only {
        require_capability(&info, capabilities::EXECUTE, "--execute-only")?;
        let dir = args.artifacts.as_ref().expect("clap requires artifacts");
        return execute_only(host, &plan, dir).await;
    }

    // The run folder: save-by-default unless --dry-run. `--resume <run_id>`
    // reopens an existing folder, keeping its id and original start time; a fresh
    // run mints a new id. `(run_id, dir, started_unix)`.
    let run_store: Option<(String, PathBuf, u64)> = if args.dry_run {
        None
    } else {
        let base = config::Config::load().results_dir();
        match &args.resume {
            Some(run_id) => {
                let dir = config::run_dir(&base, run_id);
                let started = config::load_meta(&dir)
                    .map(|m| m.started_unix)
                    .unwrap_or(started_now);
                Some((run_id.clone(), dir, started))
            }
            None => {
                let run_id = new_run_id_at(started_now);
                let dir = config::run_dir(&base, &run_id);
                Some((run_id, dir, started_now))
            }
        }
    };

    // Seed `done` from any cases already recorded in the run folder (resume), then
    // write the header meta + create `cases/` so the run is resumable from its
    // first completed case. Capture environment once; reused at finalize.
    let mut done: BTreeMap<String, RunResult> = BTreeMap::new();
    let environment = collect_environment();
    if let Some((run_id, dir, started)) = &run_store {
        for r in config::load_case_results(dir) {
            done.insert(r.key(), r);
        }
        if args.resume.is_some() {
            let have = plan.iter().filter(|c| done.contains_key(&c.key())).count();
            eprintln!(
                "resuming run {run_id}: {have}/{} case(s) already done",
                plan.len()
            );
        }
        let header = RunMeta {
            format: RUN_META_FORMAT,
            run_id: run_id.clone(),
            study: info.study.clone(),
            study_version: info.study_version.clone(),
            started_unix: *started,
            finished_unix: 0,
            environment: environment.clone(),
            summary: RunSummary::default(),
        };
        config::init_run(dir, &header)?;
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

    // Only run cases not already recorded in the run folder (resume).
    let todo: Vec<CaseSpec> = plan
        .iter()
        .filter(|case| !done.contains_key(&case.key()))
        .cloned()
        .collect();

    let cfg = concurrency(&args);

    // Run cases concurrently under the bounded, provider-aware policy. Each
    // finished case advances the bar and is written to its own
    // `cases/<key>/result.json` as it lands, so a long run stays resumable.
    {
        let handle = host.handle();
        let write_dir = run_store.as_ref().map(|(_, dir, _)| dir.clone());
        exec::run_cases(
            todo,
            &cfg,
            |case| {
                let handle = handle.clone();
                async move {
                    handle
                        .run(
                            &case.eval,
                            &case.sample,
                            &case.target,
                            &case.params,
                            case.trial,
                        )
                        .await
                }
            },
            |case, result| {
                let key = case.key();
                progress.set_message(key.clone());
                if let Some(dir) = &write_dir
                    && let Err(e) = config::write_case_result(dir, &key, &result)
                {
                    progress.suspend(|| eprintln!("warning: failed to write case result: {e}"));
                }
                done.insert(key, result);
                progress.inc(1);
            },
        )
        .await;
    }
    progress.finish_and_clear();
    host.shutdown().await?;

    // Report only the planned cases, in plan order.
    let results: Vec<RunResult> = plan
        .iter()
        .filter_map(|case| done.get(&case.key()).cloned())
        .collect();

    report::print_results(&results);

    // Resolve the --group-by breakdown once; reused by the terminal, file, and
    // saved reports so they all agree.
    let group_vals = args
        .group_by
        .as_deref()
        .map(|key| group_values(&results, key, &listing));
    let group = match (args.group_by.as_deref(), group_vals.as_deref()) {
        (Some(key), Some(values)) => {
            report::print_group_breakdown(&results, key, values);
            Some(report::Group { key, values })
        }
        _ => None,
    };

    if let Some(path) = &args.out {
        std::fs::write(path, report::render_with_group(&results, format, group))?;
        eprintln!("\nwrote {path} ({:?})", format);
    }

    // Finalize the saved run: render its reports and rewrite meta with the end
    // time and summary.
    if let Some((run_id, dir, started)) = &run_store {
        let meta = RunMeta {
            format: RUN_META_FORMAT,
            run_id: run_id.clone(),
            study: info.study.clone(),
            study_version: info.study_version.clone(),
            started_unix: *started,
            finished_unix: now_unix(),
            environment,
            summary: RunSummary::of(&results),
        };
        config::finalize_run(dir, &meta, &results, group)?;
        eprintln!("\nsaved run {run_id} to {}", dir.display());

        // Optional: publish the just-saved run to a hosted viewer.
        if let Some(target) = &args.publish {
            publish_results(target, &args.publish_conn, &meta, &results).await?;
        }
    }

    // A case that's N/A (all scores N/A — e.g. an infra failure) is neither
    // passed nor failed, so it doesn't make CI red.
    let failed = results
        .iter()
        .any(|r| !r.skipped && !report::is_na(r) && !r.passed);
    std::process::exit(if failed { 1 } else { 0 });
}

/// Capture environment context (commit, box, host version, labels) for a saved
/// run's `meta.json`, unless disabled in `mira.toml`. Best-effort: never fails the
/// run.
fn collect_environment() -> Option<mira::run::Environment> {
    let cfg = config::Config::load();
    cfg.environment
        .enabled
        .then(|| env::collect(&cfg.environment.labels))
        .flatten()
}

/// `mira publish <run_id>`: load a saved run from disk and publish its results
/// to a hosted viewer. No study process or re-execution — it reads `meta.json`
/// and `cases/*/result.json`, like `report`.
async fn publish_cmd(args: &PublishArgs) -> Result<(), Box<dyn std::error::Error>> {
    let base = config::Config::load().results_dir();
    let dir = config::run_dir(&base, &args.run_id);
    let meta = config::load_meta(&dir).ok_or_else(|| {
        format!(
            "no saved run '{}' found under {}",
            args.run_id,
            dir.display()
        )
    })?;
    let mut results = config::load_case_results(&dir);
    if let Some(f) = &args.filter {
        results.retain(|r| r.key().contains(f.as_str()));
    }
    if results.is_empty() {
        return Err(format!("no case results to publish for run {}", args.run_id).into());
    }
    publish_results(&args.to, &args.conn, &meta, &results).await
}

/// Route a publish target name to its sink. Shared by `publish` and
/// `run --publish`.
async fn publish_results(
    target: &str,
    conn: &PublishConn,
    meta: &RunMeta,
    results: &[RunResult],
) -> Result<(), Box<dyn std::error::Error>> {
    if target != "everruns" {
        return Err(
            format!("unknown publish target '{target}': only 'everruns' is supported").into(),
        );
    }
    let outcome = mira_publish_everruns::publish(meta, results, &conn.to_options()).await?;
    eprintln!(
        "published {} eval(s), {} case(s) to everruns",
        outcome.evals, outcome.cases
    );
    for id in &outcome.run_ids {
        eprintln!("  run {id}");
    }
    Ok(())
}

/// `mira report <run_id>`: re-render a saved run's reports from its stored
/// per-case results — no study process, no re-execution. Group-by resolves only
/// from axis params and transcript metadata here (sample/target metadata would
/// need the study). Refreshes the run's `report.json`/`report.html` in place when
/// the full (unfiltered) set is rendered.
fn report(args: &ReportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let format = Format::from_str(&args.format)?;
    let base = config::Config::load().results_dir();
    let dir = config::run_dir(&base, &args.run_id);
    let mut results = config::load_case_results(&dir);
    if let Some(f) = &args.filter {
        results.retain(|r| r.key().contains(f.as_str()));
    }
    if results.is_empty() {
        eprintln!(
            "no case results for run {} (looked in {}/cases)",
            args.run_id,
            dir.display()
        );
    }

    report::print_results(&results);

    // No study process here, so group-by sees an empty listing.
    let empty = ListResult { evals: Vec::new() };
    let group_vals = args
        .group_by
        .as_deref()
        .map(|key| group_values(&results, key, &empty));
    let group = match (args.group_by.as_deref(), group_vals.as_deref()) {
        (Some(key), Some(values)) => {
            report::print_group_breakdown(&results, key, values);
            Some(report::Group { key, values })
        }
        _ => None,
    };

    if let Some(path) = &args.out {
        std::fs::write(path, report::render_with_group(&results, format, group))?;
        eprintln!("\nwrote {path} ({:?})", format);
    }

    // Refresh the saved run's reports in place when rendering the full set, so the
    // on-disk viewer reflects the latest render (e.g. a new --group-by view).
    if args.filter.is_none()
        && let Some(mut meta) = config::load_meta(&dir)
    {
        meta.summary = RunSummary::of(&results);
        config::finalize_run(&dir, &meta, &results, group)?;
    }

    Ok(())
}

/// Resolve per-target wall-clock timeouts from the CLI flag, `mira.toml`
/// `[targets.LABEL].timeout`, and the preset default (see [`Timeouts`]). Warns
/// (doesn't fail) on a `[targets.LABEL]` whose label no study target matches, so
/// a typo'd label is visible rather than silently inert — without breaking a
/// `mira.toml` shared across studies with different targets.
fn resolve_timeouts(args: &RunArgs, sel: &Selection, listing: &ListResult) -> Timeouts {
    use std::collections::BTreeSet;
    let cfg = config::Config::load();
    let known: BTreeSet<&str> = listing
        .evals
        .iter()
        .flat_map(|e| e.targets.iter().map(|t| t.label.as_str()))
        .collect();
    let mut per_target = BTreeMap::new();
    for (label, tcfg) in &cfg.targets {
        let Some(secs) = tcfg.timeout else { continue };
        if !known.contains(label.as_str()) {
            eprintln!(
                "warning: mira.toml [targets.{label:?}] timeout set, but no target {label:?} is declared"
            );
        }
        per_target.insert(label.clone(), secs);
    }
    Timeouts {
        cli: args.timeout,
        preset: sel.timeout,
        per_target,
    }
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
/// it — so a study lacking the optional capability fails fast with a clear
/// message rather than a generic RPC error mid-run.
fn require_capability(
    info: &InitializeResult,
    cap: &str,
    feature: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if info.capabilities.iter().any(|c| c == cap) {
        return Ok(());
    }
    Err(format!(
        "study {} doesn't support {feature}: it doesn't advertise the `{cap}` capability",
        info.study
    )
    .into())
}

/// `run --execute-only`: run each case's subject, persist the full transcript as
/// an artifact, and skip scoring. Resumable — a case whose artifact already
/// exists is skipped (delete the artifacts dir to force a fresh run).
async fn execute_only(
    host: Host,
    plan: &[CaseSpec],
    dir: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let mut wrote = 0usize;
    for case in plan {
        let path = artifact_path(dir, &case.key());
        if path.exists() {
            continue;
        }
        let result = host
            .execute(
                &case.eval,
                &case.sample,
                &case.target,
                &case.params,
                case.trial,
            )
            .await?;
        std::fs::write(&path, serde_json::to_string_pretty(&result)?)?;
        wrote += 1;
    }
    host.shutdown().await?;
    eprintln!("executed {wrote} case(s); artifacts in {dir}");
    eprintln!("score them with: mira score --artifacts {dir}");
    Ok(())
}

/// `score`: load execution artifacts, (re-)score each via the study, and report.
/// Re-running this over the same artifacts is a re-score (e.g. after a scorer
/// change) — no subject is re-executed.
async fn score(
    host: Host,
    info: InitializeResult,
    listing: ListResult,
    args: ScoreArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    require_capability(&info, capabilities::SCORE, "mira score")?;
    let format = Format::from_str(&args.format)?;
    let started_unix = now_unix();
    let mut artifacts = load_artifacts(&args.artifacts);
    if let Some(f) = &args.filter {
        artifacts.retain(|a| a.key().contains(f.as_str()));
    }
    if artifacts.is_empty() {
        eprintln!("no artifacts in {}", args.artifacts);
    }

    let mut results = Vec::with_capacity(artifacts.len());
    for artifact in &artifacts {
        // A skipped (unexecuted) case has no transcript to score; pass it through.
        if artifact.skipped {
            results.push(skipped_result(artifact));
        } else {
            results.push(host.score(artifact).await?);
        }
    }
    host.shutdown().await?;

    report::print_results(&results);

    // Resolve the --group-by breakdown once; reused by the terminal, file, and
    // saved reports so they all agree.
    let group_vals = args
        .group_by
        .as_deref()
        .map(|key| group_values(&results, key, &listing));
    let group = match (args.group_by.as_deref(), group_vals.as_deref()) {
        (Some(key), Some(values)) => {
            report::print_group_breakdown(&results, key, values);
            Some(report::Group { key, values })
        }
        _ => None,
    };

    if let Some(path) = &args.out {
        std::fs::write(path, report::render_with_group(&results, format, group))?;
        eprintln!("\nwrote {path} ({:?})", format);
    }

    // Save the scored results as a run folder (save-by-default; --dry-run opts
    // out): write each case's result.json, then the rendered reports + meta.
    if !args.dry_run {
        let base = config::Config::load().results_dir();
        let run_id = new_run_id_at(started_unix);
        let dir = config::run_dir(&base, &run_id);
        for r in &results {
            config::write_case_result(&dir, &r.key(), r)?;
        }
        let meta = RunMeta {
            format: RUN_META_FORMAT,
            run_id: run_id.clone(),
            study: info.study.clone(),
            study_version: info.study_version.clone(),
            started_unix,
            finished_unix: now_unix(),
            environment: collect_environment(),
            summary: RunSummary::of(&results),
        };
        config::finalize_run(&dir, &meta, &results, group)?;
        eprintln!("\nsaved run {run_id} to {}", dir.display());
    }

    // A case that's N/A (all scores N/A — e.g. an infra failure) is neither
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
        target: a.target.clone(),
        params: a.params.clone(),
        trial: a.trial,
        trials: a.trials,
        seed: a.seed,
        input: Vec::new(),
        expected: None,
        passed: false,
        aggregate: 0.0,
        scores: Vec::new(),
        transcript: TranscriptSummary::of(&a.transcript),
        skipped: true,
    }
}

/// Filesystem path for a case's artifact under `dir`, using the shared reversible
/// key encoding (see [`config::encode_key`]) so distinct keys can never collide
/// onto the same filename (which would overwrite an artifact or wrongly skip
/// execution on resume).
fn artifact_path(dir: &str, key: &str) -> std::path::PathBuf {
    Path::new(dir).join(format!("{}.json", config::encode_key(key)))
}

/// Load every execution artifact in `dir`, sorted by case key for stable order.
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

/// The resolved selection for a run: a host-side *subset* of the declared grid.
/// Built from `--preset` (if any) and explicit flags (flags win). `axes` keys an
/// axis name (`target` for the primary axis, or any declared axis) to the values
/// kept; `evals` restricts which evals (hence subjects) run.
struct Selection {
    /// Cross-cutting substring on the whole case key (`cargo test PAT`-style),
    /// from the positional arg. Orthogonal to the glob dimension selectors.
    filter: Option<String>,
    tag: Option<String>,
    /// `None` = every eval; `Some` = only those whose name matches a glob here.
    evals: Option<Vec<String>>,
    /// `None` = every sample; `Some` = only those whose id matches a glob here.
    samples: Option<Vec<String>>,
    /// Axis name → allowed value globs. `target` is the primary axis. An axis
    /// absent here is unconstrained.
    axes: BTreeMap<String, Vec<String>>,
    /// Default per-case wall-clock timeout (seconds) from the preset, if any.
    /// Lowest-priority source — see [`Timeouts`].
    timeout: Option<u64>,
}

/// Resolves a per-case wall-clock timeout for each target. Precedence, first set
/// wins: `--timeout` (CLI, all targets) > `mira.toml` `[targets.LABEL].timeout`
/// (per-target) > preset `timeout`. `None` ⇒ no time limit for that target.
///
/// The CLI flag wins over saved config (mirroring how explicit flags override a
/// preset elsewhere); among saved config the more specific per-target setting
/// beats the preset default.
#[derive(Default)]
struct Timeouts {
    cli: Option<u64>,
    preset: Option<u64>,
    per_target: BTreeMap<String, u64>,
}

impl Timeouts {
    fn for_target(&self, label: &str) -> Option<Duration> {
        self.cli
            .or_else(|| self.per_target.get(label).copied())
            .or(self.preset)
            .map(Duration::from_secs)
    }
}

/// Split a comma-separated value list, trimming and dropping empties.
fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Resolve the selection from a `--preset` (loaded from `mira.toml`) overlaid by
/// explicit flags. `--targets` folds into the `target` axis; `--axis NAME=v,v`
/// (repeatable) sets/overrides any axis.
fn resolve_selection(args: &RunArgs) -> Result<Selection, String> {
    let preset = match &args.preset {
        Some(name) => config::Config::load().preset(name)?,
        None => config::Preset::default(),
    };

    // Positional arg is the cross-cutting grep; presets no longer carry it.
    let filter = args.filter.clone();
    let tag = args.tag.clone().or(preset.tag);
    // Dimension selectors: an explicit flag (comma-separated) overrides the
    // preset's list; absent both ⇒ unconstrained.
    let evals = match &args.evals {
        Some(s) => Some(split_csv(s)),
        None => (!preset.evals.is_empty()).then(|| preset.evals.clone()),
    };
    let samples = match &args.samples {
        Some(s) => Some(split_csv(s)),
        None => (!preset.samples.is_empty()).then(|| preset.samples.clone()),
    };

    // Start from the preset's axis constraints, then layer flags on top.
    let mut axes: BTreeMap<String, Vec<String>> = preset.axes.clone();

    // Primary axis: --targets wins over the preset's targets.
    let targets = match &args.targets {
        Some(s) => split_csv(s),
        None => preset.targets.clone(),
    };
    if !targets.is_empty() {
        axes.insert("target".to_string(), targets);
    }

    // --axis NAME=V1,V2 (repeatable) overrides the same-named axis.
    for spec in &args.axes {
        let (name, vals) = spec
            .split_once('=')
            .ok_or_else(|| format!("--axis expects NAME=V1,V2, got {spec:?}"))?;
        let values = split_csv(vals);
        if values.is_empty() {
            return Err(format!("--axis {name}= lists no values"));
        }
        axes.insert(name.trim().to_string(), values);
    }

    Ok(Selection {
        filter,
        tag,
        evals,
        samples,
        axes,
        timeout: preset.timeout,
    })
}

/// Reject a selection that names an axis/value/eval the study didn't advertise,
/// so a typo fails loudly instead of silently matching nothing.
fn validate_selection(sel: &Selection, listing: &ListResult) -> Result<(), String> {
    use std::collections::BTreeSet;
    // Declared axis values across all evals: `target` → all target labels; each
    // declared axis → the union of its values.
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for eval in &listing.evals {
        let t = declared.entry("target".to_string()).or_default();
        for m in &eval.targets {
            t.insert(m.label.clone());
        }
        for a in &eval.axes {
            let e = declared.entry(a.name.clone()).or_default();
            for v in &a.values {
                e.insert(v.clone());
            }
        }
    }
    let listed = |set: &BTreeSet<String>| {
        let mut v: Vec<&str> = set.iter().map(String::as_str).collect();
        v.sort_unstable();
        v.join(", ")
    };
    for (name, allowed) in &sel.axes {
        let Some(valid) = declared.get(name) else {
            let names: BTreeSet<String> = declared.keys().cloned().collect();
            return Err(format!(
                "unknown axis {name:?} (declared: {})",
                listed(&names)
            ));
        };
        for v in allowed {
            if !valid.iter().any(|d| mira::glob_match(v, d)) {
                return Err(format!(
                    "axis {name:?} has no value matching {v:?} (declared: {})",
                    listed(valid)
                ));
            }
        }
    }
    if let Some(evals) = &sel.evals {
        let known: BTreeSet<String> = listing.evals.iter().map(|e| e.name.clone()).collect();
        for e in evals {
            if !known.iter().any(|k| mira::glob_match(e, k)) {
                return Err(format!(
                    "no eval matching {e:?} (declared: {})",
                    listed(&known)
                ));
            }
        }
    }
    // `samples` aren't pre-validated: the listing may be a paginated first page,
    // so a valid id could be absent here. A glob that matches nothing simply
    // selects no cases (the run reports an empty plan).
    Ok(())
}

/// True when a case's axis params satisfy the (non-target) axis constraints.
fn axes_allowed(sel: &Selection, params: &mira::Params) -> bool {
    params.iter().all(|(name, value)| {
        sel.axes
            .get(name)
            .is_none_or(|allowed| allowed.iter().any(|p| mira::glob_match(p, value)))
    })
}

/// Expand the advertised listing into an ordered, selected list of cases. Each
/// case carries its target's provider so the executor can bucket concurrency.
fn plan_grid(
    listing: &ListResult,
    args: &RunArgs,
    sel: &Selection,
    timeouts: &Timeouts,
) -> Vec<CaseSpec> {
    let mut plan = Vec::new();
    for eval in &listing.evals {
        if let Some(evals) = &sel.evals
            && !evals.iter().any(|p| mira::glob_match(p, &eval.name))
        {
            continue;
        }
        let combos = axis_combinations(eval);
        // Trials: --trials overrides the eval's declared count (0/1 → single).
        // Seed base: --seed overrides the eval's declared seed; trial t uses
        // `base + t` so the repetition set replays deterministically.
        let trials = args.trials.unwrap_or(eval.trials).max(1);
        let seed_base = args.seed.or(eval.seed);
        for sample in &eval.samples {
            if let Some(tag) = &sel.tag
                && !sample.tags.contains(tag)
            {
                continue;
            }
            if let Some(allow) = &sel.samples
                && !allow.iter().any(|p| mira::glob_match(p, &sample.id))
            {
                continue;
            }
            for target in &eval.targets {
                if let Some(allow) = sel.axes.get("target")
                    && !allow.iter().any(|p| mira::glob_match(p, &target.label))
                {
                    continue;
                }
                for params in &combos {
                    if !axes_allowed(sel, params) {
                        continue;
                    }
                    let key = mira::case_key(&eval.name, &sample.id, &target.label, params);
                    // Filter on the logical key, so `--filter` keeps or drops all
                    // trials of a case together (a stable group to aggregate).
                    if let Some(f) = &sel.filter
                        && !key.contains(f.as_str())
                    {
                        continue;
                    }
                    let timeout = timeouts.for_target(&target.label);
                    for index in 0..trials {
                        plan.push(CaseSpec {
                            eval: eval.name.clone(),
                            sample: sample.id.clone(),
                            target: target.label.clone(),
                            provider: target.provider.clone(),
                            params: params.clone(),
                            timeout,
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

/// Per-(eval,sample) and per-(eval,target) metadata, indexed from the listing so
/// `--group-by` can resolve a key against the sample and target columns.
type MetaIndex = BTreeMap<(String, String), mira::Metadata>;

fn meta_indexes(listing: &ListResult) -> (MetaIndex, MetaIndex) {
    let mut sample_meta = MetaIndex::new();
    let mut model_meta = MetaIndex::new();
    for eval in &listing.evals {
        for s in &eval.samples {
            if !s.metadata.is_empty() {
                sample_meta.insert((eval.name.clone(), s.id.clone()), s.metadata.clone());
            }
        }
        for m in &eval.targets {
            if !m.metadata.is_empty() {
                model_meta.insert((eval.name.clone(), m.label.clone()), m.metadata.clone());
            }
        }
    }
    (sample_meta, model_meta)
}

/// Resolve a `--group-by` key for one case, in priority order: axis params,
/// sample metadata, target metadata, then transcript metadata. `None` ⇒ the case
/// carried no value for the key.
fn group_value(
    r: &RunResult,
    key: &str,
    sample_meta: &MetaIndex,
    model_meta: &MetaIndex,
) -> Option<String> {
    if let Some(v) = r.params.get(key) {
        return Some(v.clone());
    }
    if let Some(v) = sample_meta
        .get(&(r.eval.clone(), r.sample.clone()))
        .and_then(|m| m.get(key))
    {
        return Some(mira::metadata_display(v));
    }
    if let Some(v) = model_meta
        .get(&(r.eval.clone(), r.target.clone()))
        .and_then(|m| m.get(key))
    {
        return Some(mira::metadata_display(v));
    }
    r.transcript.metadata.get(key).map(mira::metadata_display)
}

/// Resolve `--group-by` values for every result (parallel to `results`).
fn group_values(results: &[RunResult], key: &str, listing: &ListResult) -> Vec<Option<String>> {
    let (sample_meta, model_meta) = meta_indexes(listing);
    results
        .iter()
        .map(|r| group_value(r, key, &sample_meta, &model_meta))
        .collect()
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
                .map(|s| {
                    let mut label = s.id.clone();
                    if !s.tags.is_empty() {
                        label.push_str(&format!(" [{}]", s.tags.join(",")));
                    }
                    if !s.metadata.is_empty() {
                        label.push_str(&format!(" {{{}}}", fmt_meta(&s.metadata)));
                    }
                    label
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  scorers: {}", eval.scorers.join(", "));
        println!(
            "  targets:  {}",
            eval.targets
                .iter()
                .map(|m| {
                    let mut label = m.label.clone();
                    if !m.available {
                        label.push_str(" (unavailable)");
                    }
                    if !m.metadata.is_empty() {
                        label.push_str(&format!(" {{{}}}", fmt_meta(&m.metadata)));
                    }
                    label
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
            println!("  meta:    {}", fmt_meta(&eval.metadata));
        }
    }
}

/// Render a metadata map as `k=v, …` for the listing (values via the shared
/// `metadata_display`, so a JSON string shows raw and anything else compact JSON).
fn fmt_meta(meta: &mira::Metadata) -> String {
    meta.iter()
        .map(|(k, v)| format!("{k}={}", mira::metadata_display(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An all-empty CLI launcher (no flags), the base for these cases.
    fn empty_launcher() -> Launcher {
        Launcher {
            launcher: None,
            bin: None,
            example: None,
            cmd: None,
            script: None,
            uv: None,
            python: None,
            python3: None,
            package: None,
            manifest_path: None,
        }
    }

    fn cfg(text: &str) -> config::Config {
        config::Config::parse(text).unwrap()
    }

    /// A default `RunArgs` (no selection); tests tweak the fields they exercise.
    fn run_args() -> RunArgs {
        RunArgs {
            filter: None,
            tag: None,
            targets: None,
            samples: None,
            evals: None,
            axes: Vec::new(),
            preset: None,
            trials: None,
            seed: None,
            group_by: None,
            out: None,
            format: "json".into(),
            dry_run: false,
            resume: None,
            max_concurrent: 8,
            provider_concurrency: None,
            no_adaptive: false,
            max_retries: 4,
            execute_only: false,
            artifacts: None,
            timeout: None,
            publish: None,
            publish_conn: PublishConn::default(),
        }
    }

    fn sample(id: &str) -> mira::protocol::SampleInfo {
        mira::protocol::SampleInfo {
            id: id.into(),
            tags: Vec::new(),
            metadata: Default::default(),
        }
    }

    fn target(label: &str) -> mira::protocol::TargetInfo {
        mira::protocol::TargetInfo {
            label: label.into(),
            provider: "sim".into(),
            available: true,
            metadata: Default::default(),
        }
    }

    /// Two evals, each with samples `france`/`spain` over targets `sim` and
    /// `anthropic/opus` — enough to exercise every glob dimension.
    fn listing() -> ListResult {
        let eval = |name: &str| mira::protocol::EvalInfo {
            name: name.into(),
            description: String::new(),
            samples: vec![sample("france"), sample("spain")],
            next_cursor: None,
            scorers: vec!["contains".into()],
            targets: vec![target("sim"), target("anthropic/opus")],
            axes: Vec::new(),
            max_turns: 0,
            trials: 0,
            seed: None,
            metadata: Default::default(),
        };
        ListResult {
            evals: vec![eval("greet"), eval("coding")],
        }
    }

    /// The distinct `eval/sample@target` keys a plan covers (trials collapsed).
    fn keys(plan: &[CaseSpec]) -> std::collections::BTreeSet<String> {
        plan.iter()
            .map(|c| format!("{}/{}@{}", c.eval, c.sample, c.target))
            .collect()
    }

    #[test]
    fn glob_targets_select_by_pattern() {
        let args = RunArgs {
            targets: Some("anthropic/*".into()),
            ..run_args()
        };
        let sel = resolve_selection(&args).unwrap();
        let plan = plan_grid(&listing(), &args, &sel, &Timeouts::default());
        assert!(keys(&plan).iter().all(|k| k.ends_with("@anthropic/opus")));
        assert_eq!(plan.len(), 4); // 2 evals × 2 samples × 1 target
    }

    #[test]
    fn glob_samples_and_evals_narrow_the_grid() {
        let args = RunArgs {
            samples: Some("s*".into()), // matches `spain`, not `france`
            evals: Some("greet".into()),
            ..run_args()
        };
        let sel = resolve_selection(&args).unwrap();
        assert_eq!(
            keys(&plan_grid(&listing(), &args, &sel, &Timeouts::default())),
            ["greet/spain@anthropic/opus", "greet/spain@sim"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn preset_samples_apply_and_flags_override() {
        let cfg = cfg("[presets.fr]\nsamples = \"france\"\n");
        let preset = cfg.preset("fr").unwrap();
        assert_eq!(preset.samples, vec!["france"]);

        // A glob that matches no declared target still validates loudly.
        let args = RunArgs {
            targets: Some("openai/*".into()),
            ..run_args()
        };
        let sel = resolve_selection(&args).unwrap();
        let err = validate_selection(&sel, &listing()).unwrap_err();
        assert!(err.contains("no value matching"), "{err}");
    }

    fn parts(cmd: &Command) -> (String, Vec<String>) {
        let std = cmd.as_std();
        let program = std.get_program().to_string_lossy().into_owned();
        let args = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        (program, args)
    }

    // Drift guard: the help guide list mirrors docs/README.md. If a doc is added,
    // renamed, or removed there without updating GUIDES (or vice versa), this fails.
    #[test]
    fn guides_match_docs_readme() {
        let readme = include_str!("../../../docs/README.md");
        for (name, _) in GUIDES {
            assert!(
                readme.contains(&format!("]({name}.md)")),
                "GUIDES entry `{name}` has no matching link in docs/README.md"
            );
        }
    }

    #[test]
    fn root_readme_diagram_urls_render_off_repo() {
        let readme = include_str!("../../../README.md");
        assert!(
            !readme.contains("src=\"docs/assets/"),
            "root README diagrams need absolute URLs so crates.io and other off-repo renderers can load them"
        );
    }

    #[test]
    fn timeout_precedence_cli_then_per_target_then_preset() {
        let per_target = BTreeMap::from([("anthropic/opus".to_string(), 300u64)]);
        // CLI wins over everything, for every target.
        let t = Timeouts {
            cli: Some(10),
            preset: Some(120),
            per_target: per_target.clone(),
        };
        assert_eq!(
            t.for_target("anthropic/opus"),
            Some(Duration::from_secs(10))
        );
        assert_eq!(t.for_target("sim"), Some(Duration::from_secs(10)));

        // No CLI: per-target beats the preset; a target without its own setting
        // falls back to the preset default.
        let t = Timeouts {
            cli: None,
            preset: Some(120),
            per_target,
        };
        assert_eq!(
            t.for_target("anthropic/opus"),
            Some(Duration::from_secs(300))
        );
        assert_eq!(t.for_target("sim"), Some(Duration::from_secs(120)));

        // Nothing set anywhere ⇒ no timeout.
        let t = Timeouts {
            cli: None,
            preset: None,
            per_target: BTreeMap::new(),
        };
        assert_eq!(t.for_target("sim"), None);
    }

    #[test]
    fn uv_launcher_prepends_run() {
        let l = config::LauncherConfig {
            uv: Some("study.py".into()),
            ..Default::default()
        };
        let (program, args) = parts(&build_command(&l).unwrap());
        assert_eq!(program, "uv");
        assert_eq!(args, ["run", "study.py"]);
    }

    #[test]
    fn python_launchers_take_script_directly() {
        let l = config::LauncherConfig {
            python: Some("study.py --flag".into()),
            ..Default::default()
        };
        let (program, args) = parts(&build_command(&l).unwrap());
        assert_eq!(program, "python");
        assert_eq!(args, ["study.py", "--flag"]);

        let l = config::LauncherConfig {
            python3: Some("examples/greet-python/study.py".into()),
            ..Default::default()
        };
        let (program, args) = parts(&build_command(&l).unwrap());
        assert_eq!(program, "python3");
        assert_eq!(args, ["examples/greet-python/study.py"]);
    }

    #[test]
    fn cmd_wins_over_python_launchers() {
        let l = config::LauncherConfig {
            cmd: Some("echo hi".into()),
            python3: Some("study.py".into()),
            ..Default::default()
        };
        let (program, args) = parts(&build_command(&l).unwrap());
        assert_eq!(program, "echo");
        assert_eq!(args, ["hi"]);
    }

    #[test]
    fn defaults_to_greet_bin() {
        let (program, args) = parts(&build_command(&config::LauncherConfig::default()).unwrap());
        assert_eq!(program, "cargo");
        assert_eq!(args, ["run", "-q", "--bin", "greet"]);
    }

    /// A polyglot mode in a named launcher resolves and overrides the default.
    #[test]
    fn named_launcher_supports_polyglot_modes() {
        let cfg = cfg("default_launcher = \"py\"\n[launchers.py]\npython3 = \"study.py\"\n");
        let l = resolve_launcher(&empty_launcher(), &cfg).unwrap();
        assert_eq!(l.python3.as_deref(), Some("study.py"));
        let (program, args) = parts(&build_command(&l).unwrap());
        assert_eq!(program, "python3");
        assert_eq!(args, ["study.py"]);
    }

    #[test]
    fn named_launcher_resolves_from_config() {
        let cfg = cfg("[launchers.greet]\nbin = \"greet\"\npackage = \"myapp\"\n");
        let cli = Launcher {
            launcher: Some("greet".into()),
            ..empty_launcher()
        };
        let l = resolve_launcher(&cli, &cfg).unwrap();
        assert_eq!(l.bin.as_deref(), Some("greet"));
        assert_eq!(l.package.as_deref(), Some("myapp"));
    }

    #[test]
    fn default_launcher_applies_when_no_flag() {
        let cfg = cfg("default_launcher = \"py\"\n[launchers.py]\ncmd = \"python s.py\"\n");
        let l = resolve_launcher(&empty_launcher(), &cfg).unwrap();
        assert_eq!(l.cmd.as_deref(), Some("python s.py"));
    }

    #[test]
    fn explicit_mode_overrides_named_launcher() {
        // --launcher picks `py` (a cmd launcher), but --bin replaces the mode.
        let cfg = cfg("[launchers.py]\ncmd = \"python s.py\"\n");
        let cli = Launcher {
            launcher: Some("py".into()),
            bin: Some("other".into()),
            ..empty_launcher()
        };
        let l = resolve_launcher(&cli, &cfg).unwrap();
        assert_eq!(l.bin.as_deref(), Some("other"));
        assert!(
            l.cmd.is_none(),
            "explicit --bin must drop the named cmd mode"
        );
    }

    #[test]
    fn explicit_mode_suppresses_default_launcher() {
        // A CLI launch mode means the configured default is ignored entirely.
        let cfg = cfg("default_launcher = \"py\"\n[launchers.py]\ncmd = \"python s.py\"\n");
        let cli = Launcher {
            bin: Some("greet".into()),
            ..empty_launcher()
        };
        let l = resolve_launcher(&cli, &cfg).unwrap();
        assert_eq!(l.bin.as_deref(), Some("greet"));
        assert!(l.cmd.is_none());
    }

    #[test]
    fn package_overlays_named_launcher() {
        // --package modifies a named bin launcher without replacing its mode.
        let cfg = cfg("[launchers.greet]\nbin = \"greet\"\npackage = \"a\"\n");
        let cli = Launcher {
            launcher: Some("greet".into()),
            package: Some("b".into()),
            ..empty_launcher()
        };
        let l = resolve_launcher(&cli, &cfg).unwrap();
        assert_eq!(l.bin.as_deref(), Some("greet"));
        assert_eq!(l.package.as_deref(), Some("b"));
    }

    #[test]
    fn unknown_launcher_errors() {
        let cfg = cfg("[launchers.greet]\nbin = \"greet\"\n");
        let cli = Launcher {
            launcher: Some("nope".into()),
            ..empty_launcher()
        };
        assert!(resolve_launcher(&cli, &cfg).is_err());
    }

    #[test]
    fn bare_cli_with_no_config_defaults_to_greet() {
        // No flags, no config → the bundled greet bin (preserved behaviour).
        let l = resolve_launcher(&empty_launcher(), &config::Config::default()).unwrap();
        assert!(l.bin.is_none() && l.cmd.is_none() && l.example.is_none());
        // build_command fills in the greet default.
        let cmd = build_command(&l).unwrap();
        let argv: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["run", "-q", "--bin", "greet"]);
    }

    #[test]
    fn parse_script_strips_shebang_and_frontmatter() {
        let src = "#!/usr/bin/env -S cargo -Zscript\n\
                   ---\n\
                   [dependencies]\n\
                   foo = \"1\"\n\
                   ---\n\
                   fn main() {}\n";
        let (manifest, body, skipped) = parse_script(src).unwrap();
        assert_eq!(manifest, "[dependencies]\nfoo = \"1\"\n");
        assert_eq!(body, "fn main() {}");
        // shebang + `---` + 2 dep lines + `---` = 5 stripped lines, so the body's
        // line number is preserved when we pad with that many newlines.
        assert_eq!(skipped, 5);
    }

    #[test]
    fn parse_script_allows_no_frontmatter() {
        let (manifest, body, skipped) = parse_script("fn main() {}\n").unwrap();
        assert!(manifest.is_empty());
        assert_eq!(body, "fn main() {}");
        assert_eq!(skipped, 0);
    }

    #[test]
    fn parse_script_rejects_unterminated_frontmatter() {
        let err = parse_script("---\n[dependencies]\nfn main() {}\n").unwrap_err();
        assert!(err.contains("never closed"), "got: {err}");
    }

    #[test]
    fn render_manifest_anchors_paths_and_adds_bin_workspace() {
        // A relative path dep is re-anchored against the script dir; a version dep
        // is left alone; [[bin]] and an isolating [workspace] are appended.
        let fm = "[dependencies]\n\
                  mira-eval = { path = \"../crates/mira-eval\" }\n\
                  tokio = { version = \"1\" }\n";
        // Use the repo root as the script dir so canonicalize resolves.
        let script_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap() // crates/
            .parent()
            .unwrap() // repo root
            .join("examples");
        let out = render_manifest(fm, "greet", &script_dir).unwrap();
        assert!(out.contains("[[bin]]"));
        assert!(out.contains("name = \"greet\""));
        assert!(out.contains("[workspace]"));
        assert!(out.contains("edition = \"2024\""));
        // The path dep became absolute and points at the real crate.
        assert!(out.contains("crates/mira-eval"));
        assert!(!out.contains("../crates/mira-eval"));
        // The version dep is untouched.
        assert!(out.contains("version = \"1\""));
    }

    #[test]
    fn sanitize_crate_name_handles_odd_stems() {
        assert_eq!(sanitize_crate_name("greet"), "greet");
        assert_eq!(sanitize_crate_name("my study.rs"), "my_study_rs");
        assert_eq!(sanitize_crate_name("123"), "study-123");
    }

    #[test]
    fn script_launcher_native_mode_uses_zscript() {
        // MIRA_SCRIPT_NATIVE routes to `cargo -Zscript <path>` without touching
        // the filesystem. Scoped set/remove keeps the env change local.
        let l = config::LauncherConfig {
            script: Some("study.rs".into()),
            ..Default::default()
        };
        // SAFETY: single-threaded test; restored immediately after the call.
        unsafe { std::env::set_var("MIRA_SCRIPT_NATIVE", "1") };
        let (program, args) = parts(&build_command(&l).unwrap());
        unsafe { std::env::remove_var("MIRA_SCRIPT_NATIVE") };
        assert_eq!(program, "cargo");
        assert_eq!(args, ["-Zscript", "study.rs"]);
    }
}
