//! `mira doctor` — diagnose a Mira setup and optionally fix what's safely
//! fixable.
//!
//! Three layers, each its own report section:
//!
//! 1. **Config** (`mira.toml`): parse errors, unknown/misspelled keys, launcher
//!    shape (missing/conflicting modes, absent scripts and programs), preset
//!    mistakes, per-target overrides.
//! 2. **Study**: launch the study exactly like `run` would, then lint the
//!    advertised listing — duplicate sample ids / target labels / axis values
//!    (which collide case keys, so results overwrite each other), empty
//!    datasets/matrices/scorers, unavailable targets — and cross-check the
//!    config's presets and `[targets.LABEL]` sections against it.
//! 3. **Run store** (the results dir): torn or foreign run folders, interrupted
//!    runs, invalid case results, leftover temp files, missing reports.
//!
//! Design decisions: every check is a pure function from parsed inputs to
//! [`Finding`]s, so each is unit-testable without a process or filesystem
//! (the run-store checks take a directory, tested against a tempdir). `--fix`
//! applies only [`Fix`]es that cannot lose data: removing leftover `*.tmp`
//! files from interrupted atomic writes, and re-rendering a finished run's
//! reports from its stored per-case results. Anything ambiguous (a typo'd key,
//! a preset that matches nothing) is reported with a suggestion instead of
//! edited. Exit code: non-zero iff any error-severity finding remains, so
//! `mira doctor` can gate CI.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::config::{self, Config};
use mira::Host;
use mira::protocol::{EvalInfo, InitializeResult, ListResult, RunResult};
use mira::run::RunMeta;

/// How bad a finding is. Warnings never fail doctor; errors set the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// One diagnosed problem, optionally carrying a safe, mechanical [`Fix`].
#[derive(Debug)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
    pub fix: Option<Fix>,
}

impl Finding {
    fn warn(message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Warning,
            message: message.into(),
            fix: None,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Error,
            message: message.into(),
            fix: None,
        }
    }

    fn fixable(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// A repair doctor may apply under `--fix`. Only operations that cannot lose
/// data belong here — anything ambiguous stays a suggestion in the finding.
#[derive(Debug)]
pub enum Fix {
    /// Remove a leftover `*.tmp` file from an interrupted atomic write.
    RemoveFile(PathBuf),
    /// Re-render a finished run's `report.json`/`report.html` from its stored
    /// per-case results (same path `mira report <run_id>` takes).
    RenderReports(PathBuf),
}

impl Fix {
    fn describe(&self) -> String {
        match self {
            Fix::RemoveFile(p) => format!("remove {}", p.display()),
            Fix::RenderReports(dir) => format!("re-render reports in {}", dir.display()),
        }
    }

    fn apply(&self) -> io::Result<()> {
        match self {
            Fix::RemoveFile(p) => std::fs::remove_file(p),
            Fix::RenderReports(dir) => {
                let meta = config::load_meta(dir).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "no readable meta.json")
                })?;
                let results = config::load_case_results(dir);
                config::finalize_run(dir, &meta, &results, None)
            }
        }
    }
}

/// One report section: a heading, informational notes, and findings.
struct Section {
    title: String,
    notes: Vec<String>,
    findings: Vec<Finding>,
}

/// Entry point for `mira doctor`. `launch` is the already-resolved study launch
/// command (an `Err` — e.g. an unknown `--launcher` — is itself a finding, and
/// the config/run-store checks still report). Never returns: exits non-zero iff
/// any error-severity finding remains.
pub async fn doctor(launch: Result<Command, String>, fix: bool) -> ! {
    let (cfg, config_section) = check_config_section();
    let study_section = check_study_section(launch, &cfg).await;
    let store_section = check_store_section(&cfg);
    let sections = [config_section, study_section, store_section];

    let mut warnings = 0usize;
    let mut errors = 0usize;
    let mut fixes: Vec<Fix> = Vec::new();
    for section in sections {
        println!("{}", section.title);
        for note in &section.notes {
            println!("  {note}");
        }
        if section.findings.is_empty() {
            println!("  ok: no problems found");
        }
        for finding in section.findings {
            let tag = match finding.severity {
                Severity::Warning => "warning",
                Severity::Error => "error",
            };
            match finding.severity {
                Severity::Warning => warnings += 1,
                Severity::Error => errors += 1,
            }
            let fixable = if finding.fix.is_some() {
                " (fixable)"
            } else {
                ""
            };
            println!("  {tag}: {}{fixable}", finding.message);
            if let Some(f) = finding.fix {
                fixes.push(f);
            }
        }
        println!();
    }

    if fix {
        for f in &fixes {
            match f.apply() {
                Ok(()) => println!("fixed: {}", f.describe()),
                Err(e) => {
                    println!("fix failed: {}: {e}", f.describe());
                    errors += 1;
                }
            }
        }
        if !fixes.is_empty() {
            println!();
        }
    } else if !fixes.is_empty() {
        println!(
            "{} finding(s) fixable — run `mira doctor --fix` to apply\n",
            fixes.len()
        );
    }

    println!("summary: {warnings} warning(s), {errors} error(s)");
    std::process::exit(if errors > 0 { 1 } else { 0 })
}

