//! Reporting. Renders the host's collected [`RunResult`]s as a terminal grid,
//! a canonical JSON record, a JUnit XML file (for CI test UIs), or a Markdown
//! summary (for PR job summaries). The JSON record is the machine-readable
//! artifact that a future `report.html` viewer and trend aggregation consume.

use crate::protocol::RunResult;

/// Output format selected on the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Junit,
    Markdown,
}

impl std::str::FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Format::Json),
            "junit" | "xml" => Ok(Format::Junit),
            "md" | "markdown" => Ok(Format::Markdown),
            other => Err(format!("unknown format: {other} (json|junit|md)")),
        }
    }
}

/// Render `results` in `format` to a string.
pub fn render(results: &[RunResult], format: Format) -> String {
    match format {
        Format::Json => serde_json::to_string_pretty(&results_json(results)).unwrap_or_default(),
        Format::Junit => junit_xml(results),
        Format::Markdown => markdown(results),
    }
}

/// Print a per-case list, a model×eval matrix, and totals to stdout.
pub fn print_results(results: &[RunResult]) {
    println!("\n── cases ──");
    for r in results {
        if r.skipped {
            println!("  [SKIP] {}/{}@{}", r.eval, r.sample, r.model);
            continue;
        }
        let mark = if r.passed { "PASS" } else { "FAIL" };
        println!(
            "  [{mark}] {}/{}@{}  ({:.0}%)",
            r.eval,
            r.sample,
            r.model,
            r.aggregate * 100.0
        );
        for s in &r.scores {
            let m = if s.pass { "✓" } else { "✗" };
            println!("         {m} {} — {}", s.scorer, s.reason);
        }
    }

    print_matrix(results);

    let ran: Vec<_> = results.iter().filter(|r| !r.skipped).collect();
    let passed = ran.iter().filter(|r| r.passed).count();
    let skipped = results.len() - ran.len();
    println!(
        "\n{} passed / {} ran ({} failed, {} skipped)",
        passed,
        ran.len(),
        ran.len() - passed,
        skipped,
    );
}

/// Distinct evals and models, in first-seen order.
fn axes(results: &[RunResult]) -> (Vec<String>, Vec<String>) {
    let mut evals: Vec<String> = Vec::new();
    let mut models: Vec<String> = Vec::new();
    for r in results {
        if !evals.contains(&r.eval) {
            evals.push(r.eval.clone());
        }
        if !models.contains(&r.model) {
            models.push(r.model.clone());
        }
    }
    (evals, models)
}

/// A compact pass-rate grid: evals down the side, models across the top.
fn print_matrix(results: &[RunResult]) {
    let (evals, models) = axes(results);
    if evals.is_empty() || models.is_empty() {
        return;
    }

    println!("\n── matrix (passed/ran) ──");
    let label_w = evals.iter().map(|e| e.len()).max().unwrap_or(4).max(4);
    let col_w = models.iter().map(|m| m.len()).max().unwrap_or(6).max(7);

    print!("  {:label_w$}", "eval", label_w = label_w);
    for m in &models {
        print!("  {:>col_w$}", m, col_w = col_w);
    }
    println!();

    for eval in &evals {
        print!("  {:label_w$}", eval, label_w = label_w);
        for model in &models {
            print!("  {:>col_w$}", cell(results, eval, model), col_w = col_w);
        }
        println!();
    }
}

/// The `passed/ran` cell for one (eval, model), or `—` if absent.
fn cell(results: &[RunResult], eval: &str, model: &str) -> String {
    let cells: Vec<_> = results
        .iter()
        .filter(|r| r.eval == eval && r.model == model && !r.skipped)
        .collect();
    if cells.is_empty() {
        "—".to_string()
    } else {
        format!(
            "{}/{}",
            cells.iter().filter(|r| r.passed).count(),
            cells.len()
        )
    }
}

/// Canonical machine-readable JSON record over the collected results.
pub fn results_json(results: &[RunResult]) -> serde_json::Value {
    let ran = results.iter().filter(|r| !r.skipped).count();
    let passed = results.iter().filter(|r| !r.skipped && r.passed).count();
    serde_json::json!({
        "summary": {
            "ran": ran,
            "passed": passed,
            "failed": ran - passed,
            "skipped": results.len() - ran,
        },
        "cases": results,
    })
}

