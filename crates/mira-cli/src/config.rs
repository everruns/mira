//! Host configuration (`mira.toml`) and the run store.
//!
//! Every `mira run`/`mira score` saves a self-contained, timestamped folder under
//! a results directory (**save-by-default**; opt out with `--dry-run`), so runs
//! accumulate in a stable place and can be listed, compared, resumed, and
//! re-reported later. The directory is `[results].dir` from the nearest
//! `mira.toml`, else `./results`.
//!
//! Layout per run (`<results_dir>/<run_id>/`):
//! ```text
//! meta.json                  run identity: id, study, timestamps, environment, summary
//! report.json                canonical machine-readable record (summary + per-case)
//! report.html                self-contained transcript viewer
//! cases/<key>/result.json    one finished case (eval/sample@target[…]#trial)
//! ```
//!
//! `meta.json` is written when the run starts (a header) and rewritten when it
//! finishes (with the end time and summary). Each case's `result.json` lands as
//! that case completes — so an interrupted run stays resumable with
//! `mira run --resume <run_id>` and can be re-rendered with `mira report <run_id>`
//! without re-executing anything.
//!
//! The `[environment]` section controls whether (and with what labels) the run's
//! environment — git checkout, box, host version — is captured into `meta.json`.
//! Capture is on by default; see [`EnvironmentConfig`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use mira::protocol::RunResult;
use mira::report::{self, Format};
use mira::run::RunMeta;

/// Default results directory when `mira.toml` doesn't set `[results].dir`.
pub const DEFAULT_RESULTS_DIR: &str = "./results";

/// Parsed `mira.toml`. All sections optional; an absent file yields defaults.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub results: ResultsConfig,
    #[serde(default)]
    pub environment: EnvironmentConfig,
    /// Named selection presets (`[presets.NAME]`), applied with `--preset NAME`.
    #[serde(default)]
    pub presets: BTreeMap<String, Preset>,
    /// Named launchers (`[launchers.NAME]`), selected with `--launcher NAME`.
    #[serde(default)]
    pub launchers: BTreeMap<String, LauncherConfig>,
    /// Launcher used when neither a launch flag nor `--launcher` picks one.
    /// Must name a key in `[launchers]`.
    #[serde(default)]
    pub default_launcher: Option<String>,
    /// Per-target host settings (`[targets.LABEL]`), keyed by target label — e.g.
    /// a wall-clock `timeout`. Host-side only (not advertised by the study).
    #[serde(default)]
    pub targets: BTreeMap<String, TargetConfig>,
    /// Directory containing the `mira.toml` this was loaded from, used to
    /// resolve relative paths. `None` for a default/parsed-in-memory config, in
    /// which case relative paths are returned verbatim (cwd-relative).
    #[serde(default, skip)]
    base: Option<PathBuf>,
}