/// Section 1: locate, parse, and lint `mira.toml`. Returns the loaded config
/// (with its base dir, for results-dir resolution) alongside the section.
fn check_config_section() -> (Config, Section) {
    let mut notes = Vec::new();
    let mut findings = Vec::new();
    let (cfg, title) = match config::find_config() {
        None => {
            notes.push("no mira.toml found (defaults apply)".to_string());
            (Config::default(), "config".to_string())
        }
        Some(path) => {
            let title = format!("config ({})", path.display());
            let cfg = match std::fs::read_to_string(&path) {
                Err(e) => {
                    findings.push(Finding::error(format!("cannot read mira.toml: {e}")));
                    Config::default()
                }
                Ok(text) => {
                    // Unknown-key lint works off the raw TOML, so typos that
                    // serde silently ignores (e.g. `[preset.x]`) surface here.
                    if let Ok(value) = text.parse::<toml::Value>() {
                        findings.extend(check_unknown_keys(&value));
                    }
                    let base = path.parent().unwrap_or(Path::new("."));
                    match Config::parse_at(&text, base) {
                        Ok(cfg) => {
                            notes.push(format!(
                                "parses: {} launcher(s), {} preset(s), {} target override(s)",
                                cfg.launchers.len(),
                                cfg.presets.len(),
                                cfg.targets.len()
                            ));
                            cfg
                        }
                        Err(e) => {
                            findings.push(Finding::error(format!("mira.toml does not parse: {e}")));
                            Config::default()
                        }
                    }
                }
            };
            (cfg, title)
        }
    };
    findings.extend(check_launchers(&cfg));
    findings.extend(check_presets(&cfg));
    (
        cfg,
        Section {
            title,
            notes,
            findings,
        },
    )
}

/// Section 2: launch the study, lint its listing, and cross-check the config
/// against what it advertises.
async fn check_study_section(launch: Result<Command, String>, cfg: &Config) -> Section {
    let mut notes = Vec::new();
    let mut findings = Vec::new();
    let mut title = "study".to_string();
    match launch {
        Err(e) => findings.push(Finding::error(format!("cannot resolve launcher: {e}"))),
        Ok(command) => match study_listing(command).await {
            Err(e) => findings.push(Finding::error(format!("study failed to start: {e}"))),
            Ok((info, listing)) => {
                title = format!(
                    "study ({} · protocol {})",
                    info.study, info.protocol_version
                );
                for eval in &listing.evals {
                    notes.push(eval_summary(eval));
                }
                findings.extend(check_listing(&listing));
                findings.extend(check_config_against_listing(cfg, &listing));
            }
        },
    }
    Section {
        title,
        notes,
        findings,
    }
}

/// Section 3: audit the saved-run store under the configured results dir.
fn check_store_section(cfg: &Config) -> Section {
    let dir = cfg.results_dir();
    let path = Path::new(&dir);
    let mut notes = Vec::new();
    let mut findings = Vec::new();
    if !path.exists() {
        notes.push("no results dir yet — created on first saved run".to_string());
    } else if !path.is_dir() {
        findings.push(Finding::error(format!(
            "results dir {dir} exists but is not a directory"
        )));
    } else {
        let (runs, run_findings) = check_run_store(path);
        notes.push(format!("{runs} saved run(s)"));
        findings.extend(run_findings);
    }
    Section {
        title: format!("results ({dir})"),
        notes,
        findings,
    }
}

/// Spawn the study and fetch its identity + complete listing, then shut it down.
async fn study_listing(
    command: Command,
) -> Result<(InitializeResult, ListResult), Box<dyn std::error::Error>> {
    let host = Host::spawn(command).await?;
    let info = host.initialize("mira-cli").await?;
    let listing = host.list_complete().await?;
    host.shutdown().await?;
    Ok((info, listing))
}

/// One informational line per eval: the grid it plans (empty axes are ignored,
/// matching `plan_grid`; 0/1 trials mean a single run).
fn eval_summary(eval: &EvalInfo) -> String {
    let combos: usize = eval.axes.iter().map(|a| a.values.len().max(1)).product();
    let trials = eval.trials.max(1);
    let cases = eval.samples.len() * eval.targets.len() * combos * trials;
    let mut s = format!(
        "eval {}: {} sample(s) × {} target(s)",
        eval.name,
        eval.samples.len(),
        eval.targets.len()
    );
    if combos > 1 {
        s.push_str(&format!(" × {combos} axis combo(s)"));
    }
    if trials > 1 {
        s.push_str(&format!(" × {trials} trial(s)"));
    }
    s.push_str(&format!(" = {cases} case(s)"));
    s
}

