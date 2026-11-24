//! Saved-run policies are reproducible artifacts: include the resolved policy
//! in output, require finished runs, and never rewrite the source run.
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct AssessArgs {
    /// Saved run directory or run ID.
    pub run: String,
    /// TOML acceptance rules and/or coverage inventory.
    #[arg(long)]
    pub policy: PathBuf,
    /// JSON destination; omit to print stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}
pub fn run(args: &AssessArgs) -> Result<i32, Box<dyn std::error::Error>> {
    let policy: mira::assessment::Policy = toml::from_str(&std::fs::read_to_string(&args.policy)?)?;
    let report = policy.evaluate(&crate::compare::load(&args.run)?)?;
    for rule in &report.rules {
        eprintln!(
            "{}: {} cases; pass rate {:?}; grading ${:?}; {} incomplete; {} breaches",
            rule.name,
            rule.cases,
            rule.pass_rate,
            rule.grading_cost_usd,
            rule.incomplete.len(),
            rule.failures.len()
        );
    }
    for cell in &report.inventory {
        eprintln!(
            "{} × {}: {}/{} evaluated; {}",
            cell.scenario,
            cell.dimension,
            cell.evaluated,
            cell.planned,
            if cell.covered { "covered" } else { "MISSING" }
        );
    }
    let bytes = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = &args.out {
        std::fs::write(path, bytes)?;
    } else {
        println!("{}", String::from_utf8(bytes)?);
    }
    Ok(report.exit_code())
}
