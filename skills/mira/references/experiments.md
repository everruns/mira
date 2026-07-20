# Experiment plans

Use an external experiment plan when a user wants to compare temporary candidates without changing the durable study definition.

Keep in the **study**:

- cases and sample tags;
- scorers and targets;
- meaningful behavioral axes;
- reusable presets.

Keep in the **experiment plan**:

- selected samples and targets;
- trial count;
- short-lived treatment names;
- treatment-specific environment configuration;
- baseline designation.

## Template

```toml
study = "my-study"
launcher = "my-launcher"
preset = "optional-study-preset"

[slice]
samples = ["case-one", "case-two"]
targets = ["provider/model"]
trials = 3

[[treatments]]
name = "baseline"
env.CANDIDATE_BIN = "./bin/current"

[[treatments]]
name = "candidate"
env.CANDIDATE_BIN = "./bin/candidate"

[compare]
group_by = "treatment"
baseline = "baseline"
```

Run it with the normal CLI:

```bash
mira run --experiment experiment.toml
```

Do not combine `--experiment` with direct run selectors such as `--preset`, `--sample`, `--target`, or `--trials`; represent them in the plan. Values beginning with `./` or `../` resolve relative to the manifest.

Each treatment creates an ordinary saved run. Mira archives the source and resolved plan and records the shared experiment ID, treatment, and baseline marker. Do not put secrets in the plan because it is archived.

Current limitation: the baseline is recorded for later comparison, but the plan does not yet enforce automated comparison gates or hash referenced binaries.