// --- mira.toml key lint -----------------------------------------------------

/// The keys serde actually reads, per table. Kept next to the lint (not the
/// config structs) so a new config field shows up as a doctor false-positive in
/// tests rather than silently un-linted.
const TOP_KEYS: &[&str] = &[
    "results",
    "environment",
    "presets",
    "launchers",
    "default_launcher",
    "targets",
];
const RESULTS_KEYS: &[&str] = &["dir"];
const ENVIRONMENT_KEYS: &[&str] = &["enabled", "labels"];
const LAUNCHER_KEYS: &[&str] = &[
    "bin",
    "example",
    "cmd",
    "uv",
    "python",
    "python3",
    "package",
    "manifest_path",
];
const PRESET_KEYS: &[&str] = &["samples", "tag", "targets", "evals", "axes", "timeout"];
const TARGET_KEYS: &[&str] = &["timeout"];

/// Flag keys `mira.toml` carries that the config never reads — serde ignores
/// unknown keys, so a typo (`[preset.smoke]`) otherwise fails silently.
fn check_unknown_keys(root: &toml::Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(table) = root.as_table() else {
        return out;
    };
    for (key, value) in table {
        if !TOP_KEYS.contains(&key.as_str()) {
            out.push(unknown_key(key, None, TOP_KEYS));
            continue;
        }
        match key.as_str() {
            "results" => check_table_keys(value, "results", RESULTS_KEYS, &mut out),
            // Only the section's own keys are checked — `[environment.labels]`
            // is free-form and check_table_keys never descends into it.
            "environment" => check_table_keys(value, "environment", ENVIRONMENT_KEYS, &mut out),
            // Named-entry tables: every entry's keys are checked against the set.
            "launchers" => check_named_tables(value, "launchers", LAUNCHER_KEYS, &mut out),
            "presets" => check_named_tables(value, "presets", PRESET_KEYS, &mut out),
            "targets" => check_named_tables(value, "targets", TARGET_KEYS, &mut out),
            _ => {}
        }
    }
    out
}

fn check_table_keys(value: &toml::Value, section: &str, known: &[&str], out: &mut Vec<Finding>) {
    if let Some(t) = value.as_table() {
        for k in t.keys() {
            if !known.contains(&k.as_str()) {
                out.push(unknown_key(k, Some(section), known));
            }
        }
    }
}

fn check_named_tables(value: &toml::Value, section: &str, known: &[&str], out: &mut Vec<Finding>) {
    if let Some(entries) = value.as_table() {
        for (name, entry) in entries {
            if let Some(t) = entry.as_table() {
                for k in t.keys() {
                    if !known.contains(&k.as_str()) {
                        out.push(unknown_key(k, Some(&format!("{section}.{name}")), known));
                    }
                }
            }
        }
    }
}

fn unknown_key(key: &str, section: Option<&str>, known: &[&str]) -> Finding {
    let place = section.map(|s| format!(" in [{s}]")).unwrap_or_default();
    let hint = match suggest(key, known) {
        Some(s) => format!("did you mean {s:?}?"),
        None => format!("known: {}", known.join(", ")),
    };
    Finding::warn(format!("mira.toml: unknown key {key:?}{place} ({hint})"))
}