/// A named **selection preset** (`[presets.NAME]` in `mira.toml`): a saved bundle
/// of selection criteria, applied with `--preset NAME`. Every field is optional;
/// explicit CLI flags override the preset. The host owns selection, so a preset
/// only *subsets* the grid the study declared (targets, samples, evals, axes) —
/// it never adds cases.
///
/// `targets`, `samples`, and `evals` are per-dimension selectors that match the
/// target label / sample id / eval name by **glob** (`*`, `?`, `[set]`,
/// `{a,b}`); a literal value (no wildcard) is an exact match. Each accepts a
/// single string or a list.
///
/// ```toml
/// [presets.smoke]
/// targets = "anthropic/*"            # glob on target labels (or a list)
/// samples = ["france", "spain"]      # glob on sample ids
/// evals   = ["greet", "coding"]      # glob on eval names (hence their subjects)
/// tag     = "quick"                  # only samples carrying this tag
/// axes    = { effort = ["low"] }     # restrict secondary axes (values are globs)
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Preset {
    /// Restrict to sample ids matching these glob patterns.
    #[serde(default, deserialize_with = "string_or_seq")]
    pub samples: Vec<String>,
    /// Only run samples carrying this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Restrict the primary (target) axis to labels matching these globs.
    #[serde(default, deserialize_with = "string_or_seq")]
    pub targets: Vec<String>,
    /// Restrict to evals whose name matches these globs (and therefore their subjects).
    #[serde(default, deserialize_with = "string_or_seq")]
    pub evals: Vec<String>,
    /// Restrict secondary axes: axis name → allowed values (each a glob).
    #[serde(default)]
    pub axes: BTreeMap<String, Vec<String>>,
    /// Default per-case wall-clock timeout (seconds) for this preset. Overridden
    /// by `--timeout` (CLI) and by a per-target `[targets.LABEL].timeout`.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Per-target host configuration (`[targets.LABEL]` in `mira.toml`), keyed by the
/// target's label. Host-side only: these settings shape how the host *drives* a
/// target, so they live in host config rather than being advertised by the study.
///
/// ```toml
/// [targets."anthropic/claude-opus-4-8"]
/// timeout = 300   # give up on a case for this target after 5 minutes
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub struct TargetConfig {
    /// Wall-clock seconds for one case against this target before the host gives
    /// up: it cancels the in-flight run and records the case failed. `None` ⇒ no
    /// limit. Takes precedence over a preset's `timeout`; `--timeout` overrides it.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Accept either a single string (`targets = "sim"`) or a list
/// (`targets = ["a", "b"]`) for a `Vec<String>` field, so presets read naturally
/// whether one value or many. Commas are *not* split here — a comma is a literal
/// glob char (and `{a,b}` brace-alternation is the in-pattern "or") — list
/// multiple patterns as an array.
fn string_or_seq<'de, D>(de: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(de)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

/// A named **launcher** (`[launchers.NAME]` in `mira.toml`): a saved way to start
/// the study process, so a repo's invocation lives in config instead of being
/// retyped on every `mira` call. Selected with `--launcher NAME` (or via
/// `default_launcher` when no flag picks one); explicit launch flags override the
/// matching fields, mirroring how `--preset` composes with selection flags.
///
/// The launch mode (`cmd` | `bin` | `example` | `uv` | `python` | `python3`) is
/// mutually exclusive; `package` and `manifest_path` modify a cargo bin/example
/// launch.
///
/// ```toml
/// [launchers.greet]
/// bin = "greet"            # cargo run -q --bin greet
/// package = "myapp"        # …from package `myapp` (optional)
///
/// [launchers.py]
/// python3 = "study.py"     # python3 study.py (a polyglot study)
///
/// default_launcher = "greet"
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub struct LauncherConfig {
    /// `cargo run -q --bin <NAME>`.
    #[serde(default)]
    pub bin: Option<String>,
    /// `cargo run -q --example <NAME>`.
    #[serde(default)]
    pub example: Option<String>,
    /// An arbitrary command (split on whitespace).
    #[serde(default)]
    pub cmd: Option<String>,
    /// `uv run <SCRIPT...>` (split on whitespace).
    #[serde(default)]
    pub uv: Option<String>,
    /// `python <SCRIPT...>` (split on whitespace).
    #[serde(default)]
    pub python: Option<String>,
    /// `python3 <SCRIPT...>` (split on whitespace).
    #[serde(default)]
    pub python3: Option<String>,
    /// Cargo package to run the bin/example from (`-p`).
    #[serde(default)]
    pub package: Option<String>,
    /// Passed through to cargo (`--manifest-path`).
    #[serde(default)]
    pub manifest_path: Option<String>,
}

impl Config {
    /// Look up a named preset, erroring (with the available names) when absent so
    /// a typo'd `--preset` fails loudly rather than silently running everything.
    pub fn preset(&self, name: &str) -> Result<Preset, String> {
        self.presets.get(name).cloned().ok_or_else(|| {
            let mut names: Vec<&str> = self.presets.keys().map(String::as_str).collect();
            names.sort_unstable();
            let known = if names.is_empty() {
                "none defined in mira.toml".to_string()
            } else {
                names.join(", ")
            };
            format!("no such preset {name:?} (known: {known})")
        })
    }

    /// Look up a named launcher, erroring (with the available names) when absent so
    /// a typo'd `--launcher`/`default_launcher` fails loudly rather than silently
    /// falling back to the default study.
    pub fn launcher(&self, name: &str) -> Result<LauncherConfig, String> {
        self.launchers.get(name).cloned().ok_or_else(|| {
            let mut names: Vec<&str> = self.launchers.keys().map(String::as_str).collect();
            names.sort_unstable();
            let known = if names.is_empty() {
                "none defined in mira.toml".to_string()
            } else {
                names.join(", ")
            };
            format!("no such launcher {name:?} (known: {known})")
        })
    }
}

/// `[results]` — where saved runs are written (one folder per run).
#[derive(Debug, Default, Deserialize)]
pub struct ResultsConfig {
    /// Base directory for per-run folders. Defaults to [`DEFAULT_RESULTS_DIR`].
    pub dir: Option<String>,
}

/// `[environment]` — what context to record in a saved run's `meta.json`.
///
/// Capture is **on by default**: a saved run is far more useful when it carries
/// the commit and box it came from. Set `enabled = false` to opt out, or add
/// `[environment.labels]` to stamp every run with static context (team, region,
/// suite tier, …) for later filtering.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EnvironmentConfig {
    /// Capture environment metadata into `meta.json`. Default `true`.
    pub enabled: bool,
    /// Static labels merged into every captured environment. Override
    /// auto-detected CI labels on key collision.
    pub labels: BTreeMap<String, String>,
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        EnvironmentConfig {
            enabled: true,
            labels: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Load the nearest `mira.toml` by walking up from the current directory.
    /// A missing file is not an error (defaults); a malformed one warns and
    /// falls back to defaults rather than aborting the run.
    pub fn load() -> Config {
        let Some(path) = find_config() else {
            return Config::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Config::parse(&text) {
                Ok(mut cfg) => {
                    cfg.base = path.parent().map(Path::to_path_buf);
                    cfg
                }
                Err(e) => {
                    eprintln!("warning: ignoring {}: {e}", path.display());
                    Config::default()
                }
            },
            Err(e) => {
                eprintln!("warning: cannot read {}: {e}", path.display());
                Config::default()
            }
        }
    }

    /// Parse config TOML text (no `base`; relative paths stay cwd-relative).
    pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }

    /// Parse config text resolving relative paths against `base` (the directory
    /// containing the file) — how [`Config::load`] builds it. Exposed so
    /// `mira doctor` can surface the parse error itself instead of the
    /// warn-and-default fallback `load()` applies.
    pub fn parse_at(text: &str, base: &Path) -> Result<Config, toml::de::Error> {
        let mut cfg = Config::parse(text)?;
        cfg.base = Some(base.to_path_buf());
        Ok(cfg)
    }

    /// The configured results dir, or the default. A relative dir from a loaded
    /// `mira.toml` resolves against that file's directory, so results land in the
    /// same place no matter which subdir `mira` runs from.
    pub fn results_dir(&self) -> String {
        let dir = self.results.dir.as_deref().unwrap_or(DEFAULT_RESULTS_DIR);
        match &self.base {
            Some(base) if !Path::new(dir).is_absolute() => {
                let rel = dir.strip_prefix("./").unwrap_or(dir);
                base.join(rel).to_string_lossy().into_owned()
            }
            _ => dir.to_string(),
        }
    }
}

/// Find `mira.toml` by walking up from the current working directory to the root.
pub fn find_config() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("mira.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The run folder for `run_id` under `base`: `<base>/<run_id>/`.
pub fn run_dir(base: &str, run_id: &str) -> PathBuf {
    Path::new(base).join(run_id)
}

/// Reversible, filesystem-safe encoding of a case key — `[A-Za-z0-9]` kept
/// verbatim, every other byte escaped as `_HH` (hex) — so distinct keys can never
/// collide onto the same path. Shared by the run store (`cases/<enc>/`) and the
/// execution-artifact store.
pub fn encode_key(key: &str) -> String {
    let mut safe = String::with_capacity(key.len());
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() {
            safe.push(b as char);
        } else {
            safe.push('_');
            safe.push_str(&format!("{b:02x}"));
        }
    }
    safe
}

/// Create the run folder (and its `cases/` subdir) and write the header `meta.json`.
/// Called at run start so a run is resumable from its first completed case.
pub fn init_run(run_dir: &Path, meta: &RunMeta) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir.join("cases"))?;
    write_meta(run_dir, meta)
}

