# Experiment plans

An experiment plan composes short-lived treatments with a durable study. Treatment names become run-level comparison metadata; they do not become study axes or catalog entries.

```toml
# prompt-experiment.toml
study = "harness_basic"
launcher = "harness-basic"
preset = "prompt-regression"

[slice]
samples = ["approval", "instruction-precedence", "add-fn"]
targets = ["openai/gpt-6.2-sol"]
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

Mira attempts all treatments before evaluating the experiment outcome. With
`[compare]`, each candidate is paired with the baseline. The default gate allows
zero logical-case pass-rate regressions. Optional `compare.filter` and
`compare.scorer` select a case slice and scoring dimension. An explicit
`[compare.gates]` table replaces the default limits and accepts `max_regressions`,
`max_score_drop`, `max_cost_increase`, and `max_latency_increase`. Relative
operational limits use fractions (`0.10` means 10%).

Each treatment can declare `files = ["./binary", "./prompt.txt"]`; these paths
resolve relative to the plan and their SHA-256 hashes are saved in
`treatment-files.json`. Changed declared files or plans cannot be overwritten
when resuming a treatment run.

The experiment writes `results/experiments/<experiment_id>/comparison.json`.
Quality failures do not stop subsequent treatments. Missing or incomplete runs
and failed comparison gates fail the experiment. Without a comparison, any
unsuccessful treatment fails the experiment after all have been attempted.
See [Evaluation quality](evaluation-quality.md) for comparison semantics.