/// Nearest known key within edit distance 2 — catches `preset`/`presets`,
/// `bins`/`bin`, `default-launcher`/`default_launcher`.
fn suggest<'a>(unknown: &str, known: &[&'a str]) -> Option<&'a str> {
    known
        .iter()
        .map(|k| (edit_distance(unknown, k), *k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

/// Plain Levenshtein distance; the inputs are short config keys.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            row.push(sub.min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()]
}

// --- launcher / preset checks (config only) ----------------------------------

/// Launch modes in the precedence order `build_command` applies them, so the
/// "which one wins" message matches actual behaviour.
fn launcher_modes(l: &config::LauncherConfig) -> Vec<(&'static str, &String)> {
    [
        ("cmd", &l.cmd),
        ("uv", &l.uv),
        ("python", &l.python),
        ("python3", &l.python3),
        ("bin", &l.bin),
        ("example", &l.example),
    ]
    .into_iter()
    .filter_map(|(name, v)| v.as_ref().map(|v| (name, v)))
    .collect()
}

fn check_launchers(cfg: &Config) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Some(name) = &cfg.default_launcher
        && !cfg.launchers.contains_key(name)
    {
        let known: Vec<&str> = cfg.launchers.keys().map(String::as_str).collect();
        let known = if known.is_empty() {
            "none defined".to_string()
        } else {
            known.join(", ")
        };
        out.push(Finding::error(format!(
            "default_launcher {name:?} is not a [launchers] entry (known: {known})"
        )));
    }
    for (name, l) in &cfg.launchers {
        let modes = launcher_modes(l);
        match modes.len() {
            0 => out.push(Finding::warn(format!(
                "launcher {name:?}: no launch mode set \
                 (bin/example/cmd/uv/python/python3) — falls back to the default `greet` bin"
            ))),
            1 => {}
            _ => {
                let names: Vec<&str> = modes.iter().map(|(n, _)| *n).collect();
                out.push(Finding::warn(format!(
                    "launcher {name:?}: multiple launch modes set ({}); `{}` wins, \
                     the others are ignored",
                    names.join(", "),
                    names[0]
                )));
            }
        }
        for (mode, value) in &modes {
            if value.trim().is_empty() {
                out.push(Finding::error(format!(
                    "launcher {name:?}: `{mode}` is empty"
                )));
            }
        }
        // Script/program existence for the winning mode. Scripts resolve from
        // the directory `mira` runs in (not the mira.toml dir), matching
        // `build_command`.
        if let Some((mode, value)) = modes.first() {
            let program = value.split_whitespace().next().unwrap_or_default();
            match *mode {
                "uv" | "python" | "python3" => {
                    if !program.is_empty() && !Path::new(program).exists() {
                        out.push(Finding::warn(format!(
                            "launcher {name:?}: script {program:?} not found \
                             (resolved from the directory `mira` runs in)"
                        )));
                    }
                }
                "cmd" => {
                    if !program.is_empty() && !on_path(program) {
                        out.push(Finding::warn(format!(
                            "launcher {name:?}: program {program:?} not found on PATH"
                        )));
                    }
                }
                // bin/example resolve via cargo at launch time.
                _ => {}
            }
            if !matches!(*mode, "bin" | "example")
                && (l.package.is_some() || l.manifest_path.is_some())
            {
                out.push(Finding::warn(format!(
                    "launcher {name:?}: `package`/`manifest_path` only apply to \
                     bin/example launches — ignored with `{mode}`"
                )));
            }
        }
    }
    out
}

/// Is `program` runnable — an existing path, or findable on PATH?
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).exists();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Config-only preset/target lint (cross-checks against the study's listing
/// live in [`check_config_against_listing`]).
fn check_presets(cfg: &Config) -> Vec<Finding> {
    let mut out = Vec::new();
    for (name, preset) in &cfg.presets {
        for (axis, values) in &preset.axes {
            if values.is_empty() {
                out.push(Finding::warn(format!(
                    "preset {name:?}: axis {axis:?} lists no values — it selects no cases"
                )));
            }
        }
        if preset.timeout == Some(0) {
            out.push(Finding::warn(format!(
                "preset {name:?}: timeout = 0 fails every case immediately"
            )));
        }
    }
    for (label, target) in &cfg.targets {
        if target.timeout == Some(0) {
            out.push(Finding::warn(format!(
                "[targets.{label:?}]: timeout = 0 fails every case immediately"
            )));
        }
    }
    out
}

// --- study listing checks -----------------------------------------------------

/// The values of `items` that appear more than once (sorted, deduped).
fn duplicates<'a>(items: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut dups = BTreeSet::new();
    for item in items {
        if !seen.insert(item) {
            dups.insert(item.to_string());
        }
    }
    dups.into_iter().collect()
}

/// Lint the study's advertised listing. Duplicates are errors because case keys
/// collide: two cases share one `cases/<key>/result.json`, so one result
/// silently overwrites the other and resume wrongly skips work.
fn check_listing(listing: &ListResult) -> Vec<Finding> {
    let mut out = Vec::new();
    if listing.evals.is_empty() {
        out.push(Finding::error("study advertises no evals"));
    }
    for name in duplicates(listing.evals.iter().map(|e| e.name.as_str())) {
        out.push(Finding::error(format!(
            "duplicate eval name {name:?} — case keys collide (results overwrite)"
        )));
    }
    for eval in &listing.evals {
        let n = &eval.name;
        if eval.samples.is_empty() {
            out.push(Finding::warn(format!(
                "eval {n:?}: no samples — plans no cases"
            )));
        }
        if eval.targets.is_empty() {
            out.push(Finding::warn(format!(
                "eval {n:?}: no targets — plans no cases"
            )));
        }
        if eval.scorers.is_empty() {
            out.push(Finding::warn(format!(
                "eval {n:?}: no scorers — cases have nothing to grade"
            )));
        }
        for id in duplicates(eval.samples.iter().map(|s| s.id.as_str())) {
            out.push(Finding::error(format!(
                "eval {n:?}: duplicate sample id {id:?} — case keys collide (results overwrite)"
            )));
        }
        for label in duplicates(eval.targets.iter().map(|t| t.label.as_str())) {
            out.push(Finding::error(format!(
                "eval {n:?}: duplicate target label {label:?} — case keys collide"
            )));
        }
        for axis in duplicates(eval.axes.iter().map(|a| a.name.as_str())) {
            out.push(Finding::error(format!(
                "eval {n:?}: duplicate axis {axis:?}"
            )));
        }
        for axis in &eval.axes {
            if axis.name == "target" {
                out.push(Finding::error(format!(
                    "eval {n:?}: axis named \"target\" collides with the primary target axis"
                )));
            }
            if axis.values.is_empty() {
                out.push(Finding::warn(format!(
                    "eval {n:?}: axis {:?} has no values — it is ignored",
                    axis.name
                )));
            }
            for v in duplicates(axis.values.iter().map(String::as_str)) {
                out.push(Finding::error(format!(
                    "eval {n:?}: duplicate value {v:?} in axis {:?} — case keys collide",
                    axis.name
                )));
            }
        }
        for target in &eval.targets {
            if !target.available {
                out.push(Finding::warn(format!(
                    "eval {n:?}: target {:?} unavailable — its cases will skip \
                     (missing the provider's API-key env var?)",
                    target.label
                )));
            }
        }
    }
    out
}