/// JUnit XML: one `<testcase>` per cell (`eval` ⇒ classname, `sample@model` ⇒
/// name), a failed cell carries `<failure>` with the failing scorers, a skipped
/// cell carries `<skipped>`. Surfaces evals in any CI that understands JUnit.
pub fn junit_xml(results: &[RunResult]) -> String {
    let ran = results.iter().filter(|r| !r.skipped).count();
    let failures = results.iter().filter(|r| !r.skipped && !r.passed).count();
    let skipped = results.len() - ran;

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuite name=\"mira\" tests=\"{}\" failures=\"{}\" skipped=\"{}\">\n",
        results.len(),
        failures,
        skipped
    ));
    for r in results {
        out.push_str(&format!(
            "  <testcase classname=\"{}\" name=\"{}@{}\">",
            xml_escape(&r.eval),
            xml_escape(&r.sample),
            xml_escape(&r.model),
        ));
        if r.skipped {
            out.push_str("\n    <skipped/>\n  ");
        } else if !r.passed {
            let reasons: Vec<String> = r
                .scores
                .iter()
                .filter(|s| !s.pass)
                .map(|s| format!("{}: {}", s.scorer, s.reason))
                .collect();
            out.push_str(&format!(
                "\n    <failure message=\"{}\"/>\n  ",
                xml_escape(&reasons.join("; "))
            ));
        }
        out.push_str("</testcase>\n");
    }
    out.push_str("</testsuite>\n");
    out
}

/// Markdown summary suitable for a CI job summary / PR comment.
pub fn markdown(results: &[RunResult]) -> String {
    let (evals, models) = axes(results);
    let mut out = String::new();
    let ran = results.iter().filter(|r| !r.skipped).count();
    let passed = results.iter().filter(|r| !r.skipped && r.passed).count();
    out.push_str(&format!(
        "## Mira eval results\n\n**{passed} / {ran} passed** ({} failed, {} skipped)\n\n",
        ran - passed,
        results.len() - ran
    ));
    if evals.is_empty() || models.is_empty() {
        return out;
    }
    out.push_str("| eval |");
    for m in &models {
        out.push_str(&format!(" {m} |"));
    }
    out.push_str("\n|---|");
    for _ in &models {
        out.push_str("---|");
    }
    out.push('\n');
    for eval in &evals {
        out.push_str(&format!("| {eval} |"));
        for model in &models {
            out.push_str(&format!(" {} |", cell(results, eval, model)));
        }
        out.push('\n');
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Score;
    use crate::protocol::TranscriptSummary;

    fn result(eval: &str, sample: &str, model: &str, passed: bool, skipped: bool) -> RunResult {
        RunResult {
            eval: eval.into(),
            sample: sample.into(),
            model: model.into(),
            passed,
            aggregate: if passed { 1.0 } else { 0.0 },
            scores: if passed {
                vec![Score::pass("s", "ok")]
            } else {
                vec![Score::fail("s", "nope")]
            },
            transcript: TranscriptSummary::default(),
            skipped,
        }
    }

    fn sample_results() -> Vec<RunResult> {
        vec![
            result("greet", "hi", "sim", true, false),
            result("greet", "hi", "opus", false, false),
            result("code", "a", "opus", false, true),
        ]
    }

    #[test]
    fn json_summary_counts() {
        let v = results_json(&sample_results());
        assert_eq!(v["summary"]["ran"], 2);
        assert_eq!(v["summary"]["passed"], 1);
        assert_eq!(v["summary"]["failed"], 1);
        assert_eq!(v["summary"]["skipped"], 1);
    }

    #[test]
    fn junit_has_failure_and_skip() {
        let xml = junit_xml(&sample_results());
        assert!(xml.contains("tests=\"3\""));
        assert!(xml.contains("failures=\"1\""));
        assert!(xml.contains("skipped=\"1\""));
        assert!(xml.contains("<failure message=\"s: nope\"/>"));
        assert!(xml.contains("<skipped/>"));
        assert!(xml.contains("name=\"hi@sim\""));
    }

    #[test]
    fn markdown_has_matrix() {
        let md = markdown(&sample_results());
        assert!(md.contains("1 / 2 passed"));
        assert!(md.contains("| greet |"));
    }

    #[test]
    fn xml_escaping() {
        assert_eq!(xml_escape("a<b&\"c\""), "a&lt;b&amp;&quot;c&quot;");
    }

    #[test]
    fn format_parsing() {
        use std::str::FromStr;
        assert_eq!(Format::from_str("json").unwrap(), Format::Json);
        assert_eq!(Format::from_str("JUnit").unwrap(), Format::Junit);
        assert_eq!(Format::from_str("md").unwrap(), Format::Markdown);
        assert!(Format::from_str("yaml").is_err());
    }
}