/// Persist one finished case under `<run_dir>/cases/<key>/result.json`. Written to
/// a temp file and renamed, so a crash mid-write never leaves a torn `result.json`
/// (and a re-run overwrites a case's own folder without touching the others).
pub fn write_case_result(run_dir: &Path, key: &str, result: &RunResult) -> std::io::Result<()> {
    let dir = run_dir.join("cases").join(encode_key(key));
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(result)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join("result.json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, dir.join("result.json"))
}

/// Load every `cases/*/result.json` under `run_dir`, sorted by case key for a
/// stable order. A missing `cases/` dir yields an empty vec (a fresh or dry run);
/// unreadable/invalid files are skipped with a warning so a partial write is
/// visible rather than silently dropped.
pub fn load_case_results(run_dir: &Path) -> Vec<RunResult> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(run_dir.join("cases")) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path().join("result.json");
        if !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<RunResult>(&text) {
                Ok(r) => out.push(r),
                Err(e) => eprintln!(
                    "warning: skipping {}: invalid result JSON: {e}",
                    path.display()
                ),
            },
            Err(e) => eprintln!("warning: skipping {}: {e}", path.display()),
        }
    }
    out.sort_by_key(|r| r.key());
    out
}

/// Write `meta.json` into the run folder.
pub fn write_meta(run_dir: &Path, meta: &RunMeta) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(run_dir.join("meta.json"), json)
}

