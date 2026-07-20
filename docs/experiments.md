# Experiment plans

An experiment plan composes short-lived treatments with a durable study. Treatment names become run-level comparison metadata; they do not become study axes or catalog entries.

```toml
# prompt-experiment.toml
study = "harness_basic"
launcher = "harness-basic"
preset = "prompt-regression"

[slice]
samples = ["approval", "instruction-precedence", "add-fn"]
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
```

Run every treatment with:

```bash
mira run --experiment prompt-experiment.toml
```

Mira executes each treatment as a separate ordinary run. Every saved run contains:

- `experiment-source.toml`, the supplied plan;
- `experiment.toml`, the resolved plan, including manifest-relative environment paths;
- `environment.json` labels for `experiment_id`, `treatment`, and, for the designated baseline, `experiment_baseline=true`.

The initialized study name must equal `study`. `launcher` uses the same launcher lookup as `mira run --launcher`; `preset` is applied before the experiment slice. The slice then narrows samples and targets and overrides trial count.

Relative environment values beginning with `./` or `../` are resolved relative to the manifest directory. Other values are passed unchanged. Environment values are not interpreted by Mira, so treatments can configure binaries, prompts, tools, adapters, or feature flags without adding domain-specific study concepts.

Do not put secrets directly in a plan: the original and resolved manifests are archived with every treatment run. Secret environment variables already present in the parent process remain available to launched studies without being copied into the manifest.

The current format records baseline identity but does not yet apply automated comparison gates or hash treatment binaries and input files. Saved runs remain independently reportable and comparable through the normal run interfaces.
