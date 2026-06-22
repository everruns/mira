//! Host configuration (`mira.toml`) and the `--save` run store.
//!
//! `--save` writes a self-contained, timestamped folder per run under a results
//! directory, so runs accumulate in a stable place and can be listed/compared
//! later. The directory is resolved as: an explicit `--save <DIR>` value, else
//! `[results].dir` from the nearest `mira.toml`, else `./results`.
//!
//! Layout per run (`<results_dir>/<run_id>/`):
//! ```text
//! report.json   canonical machine-readable record (summary + per-case)
//! report.html   self-contained transcript viewer
//! meta.json     run identity: id, study, timestamps, environment, summary
//! ```
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

/// Default results directory when neither `--save <DIR>` nor `mira.toml` sets one.
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
    /// Directory containing the `mira.toml` this was loaded from, used to
    /// resolve relative paths. `None` for a default/parsed-in-memory config, in
    /// which case relative paths are returned verbatim (cwd-relative).
    #[serde(default, skip)]
    base: Option<PathBuf>,
}

/// A named **selection preset** (`[presets.NAME]` in `mira.toml`): a saved bundle
/// of selection criteria, applied with `--preset NAME`. Every field is optional;
/// explicit CLI flags override the preset. The host owns selection, so a preset
/// only *subsets* the grid the study declared (targets, axes, samples, evals) —
/// it never adds cells.
///
/// ```toml
/// [presets.smoke]
/// targets = ["sim"]            # primary axis (target labels)
/// tag = "quick"               # only samples carrying this tag
/// filter = "greet"            # substring on the case key
/// evals = ["greet", "coding"] # restrict to these evals (hence their subjects)
/// axes = { effort = ["low"] } # restrict secondary axes
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Preset {
    /// Substring filter on the case key (`eval/sample@target`).
    #[serde(default)]
    pub filter: Option<String>,
    /// Only run samples carrying this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Restrict the primary (target) axis to these labels.
    #[serde(default)]
    pub targets: Vec<String>,
    /// Restrict to these evals (and therefore their subjects).
    #[serde(default)]
    pub evals: Vec<String>,
    /// Restrict secondary axes: axis name → allowed values.
    #[serde(default)]
    pub axes: BTreeMap<String, Vec<String>>,
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
}

/// `[results]` — where `--save` writes run folders.
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
fn find_config() -> Option<PathBuf> {
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

/// Resolve the results base dir for a `--save` value, loading `mira.toml` only
/// when it's actually needed (a bare `--save`). Returns `None` when `--save`
/// wasn't given, so a plain run does no config I/O.
pub fn resolve_save_dir(save: &Option<String>) -> Option<String> {
    // Only a bare `--save` consults mira.toml; every other case avoids the
    // directory walk + file read entirely.
    if matches!(save, Some(s) if s.is_empty()) {
        resolve_results_dir(save, &Config::load())
    } else {
        resolve_results_dir(save, &Config::default())
    }
}

/// Resolve the results dir against an already-loaded config (the pure core of
/// [`resolve_save_dir`]).
pub fn resolve_results_dir(save: &Option<String>, config: &Config) -> Option<String> {
    match save {
        None => None,
        Some(s) if s.is_empty() => Some(config.results_dir()),
        Some(dir) => Some(dir.clone()),
    }
}

/// Write a run's report bundle into `<base>/<run_id>/`. Returns the run folder.
/// An optional `--group-by` view is folded into the saved JSON and HTML reports.
pub fn save_run(
    base: &str,
    meta: &RunMeta,
    results: &[RunResult],
    group: Option<report::Group<'_>>,
) -> std::io::Result<PathBuf> {
    let run_dir = Path::new(base).join(&meta.run_id);
    std::fs::create_dir_all(&run_dir)?;
    std::fs::write(
        run_dir.join("report.json"),
        report::render_with_group(results, Format::Json, group),
    )?;
    std::fs::write(
        run_dir.join("report.html"),
        report::render_with_group(results, Format::Html, group),
    )?;
    let meta_json = serde_json::to_string_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(run_dir.join("meta.json"), meta_json)?;
    Ok(run_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolve_precedence() {
        let cfg = Config::parse("[results]\ndir = \"from-toml\"\n").unwrap();
        // Flag absent.
        assert_eq!(resolve_results_dir(&None, &cfg), None);
        // Flag present, no value → config.
        assert_eq!(
            resolve_results_dir(&Some(String::new()), &cfg),
            Some("from-toml".to_string())
        );
        // Explicit value → override.
        assert_eq!(
            resolve_results_dir(&Some("cli-dir".into()), &cfg),
            Some("cli-dir".to_string())
        );
    }

    #[test]
    fn save_writes_bundle() {
        use mira::run::{RUN_META_FORMAT, RunSummary};
        let dir = tempfile::tempdir().unwrap();
        let meta = RunMeta {
            format: RUN_META_FORMAT,
            run_id: "20260621T090012Z-abcd".into(),
            study: "greet".into(),
            study_version: None,
            started_unix: 100,
            finished_unix: 200,
            environment: None,
            summary: RunSummary::default(),
        };
        let run_dir = save_run(dir.path().to_str().unwrap(), &meta, &[], None).unwrap();
        assert!(run_dir.join("report.json").is_file());
        assert!(run_dir.join("report.html").is_file());
        let meta_back = std::fs::read_to_string(run_dir.join("meta.json")).unwrap();
        assert!(meta_back.contains("20260621T090012Z-abcd"));
        assert!(meta_back.contains("\"started_unix\": 100"));
    }
}