/// Load `meta.json` from a run folder, if present and valid (e.g. to recover a
/// resumed run's original start time).
pub fn load_meta(run_dir: &Path) -> Option<RunMeta> {
    let text = std::fs::read_to_string(run_dir.join("meta.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Render and write the run's reports (`report.json`/`report.html`) and the final
/// `meta.json`. An optional `--group-by` view is folded into both reports. Used at
/// the end of a run and by `mira report` to re-render a saved run in place.
pub fn finalize_run(
    run_dir: &Path,
    meta: &RunMeta,
    results: &[RunResult],
    group: Option<report::Group<'_>>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    std::fs::write(
        run_dir.join("report.json"),
        report::render_with_group(results, Format::Json, group),
    )?;
    std::fs::write(
        run_dir.join("report.html"),
        report::render_with_group(results, Format::Html, group),
    )?;
    write_meta(run_dir, meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mira::protocol::TranscriptSummary;

    #[test]
    fn parse_results_dir() {
        let cfg = Config::parse("[results]\ndir = \"/tmp/evals\"\n").unwrap();
        assert_eq!(cfg.results_dir(), "/tmp/evals");
    }

    #[test]
    fn environment_capture_on_by_default() {
        // Absent section and empty file both default to enabled with no labels.
        let cfg = Config::default();
        assert!(cfg.environment.enabled);
        assert!(cfg.environment.labels.is_empty());
        assert!(Config::parse("").unwrap().environment.enabled);
        assert!(
            Config::parse("[results]\ndir = \"x\"\n")
                .unwrap()
                .environment
                .enabled
        );
    }

    #[test]
    fn environment_config_parses_disable_and_labels() {
        let cfg = Config::parse(
            "[environment]\nenabled = false\n\n[environment.labels]\nteam = \"search\"\n",
        )
        .unwrap();
        assert!(!cfg.environment.enabled);
        assert_eq!(
            cfg.environment.labels.get("team").map(String::as_str),
            Some("search")
        );
    }

    #[test]
    fn empty_config_uses_default() {
        let cfg = Config::default();
        assert_eq!(cfg.results_dir(), DEFAULT_RESULTS_DIR);
        // An empty file parses and still defaults.
        assert_eq!(
            Config::parse("").unwrap().results_dir(),
            DEFAULT_RESULTS_DIR
        );
    }

    #[test]
    fn relative_dir_resolves_against_config_parent() {
        // A relative dir from a loaded mira.toml resolves against the file's
        // directory (not cwd), and a leading `./` is normalised away.
        let cfg = Config {
            results: ResultsConfig {
                dir: Some("./results".into()),
            },
            base: Some(PathBuf::from("/proj")),
            ..Default::default()
        };
        assert_eq!(cfg.results_dir(), "/proj/results");

        // An absolute dir is left untouched.
        let abs = Config {
            results: ResultsConfig {
                dir: Some("/var/evals".into()),
            },
            base: Some(PathBuf::from("/proj")),
            ..Default::default()
        };
        assert_eq!(abs.results_dir(), "/var/evals");
    }

    #[test]
    fn launchers_parse_and_lookup() {
        let cfg = Config::parse(
            "default_launcher = \"greet\"\n\n\
             [launchers.greet]\nbin = \"greet\"\npackage = \"myapp\"\n\n\
             [launchers.py]\ncmd = \"python study.py\"\n",
        )
        .unwrap();
        assert_eq!(cfg.default_launcher.as_deref(), Some("greet"));

        let greet = cfg.launcher("greet").unwrap();
        assert_eq!(greet.bin.as_deref(), Some("greet"));
        assert_eq!(greet.package.as_deref(), Some("myapp"));
        assert!(greet.cmd.is_none());

        let py = cfg.launcher("py").unwrap();
        assert_eq!(py.cmd.as_deref(), Some("python study.py"));

        // A typo names the known launchers so it fails loudly.
        let err = cfg.launcher("nope").unwrap_err();
        assert!(err.contains("greet") && err.contains("py"), "{err}");
    }

    #[test]
    fn per_target_and_preset_timeouts_parse() {
        let cfg = Config::parse(
            "[targets.\"anthropic/opus\"]\ntimeout = 300\n\n\
             [presets.slow]\ntimeout = 120\ntargets = [\"sim\"]\n",
        )
        .unwrap();
        assert_eq!(
            cfg.targets.get("anthropic/opus").and_then(|t| t.timeout),
            Some(300)
        );
        assert_eq!(cfg.preset("slow").unwrap().timeout, Some(120));
        // A target with no [targets.LABEL] section has no configured timeout.
        assert!(!cfg.targets.contains_key("sim"));
    }

    #[test]
    fn no_timeouts_by_default() {
        let cfg = Config::parse("[presets.smoke]\ntargets = [\"sim\"]\n").unwrap();
        assert!(cfg.targets.is_empty());
        assert!(cfg.preset("smoke").unwrap().timeout.is_none());
    }

    #[test]
    fn preset_accepts_string_or_list() {
        // `targets` as a single string, `samples`/`evals` as lists.
        let cfg = Config::parse(
            "[presets.smoke]\n\
             targets = \"anthropic/*\"\n\
             samples = [\"france\", \"spain\"]\n\
             evals = \"greet\"\n\
             tag = \"quick\"\n",
        )
        .unwrap();
        let p = cfg.preset("smoke").unwrap();
        assert_eq!(p.targets, vec!["anthropic/*"]);
        assert_eq!(p.samples, vec!["france", "spain"]);
        assert_eq!(p.evals, vec!["greet"]);
        assert_eq!(p.tag.as_deref(), Some("quick"));

        // A typo names the known presets so it fails loudly.
        let err = cfg.preset("nope").unwrap_err();
        assert!(err.contains("smoke"), "{err}");
    }

    #[test]
    fn no_launchers_is_default() {
        let cfg = Config::default();
        assert!(cfg.launchers.is_empty());
        assert!(cfg.default_launcher.is_none());
        assert!(cfg.launcher("x").unwrap_err().contains("none defined"));
    }

    fn run_result(sample: &str, passed: bool) -> RunResult {
        RunResult {
            eval: "greet".into(),
            sample: sample.into(),
            target: "sim".into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            input: Vec::new(),
            expected: None,
            passed,
            aggregate: if passed { 1.0 } else { 0.0 },
            scores: vec![],
            transcript: TranscriptSummary::default(),
            skipped: false,
        }
    }

    #[test]
    fn run_store_roundtrips_cases_and_meta() {
        use mira::run::{RUN_META_FORMAT, RunSummary};
        let tmp = tempfile::tempdir().unwrap();
        let rd = run_dir(tmp.path().to_str().unwrap(), "20260621T090012Z-abcd");

        // Header written at start: cases/ exists and the start time round-trips.
        let header = RunMeta {
            format: RUN_META_FORMAT,
            run_id: "20260621T090012Z-abcd".into(),
            study: "greet".into(),
            study_version: None,
            started_unix: 100,
            finished_unix: 0,
            environment: None,
            summary: RunSummary::default(),
        };
        init_run(&rd, &header).unwrap();
        assert!(rd.join("cases").is_dir());
        assert_eq!(load_meta(&rd).unwrap().started_unix, 100);

        // Per-case results land under cases/<enc>/result.json and reload sorted.
        let hi = run_result("hi", true);
        let bye = run_result("bye", false);
        write_case_result(&rd, &hi.key(), &hi).unwrap();
        write_case_result(&rd, &bye.key(), &bye).unwrap();
        let loaded = load_case_results(&rd);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].sample, "bye", "sorted by case key");
        assert_eq!(loaded[1].sample, "hi");

        // Finalize writes the reports and rewrites meta with the end time/summary.
        let final_meta = RunMeta {
            finished_unix: 200,
            summary: RunSummary::of(&loaded),
            ..header
        };
        finalize_run(&rd, &final_meta, &loaded, None).unwrap();
        assert!(rd.join("report.json").is_file());
        assert!(rd.join("report.html").is_file());
        assert_eq!(load_meta(&rd).unwrap().finished_unix, 200);
    }
}