/// Cross-check `mira.toml` against what the study actually advertises.
/// Everything here is warning-severity: a mira.toml is legitimately shared
/// across studies (this repo's own has a preset for the `matrix` example that
/// the `greet` study doesn't declare), so a preset/target section that doesn't
/// apply to *this* study is suspicious, not broken. Using such a preset with
/// this study still fails loudly at `run --preset` time.
fn check_config_against_listing(cfg: &Config, listing: &ListResult) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut target_labels = BTreeSet::new();
    let mut eval_names = BTreeSet::new();
    let mut sample_ids = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut axes: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for eval in &listing.evals {
        eval_names.insert(eval.name.as_str());
        for t in &eval.targets {
            target_labels.insert(t.label.as_str());
        }
        for s in &eval.samples {
            sample_ids.insert(s.id.as_str());
            for tag in &s.tags {
                tags.insert(tag.as_str());
            }
        }
        for a in &eval.axes {
            let e = axes.entry(a.name.as_str()).or_default();
            for v in &a.values {
                e.insert(v.as_str());
            }
        }
    }

    for (label, tcfg) in &cfg.targets {
        if tcfg.timeout.is_some() && !target_labels.contains(label.as_str()) {
            out.push(Finding::warn(format!(
                "[targets.{label:?}]: no declared target has this label — the timeout is inert"
            )));
        }
    }

    let matches_any =
        |globs: &BTreeSet<&str>, pat: &str| globs.iter().any(|v| mira::glob_match(pat, v));
    for (name, preset) in &cfg.presets {
        for pat in &preset.targets {
            if !matches_any(&target_labels, pat) {
                out.push(Finding::warn(format!(
                    "preset {name:?}: targets glob {pat:?} matches no declared target \
                     (fails `run --preset {name}` against this study)"
                )));
            }
        }
        for pat in &preset.evals {
            if !matches_any(&eval_names, pat) {
                out.push(Finding::warn(format!(
                    "preset {name:?}: evals glob {pat:?} matches no declared eval \
                     (fails `run --preset {name}` against this study)"
                )));
            }
        }
        for (axis, values) in &preset.axes {
            // `target` is the primary axis; other names must be declared.
            let declared = if axis == "target" {
                Some(&target_labels)
            } else {
                axes.get(axis.as_str())
            };
            let Some(declared) = declared else {
                let known: Vec<&str> = axes.keys().copied().collect();
                out.push(Finding::warn(format!(
                    "preset {name:?}: unknown axis {axis:?} (declared: {})",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                )));
                continue;
            };
            for pat in values {
                if !matches_any(declared, pat) {
                    out.push(Finding::warn(format!(
                        "preset {name:?}: axis {axis:?} has no value matching {pat:?}"
                    )));
                }
            }
        }
        for pat in &preset.samples {
            if !matches_any(&sample_ids, pat) {
                out.push(Finding::warn(format!(
                    "preset {name:?}: samples glob {pat:?} matches no declared sample"
                )));
            }
        }
        if let Some(tag) = &preset.tag
            && !tags.contains(tag.as_str())
        {
            out.push(Finding::warn(format!(
                "preset {name:?}: no sample carries tag {tag:?} — it selects no cases"
            )));
        }
    }
    out
}

// --- run store checks ----------------------------------------------------------

