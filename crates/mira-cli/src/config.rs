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
//! meta.json     run identity: id, study, start/finish timestamps, summary
//! ```

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
}

/// `[results]` — where `--save` writes run folders.
#[derive(Debug, Default, Deserialize)]
pub struct ResultsConfig {
    /// Base directory for per-run folders. Defaults to [`DEFAULT_RESULTS_DIR`].
    pub dir: Option<String>,
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
            Ok(text) => Config::parse(&text).unwrap_or_else(|e| {
                eprintln!("warning: ignoring {}: {e}", path.display());
                Config::default()
            }),
            Err(e) => {
                eprintln!("warning: cannot read {}: {e}", path.display());
                Config::default()
            }
        }
    }

    /// Parse config TOML text.
    pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }

    /// The configured results dir, or the default.
    pub fn results_dir(&self) -> String {
        self.results
            .dir
            .clone()
            .unwrap_or_else(|| DEFAULT_RESULTS_DIR.to_string())
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

/// Resolve the results base directory for a `--save` value: explicit dir wins,
/// then `mira.toml`, then the default. `None` means `--save` was not given.
pub fn resolve_results_dir(save: &Option<String>, config: &Config) -> Option<String> {
    match save {
        None => None,
        // `--save` with no value: use config/default.
        Some(s) if s.is_empty() => Some(config.results_dir()),
        // `--save <DIR>`: explicit override.
        Some(dir) => Some(dir.clone()),
    }
}

/// Write a run's report bundle into `<base>/<run_id>/`. Returns the run folder.
pub fn save_run(base: &str, meta: &RunMeta, results: &[RunResult]) -> std::io::Result<PathBuf> {
    let run_dir = Path::new(base).join(&meta.run_id);
    std::fs::create_dir_all(&run_dir)?;
    std::fs::write(
        run_dir.join("report.json"),
        report::render(results, Format::Json),
    )?;
    std::fs::write(
        run_dir.join("report.html"),
        report::render(results, Format::Html),
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
            summary: RunSummary::default(),
        };
        let run_dir = save_run(dir.path().to_str().unwrap(), &meta, &[]).unwrap();
        assert!(run_dir.join("report.json").is_file());
        assert!(run_dir.join("report.html").is_file());
        let meta_back = std::fs::read_to_string(run_dir.join("meta.json")).unwrap();
        assert!(meta_back.contains("20260621T090012Z-abcd"));
        assert!(meta_back.contains("\"started_unix\": 100"));
    }
}
