use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExperimentPlan {
    pub(crate) study: String,
    #[serde(default)]
    pub(crate) launcher: Option<String>,
    #[serde(default)]
    pub(crate) preset: Option<String>,
    #[serde(default)]
    pub(crate) slice: ExperimentSlice,
    pub(crate) treatments: Vec<Treatment>,
    #[serde(default)]
    pub(crate) compare: Option<Comparison>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExperimentSlice {
    #[serde(default)]
    pub(crate) samples: Vec<String>,
    #[serde(default)]
    pub(crate) targets: Vec<String>,
    #[serde(default)]
    pub(crate) trials: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Treatment {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Comparison {
    #[serde(default = "treatment_group")]
    pub(crate) group_by: String,
    pub(crate) baseline: String,
}

fn treatment_group() -> String {
    "treatment".to_owned()
}

impl ExperimentPlan {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read experiment plan {}: {e}", path.display()))?;
        let mut plan: Self = toml::from_str(&source)
            .map_err(|e| format!("invalid experiment plan {}: {e}", path.display()))?;
        plan.validate()?;
        plan.resolve_paths(path.parent().unwrap_or_else(|| Path::new(".")));
        Ok(plan)
    }

    fn validate(&self) -> Result<(), String> {
        if self.study.trim().is_empty() {
            return Err("experiment study cannot be empty".to_owned());
        }
        if self.treatments.is_empty() {
            return Err("experiment must declare at least one treatment".to_owned());
        }
        if self.slice.trials == Some(0) {
            return Err("slice.trials must be at least 1".to_owned());
        }
        let mut names = HashSet::new();
        for treatment in &self.treatments {
            if treatment.name.trim().is_empty() {
                return Err("treatment name cannot be empty".to_owned());
            }
            if !names.insert(treatment.name.as_str()) {
                return Err(format!("duplicate treatment name `{}`", treatment.name));
            }
        }
        if let Some(compare) = &self.compare {
            if compare.group_by != "treatment" {
                return Err("compare.group_by must be `treatment`".to_owned());
            }
            if !names.contains(compare.baseline.as_str()) {
                return Err(format!(
                    "baseline treatment `{}` is not declared",
                    compare.baseline
                ));
            }
        }
        Ok(())
    }

    // Relative path-looking environment values are resolved at load time so
    // treatment execution never depends on the caller's working directory.
    fn resolve_paths(&mut self, base: &Path) {
        for treatment in &mut self.treatments {
            for value in treatment.env.values_mut() {
                let path = PathBuf::from(value.as_str());
                if path.is_relative() && (value.starts_with("./") || value.starts_with("../")) {
                    let path = path.strip_prefix(".").unwrap_or(&path);
                    *value = base.join(path).to_string_lossy().into_owned();
                }
            }
        }
    }

    pub(crate) fn resolved_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self)
            .map_err(|e| format!("cannot serialize resolved experiment plan: {e}"))
    }

    pub(crate) fn archive(source: &Path, run_dir: &Path) -> Result<(), String> {
        let resolved = Self::load(source)?.resolved_toml()?;
        std::fs::copy(source, run_dir.join("experiment-source.toml"))
            .map_err(|e| format!("cannot archive experiment plan {}: {e}", source.display()))?;
        std::fs::write(run_dir.join("experiment.toml"), resolved)
            .map_err(|e| format!("cannot archive resolved experiment plan: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_treatments_and_shared_slice() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("prompt-experiment.toml");
        fs::write(
            &path,
            r#"
study = "harness_basic"
launcher = "harness-basic"
preset = "regression"

[slice]
samples = ["approval", "add-fn"]
targets = ["openai/gpt-5.5"]
trials = 3

[[treatments]]
name = "baseline"
env.HARNESS_BASIC_YOLOP_BIN = "./bin/current/yolop"

[[treatments]]
name = "minimal-prompt"
env.HARNESS_BASIC_YOLOP_BIN = "./bin/minimal/yolop"

[compare]
group_by = "treatment"
baseline = "baseline"
"#,
        )
        .unwrap();

        let plan = ExperimentPlan::load(&path).unwrap();
        assert_eq!(plan.study, "harness_basic");
        assert_eq!(plan.launcher.as_deref(), Some("harness-basic"));
        assert_eq!(plan.preset.as_deref(), Some("regression"));
        assert_eq!(plan.slice.samples, vec!["approval", "add-fn"]);
        assert_eq!(plan.slice.targets, vec!["openai/gpt-5.5"]);
        assert_eq!(plan.slice.trials, Some(3));
        assert_eq!(plan.treatments[0].name, "baseline");
        assert_eq!(
            plan.treatments[1].env["HARNESS_BASIC_YOLOP_BIN"],
            dir.path().join("bin/minimal/yolop").to_string_lossy()
        );
        let resolved = plan.resolved_toml().unwrap();
        assert_eq!(plan.compare.unwrap().baseline, "baseline");
        assert!(
            resolved.contains(
                &dir.path()
                    .join("bin/current/yolop")
                    .to_string_lossy()
                    .to_string()
            )
        );
        assert!(resolved.contains("baseline = \"baseline\""));
    }

    #[test]
    fn rejects_duplicate_treatment_names_and_unknown_baseline() {
        let dir = tempdir().unwrap();
        let duplicate = dir.path().join("duplicate.toml");
        fs::write(
            &duplicate,
            "study = \"demo\"\n[[treatments]]\nname = \"same\"\n[[treatments]]\nname = \"same\"\n",
        )
        .unwrap();
        assert!(
            ExperimentPlan::load(&duplicate)
                .unwrap_err()
                .contains("duplicate treatment name `same`")
        );

        let missing = dir.path().join("missing.toml");
        fs::write(&missing, "study = \"demo\"\n[[treatments]]\nname = \"candidate\"\n[compare]\nbaseline = \"baseline\"\n").unwrap();
        assert!(
            ExperimentPlan::load(&missing)
                .unwrap_err()
                .contains("baseline treatment `baseline` is not declared")
        );
    }

    #[test]
    fn resolves_relative_paths_from_manifest_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("experiment.toml");
        let source = "study = \"demo\"\n[[treatments]]\nname = \"candidate\"\nenv.BINARY = \"./bin/candidate\"\n";
        fs::write(&path, source).unwrap();

        let plan = ExperimentPlan::load(&path).unwrap();
        assert_eq!(
            plan.treatments[0].env["BINARY"],
            dir.path().join("bin/candidate").to_string_lossy()
        );

        let run_dir = dir.path().join("run");
        fs::create_dir(&run_dir).unwrap();
        ExperimentPlan::archive(&path, &run_dir).unwrap();
        assert_eq!(
            fs::read_to_string(run_dir.join("experiment-source.toml")).unwrap(),
            source
        );
        assert!(
            fs::read_to_string(run_dir.join("experiment.toml"))
                .unwrap()
                .contains(
                    &dir.path()
                        .join("bin/candidate")
                        .to_string_lossy()
                        .to_string()
                )
        );
    }
}