/// Audit every run folder under the results dir. Returns `(run count, findings)`.
fn check_run_store(dir: &Path) -> (usize, Vec<Finding>) {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.push(Finding::warn(format!("cannot read results dir: {e}")));
            return (0, out);
        }
    };
    let mut runs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    runs.sort();
    for run in &runs {
        let name = run
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta_path = run.join("meta.json");
        let meta: Option<RunMeta> = if meta_path.is_file() {
            let parsed = std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            if parsed.is_none() {
                out.push(Finding::warn(format!(
                    "run {name}: meta.json is not valid run metadata"
                )));
            }
            parsed
        } else {
            out.push(Finding::warn(format!(
                "run {name}: no meta.json — not a run folder, or torn at creation"
            )));
            None
        };
        if let Some(m) = &meta
            && m.finished_unix == 0
        {
            out.push(Finding::warn(format!(
                "run {name}: never finished — resume with `mira run --resume {}` \
                 or delete the folder",
                m.run_id
            )));
        }
        let cases = run.join("cases");
        if let Ok(entries) = std::fs::read_dir(&cases) {
            let mut case_dirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            case_dirs.sort();
            for case in case_dirs {
                let result = case.join("result.json");
                if result.is_file() {
                    let valid = std::fs::read_to_string(&result)
                        .ok()
                        .and_then(|t| serde_json::from_str::<RunResult>(&t).ok())
                        .is_some();
                    if !valid {
                        out.push(Finding::warn(format!(
                            "run {name}: invalid case result {}",
                            result.display()
                        )));
                    }
                }
                let tmp = case.join("result.json.tmp");
                if tmp.is_file() {
                    out.push(
                        Finding::warn(format!(
                            "run {name}: leftover {} from an interrupted write",
                            tmp.display()
                        ))
                        .fixable(Fix::RemoveFile(tmp)),
                    );
                }
            }
        }
        if let Some(m) = &meta
            && m.finished_unix > 0
        {
            let missing: Vec<&str> = ["report.json", "report.html"]
                .into_iter()
                .filter(|f| !run.join(f).is_file())
                .collect();
            if !missing.is_empty() {
                out.push(
                    Finding::warn(format!("run {name}: missing {}", missing.join(", ")))
                        .fixable(Fix::RenderReports(run.clone())),
                );
            }
        }
    }
    (runs.len(), out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mira::protocol::{AxisInfo, SampleInfo, TargetInfo};
    use mira::run::{RUN_META_FORMAT, RunSummary};

    fn cfg(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    fn keys(text: &str) -> Vec<Finding> {
        check_unknown_keys(&text.parse::<toml::Value>().unwrap())
    }

    fn messages(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.message.as_str()).collect()
    }

    #[test]
    fn unknown_top_level_key_suggests_nearest() {
        let f = keys("[preset.smoke]\ntargets = \"sim\"\n");
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("\"preset\""), "{}", f[0].message);
        assert!(f[0].message.contains("\"presets\""), "{}", f[0].message);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn unknown_nested_keys_flagged_per_section() {
        let f = keys(
            "[launchers.g]\nbins = \"x\"\n\n[presets.p]\ntargest = \"sim\"\n\n\
             [results]\ndirectory = \"r\"\n",
        );
        let msgs = messages(&f).join("\n");
        assert!(msgs.contains("\"bins\" in [launchers.g]"), "{msgs}");
        assert!(msgs.contains("did you mean \"bin\"?"), "{msgs}");
        assert!(msgs.contains("\"targest\" in [presets.p]"), "{msgs}");
        assert!(msgs.contains("\"directory\" in [results]"), "{msgs}");
    }

    #[test]
    fn known_keys_and_free_form_labels_pass() {
        let f = keys(
            "default_launcher = \"g\"\n[launchers.g]\nbin = \"g\"\n\n\
             [environment.labels]\nanything_goes = \"yes\"\n\n\
             [presets.p]\naxes = { effort = [\"low\"] }\n\n[targets.\"a/b\"]\ntimeout = 3\n",
        );
        assert!(f.is_empty(), "{:?}", messages(&f));
    }

    #[test]
    fn default_launcher_must_exist() {
        let f = check_launchers(&cfg(
            "default_launcher = \"greta\"\n[launchers.greet]\nbin = \"greet\"\n",
        ));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert!(f[0].message.contains("greta") && f[0].message.contains("greet"));
    }

    #[test]
    fn launcher_mode_conflicts_and_gaps_warn() {
        // Multiple modes: cmd wins (build_command precedence).
        let f = check_launchers(&cfg("[launchers.x]\ncmd = \"echo hi\"\nbin = \"greet\"\n"));
        assert!(
            f.iter().any(|f| f.message.contains("`cmd` wins")),
            "{:?}",
            messages(&f)
        );
        // No mode at all.
        let f = check_launchers(&cfg("[launchers.x]\npackage = \"p\"\n"));
        assert!(
            f.iter().any(|f| f.message.contains("no launch mode")),
            "{:?}",
            messages(&f)
        );
        // Empty cmd is an error.
        let f = check_launchers(&cfg("[launchers.x]\ncmd = \" \"\n"));
        assert!(
            f.iter()
                .any(|f| f.severity == Severity::Error && f.message.contains("empty")),
            "{:?}",
            messages(&f)
        );
    }

    #[test]
    fn launcher_missing_script_and_program_warn() {
        let f = check_launchers(&cfg(
            "[launchers.py]\npython3 = \"definitely/missing/study.py\"\n",
        ));
        assert!(
            f.iter().any(|f| f.message.contains("script")),
            "{:?}",
            messages(&f)
        );
        let f = check_launchers(&cfg(
            "[launchers.c]\ncmd = \"definitely-not-a-real-program-xyz\"\n",
        ));
        assert!(
            f.iter().any(|f| f.message.contains("not found on PATH")),
            "{:?}",
            messages(&f)
        );
    }

    #[test]
    fn package_ignored_outside_cargo_modes() {
        let f = check_launchers(&cfg("[launchers.c]\ncmd = \"echo hi\"\npackage = \"p\"\n"));
        assert!(
            f.iter()
                .any(|f| f.message.contains("only apply to bin/example")),
            "{:?}",
            messages(&f)
        );
        // package with a bin mode is fine.
        let f = check_launchers(&cfg("[launchers.b]\nbin = \"g\"\npackage = \"p\"\n"));
        assert!(f.is_empty(), "{:?}", messages(&f));
    }

    #[test]
    fn preset_empty_axis_and_zero_timeouts_warn() {
        let f = check_presets(&cfg("[presets.p]\ntimeout = 0\naxes = { effort = [] }\n\n\
             [targets.\"a/b\"]\ntimeout = 0\n"));
        let msgs = messages(&f).join("\n");
        assert!(msgs.contains("lists no values"), "{msgs}");
        assert_eq!(f.len(), 3, "{msgs}"); // empty axis + two zero timeouts
        assert!(f.iter().all(|f| f.severity == Severity::Warning));
    }

    fn sample(id: &str, tags: &[&str]) -> SampleInfo {
        SampleInfo {
            id: id.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            metadata: Default::default(),
        }
    }

    fn target(label: &str, available: bool) -> TargetInfo {
        TargetInfo {
            label: label.into(),
            provider: "sim".into(),
            available,
            metadata: Default::default(),
        }
    }

    fn eval(name: &str) -> EvalInfo {
        EvalInfo {
            name: name.into(),
            description: String::new(),
            samples: vec![sample("france", &["smoke"]), sample("spain", &[])],
            next_cursor: None,
            scorers: vec!["contains".into()],
            targets: vec![target("sim", true)],
            axes: Vec::new(),
            max_turns: 0,
            trials: 0,
            seed: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn healthy_listing_has_no_findings() {
        let listing = ListResult {
            evals: vec![eval("greet")],
        };
        assert!(check_listing(&listing).is_empty());
    }

    #[test]
    fn empty_study_is_an_error() {
        let f = check_listing(&ListResult { evals: Vec::new() });
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
    }

    #[test]
    fn duplicate_ids_collide_case_keys() {
        let mut e = eval("greet");
        e.samples.push(sample("france", &[]));
        e.targets.push(target("sim", true));
        e.axes.push(AxisInfo {
            name: "effort".into(),
            values: vec!["low".into(), "low".into()],
        });
        let listing = ListResult {
            evals: vec![e, eval("greet")],
        };
        let f = check_listing(&listing);
        let msgs = messages(&f).join("\n");
        assert!(msgs.contains("duplicate eval name \"greet\""), "{msgs}");
        assert!(msgs.contains("duplicate sample id \"france\""), "{msgs}");
        assert!(msgs.contains("duplicate target label \"sim\""), "{msgs}");
        assert!(msgs.contains("duplicate value \"low\""), "{msgs}");
        assert!(f.iter().all(|f| f.severity == Severity::Error), "{msgs}");
    }

    #[test]
    fn degenerate_evals_warn() {
        let mut e = eval("greet");
        e.samples.clear();
        e.scorers.clear();
        e.targets = vec![target("anthropic/opus", false)];
        e.axes = vec![
            AxisInfo {
                name: "target".into(),
                values: vec!["x".into()],
            },
            AxisInfo {
                name: "effort".into(),
                values: Vec::new(),
            },
        ];
        let f = check_listing(&ListResult { evals: vec![e] });
        let msgs = messages(&f).join("\n");
        assert!(msgs.contains("no samples"), "{msgs}");
        assert!(msgs.contains("no scorers"), "{msgs}");
        assert!(msgs.contains("unavailable"), "{msgs}");
        assert!(
            msgs.contains("collides with the primary target axis"),
            "{msgs}"
        );
        assert!(msgs.contains("has no values"), "{msgs}");
    }

    #[test]
    fn presets_cross_checked_against_listing() {
        let mut e = eval("greet");
        e.axes.push(AxisInfo {
            name: "effort".into(),
            values: vec!["low".into(), "high".into()],
        });
        let listing = ListResult { evals: vec![e] };
        let cfg = cfg(
            "[presets.ok]\ntargets = \"s*\"\nevals = \"greet\"\ntag = \"smoke\"\n\
             axes = { effort = [\"low\"], target = [\"sim\"] }\n\n\
             [presets.bad]\ntargets = \"openai/*\"\nevals = \"nope\"\ntag = \"missing\"\n\
             samples = \"berlin\"\naxes = { speed = [\"fast\"], effort = [\"max\"] }\n\n\
             [targets.\"anthropic/opus\"]\ntimeout = 60\n",
        );
        let f = check_config_against_listing(&cfg, &listing);
        let msgs = messages(&f).join("\n");
        assert!(!msgs.contains("\"ok\""), "healthy preset flagged: {msgs}");
        assert!(msgs.contains("targets glob \"openai/*\""), "{msgs}");
        assert!(msgs.contains("evals glob \"nope\""), "{msgs}");
        assert!(msgs.contains("unknown axis \"speed\""), "{msgs}");
        assert!(msgs.contains("no value matching \"max\""), "{msgs}");
        assert!(msgs.contains("samples glob \"berlin\""), "{msgs}");
        assert!(msgs.contains("tag \"missing\""), "{msgs}");
        assert!(msgs.contains("[targets.\"anthropic/opus\"]"), "{msgs}");
        // All warnings: a mira.toml is shared across studies, so a preset that
        // doesn't apply to this one is suspicious, not broken.
        assert!(f.iter().all(|f| f.severity == Severity::Warning), "{msgs}");
    }

    fn meta(run_id: &str, finished: u64) -> RunMeta {
        RunMeta {
            format: RUN_META_FORMAT,
            run_id: run_id.into(),
            study: "greet".into(),
            study_version: None,
            started_unix: 100,
            finished_unix: finished,
            environment: None,
            summary: RunSummary::default(),
        }
    }

    #[test]
    fn run_store_flags_and_fixes_torn_and_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        // A finished run missing its reports, with a leftover tmp file.
        let run = base.join("20260101T000000Z-good");
        let m = meta("20260101T000000Z-good", 200);
        config::init_run(&run, &m).unwrap();
        let torn = run.join("cases").join("x");
        std::fs::create_dir_all(&torn).unwrap();
        std::fs::write(torn.join("result.json.tmp"), "{").unwrap();
        // An interrupted run and a foreign folder.
        config::init_run(
            &base.join("20260101T000001Z-open"),
            &meta("20260101T000001Z-open", 0),
        )
        .unwrap();
        std::fs::create_dir_all(base.join("junk")).unwrap();

        let (runs, f) = check_run_store(base);
        assert_eq!(runs, 3);
        let msgs = messages(&f).join("\n");
        assert!(msgs.contains("missing report.json, report.html"), "{msgs}");
        assert!(msgs.contains("interrupted write"), "{msgs}");
        assert!(msgs.contains("--resume 20260101T000001Z-open"), "{msgs}");
        assert!(msgs.contains("run junk: no meta.json"), "{msgs}");

        // Applying the fixes removes the tmp file and renders the reports.
        for fix in f.into_iter().filter_map(|f| f.fix) {
            fix.apply().unwrap();
        }
        assert!(!torn.join("result.json.tmp").exists());
        assert!(run.join("report.json").is_file());
        assert!(run.join("report.html").is_file());

        // A clean re-check: only the interrupted run and the foreign dir remain.
        let (_, f) = check_run_store(base);
        assert!(f.iter().all(|f| f.fix.is_none()), "{:?}", messages(&f));
    }

    #[test]
    fn invalid_case_result_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("20260101T000000Z-bad");
        config::init_run(&run, &meta("20260101T000000Z-bad", 0)).unwrap();
        let case = run.join("cases").join("c");
        std::fs::create_dir_all(&case).unwrap();
        std::fs::write(case.join("result.json"), "not json").unwrap();
        let (_, f) = check_run_store(tmp.path());
        assert!(
            f.iter().any(|f| f.message.contains("invalid case result")),
            "{:?}",
            messages(&f)
        );
    }

    #[test]
    fn eval_summary_counts_the_grid() {
        let mut e = eval("greet"); // 2 samples × 1 target
        e.trials = 3;
        e.axes = vec![
            AxisInfo {
                name: "effort".into(),
                values: vec!["low".into(), "high".into()],
            },
            AxisInfo {
                name: "empty".into(),
                values: Vec::new(), // ignored, like plan_grid
            },
        ];
        assert_eq!(
            eval_summary(&e),
            "eval greet: 2 sample(s) × 1 target(s) × 2 axis combo(s) × 3 trial(s) = 12 case(s)"
        );
    }

    #[test]
    fn edit_distance_drives_suggestions() {
        assert_eq!(suggest("preset", TOP_KEYS), Some("presets"));
        assert_eq!(
            suggest("default-launcher", TOP_KEYS),
            Some("default_launcher")
        );
        assert_eq!(suggest("completely_unrelated", TOP_KEYS), None);
    }
}
