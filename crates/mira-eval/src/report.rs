//! Reporting. Renders the host's collected [`RunResult`]s as a terminal grid, a
//! canonical JSON record, a JUnit XML file (for CI test UIs), a Markdown summary
//! (for PR job summaries), or a self-contained HTML transcript viewer. The JSON
//! record is the machine-readable artifact the HTML viewer and trend aggregation
//! consume.

use crate::protocol::RunResult;

/// Output format selected on the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Junit,
    Markdown,
    /// Self-contained, dependency-free HTML report (the transcript viewer).
    Html,
}

impl std::str::FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Format::Json),
            "junit" | "xml" => Ok(Format::Junit),
            "md" | "markdown" => Ok(Format::Markdown),
            "html" => Ok(Format::Html),
            other => Err(format!("unknown format: {other} (json|junit|md|html)")),
        }
    }
}

impl Format {
    /// The conventional file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Junit => "xml",
            Format::Markdown => "md",
            Format::Html => "html",
        }
    }
}

/// Render `results` in `format` to a string.
pub fn render(results: &[RunResult], format: Format) -> String {
    match format {
        Format::Json => serde_json::to_string_pretty(&results_json(results)).unwrap_or_default(),
        Format::Junit => junit_xml(results),
        Format::Markdown => markdown(results),
        Format::Html => html(results),
    }
}

/// `[k=v,…]` for a cell's axis params, or empty when there are none.
fn params_suffix(params: &crate::Params) -> String {
    if params.is_empty() {
        return String::new();
    }
    format!(
        "[{}]",
        params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Print a per-case list, a model×eval matrix, and totals to stdout.
pub fn print_results(results: &[RunResult]) {
    println!("\n── cases ──");
    for r in results {
        let suffix = params_suffix(&r.params);
        if r.skipped {
            println!("  [SKIP] {}/{}@{}{suffix}", r.eval, r.sample, r.model);
            continue;
        }
        if is_na(r) {
            // Every score N/A — nothing could be evaluated (e.g. an infra error).
            let why = r
                .transcript
                .error
                .as_deref()
                .or_else(|| r.scores.first().map(|s| s.reason.as_str()))
                .unwrap_or("not evaluated");
            println!(
                "  [N/A]  {}/{}@{}{suffix}  ({why})",
                r.eval, r.sample, r.model
            );
            continue;
        }
        let mark = if r.passed { "PASS" } else { "FAIL" };
        println!(
            "  [{mark}] {}/{}@{}{suffix}  ({:.0}%)",
            r.eval,
            r.sample,
            r.model,
            r.aggregate * 100.0
        );
        let t = &r.transcript;
        let mut metrics = vec![format!("{} tok", t.usage.total_tokens())];
        if t.usage.cost_usd > 0.0 {
            metrics.push(format!("${:.4}", t.usage.cost_usd));
        }
        if t.timing.duration_ms > 0 {
            metrics.push(format!("{}ms", t.timing.duration_ms));
        }
        if t.tool_calls_count > 0 {
            metrics.push(format!("{} tool calls", t.tool_calls_count));
        }
        println!("         · {}", metrics.join(" · "));
        for s in &r.scores {
            let m = if s.na {
                "–"
            } else if s.pass {
                "✓"
            } else {
                "✗"
            };
            println!("         {m} {} — {}", s.scorer, s.reason);
        }
    }

    print_matrix(results);

    let scored: Vec<_> = results.iter().filter(|r| !r.skipped && !is_na(r)).collect();
    let passed = scored.iter().filter(|r| r.passed).count();
    let na = results.iter().filter(|r| !r.skipped && is_na(r)).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    println!(
        "\n{} passed / {} scored ({} failed, {} n/a, {} skipped)",
        passed,
        scored.len(),
        scored.len() - passed,
        na,
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

    println!("\n── matrix (passed/scored) ──");
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

/// The `passed/scored` cell for one (eval, model), or `—` if absent. Skipped and
/// all-N/A cells are excluded from the denominator (see [`is_na`]).
fn cell(results: &[RunResult], eval: &str, model: &str) -> String {
    let cells: Vec<_> = results
        .iter()
        .filter(|r| r.eval == eval && r.model == model && !r.skipped && !is_na(r))
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

/// Canonical machine-readable JSON record over the collected results. Carries
/// per-case usage/timing (each `RunResult.transcript`) plus rolled-up totals,
/// so the HTML viewer and trend aggregation consume one stable shape.
pub fn results_json(results: &[RunResult]) -> serde_json::Value {
    serde_json::json!({
        "summary": crate::run::RunSummary::of(results),
        "cases": results,
    })
}

/// True when a cell ran but every score was N/A — nothing could be evaluated
/// (an unreachable judge, or an infrastructure failure that short-circuited
/// scoring). Such a cell is **neither passed nor failed**: it's excluded from
/// the pass-rate, like a skip, so infra hiccups don't masquerade as real
/// failures. The host retries infra-errored cells; one that stays N/A is
/// reported as such rather than counted against the model.
pub fn is_na(r: &RunResult) -> bool {
    !r.scores.is_empty() && r.scores.iter().all(|s| s.na)
}

/// JUnit XML: one `<testcase>` per cell (`eval` ⇒ classname, `sample@model` ⇒
/// name), a failed cell carries `<failure>` with the failing scorers, a skipped
/// cell carries `<skipped>`. A cell that was not executed or whose scores are
/// all N/A counts as skipped. Surfaces evals in any CI that understands JUnit.
pub fn junit_xml(results: &[RunResult]) -> String {
    let is_skipped = |r: &RunResult| r.skipped || is_na(r);
    let skipped = results.iter().filter(|r| is_skipped(r)).count();
    let failures = results
        .iter()
        .filter(|r| !is_skipped(r) && !r.passed)
        .count();

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
            "  <testcase classname=\"{}\" name=\"{}@{}{}\">",
            xml_escape(&r.eval),
            xml_escape(&r.sample),
            xml_escape(&r.model),
            xml_escape(&params_suffix(&r.params)),
        ));
        if is_skipped(r) {
            out.push_str("\n    <skipped/>\n  ");
        } else if !r.passed {
            let reasons: Vec<String> = r
                .scores
                .iter()
                .filter(|s| !s.pass && !s.na)
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
    let scored = results.iter().filter(|r| !r.skipped && !is_na(r)).count();
    let passed = results
        .iter()
        .filter(|r| !r.skipped && !is_na(r) && r.passed)
        .count();
    let na = results.iter().filter(|r| !r.skipped && is_na(r)).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    out.push_str(&format!(
        "## Mira eval results\n\n**{passed} / {scored} passed** ({} failed, {na} n/a, {skipped} skipped)\n\n",
        scored - passed,
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A self-contained, dependency-free HTML report — the transcript viewer. One
/// file: inline CSS, no external assets, the full JSON record embedded for
/// programmatic consumers. Renders a summary banner, the eval×model matrix, and
/// a per-case breakdown (scores, usage, timing, tools, metadata links, error,
/// final response). Open it straight from a CI artifact.
pub fn html(results: &[RunResult]) -> String {
    let (evals, models) = axes(results);
    let scored = results.iter().filter(|r| !r.skipped && !is_na(r)).count();
    let passed = results
        .iter()
        .filter(|r| !r.skipped && !is_na(r) && r.passed)
        .count();
    let na = results.iter().filter(|r| !r.skipped && is_na(r)).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    let failed = scored - passed;
    let total_tokens: u64 = results
        .iter()
        .filter(|r| !r.skipped)
        .map(|r| r.transcript.usage.total_tokens())
        .sum();
    let total_cost: f64 = results
        .iter()
        .filter(|r| !r.skipped)
        .map(|r| r.transcript.usage.cost_usd)
        .sum();

    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str("<title>Mira eval report</title>\n<style>\n");
    out.push_str(CSS);
    out.push_str("</style>\n</head>\n<body>\n<main>\n");
    out.push_str("<h1>Mira eval report</h1>\n");

    // Summary banner.
    out.push_str("<section class=\"cards\">\n");
    // Green when nothing failed; a warn tint when only infra/N/A remains.
    let banner = if failed > 0 {
        "bad"
    } else if na > 0 {
        "warn"
    } else {
        "ok"
    };
    out.push_str(&format!(
        "<div class=\"card {banner}\"><b>{passed}/{scored}</b><span>passed</span></div>\n"
    ));
    out.push_str(&format!(
        "<div class=\"card\"><b>{failed}</b><span>failed</span></div>\n"
    ));
    out.push_str(&format!(
        "<div class=\"card\"><b>{na}</b><span>n/a</span></div>\n"
    ));
    out.push_str(&format!(
        "<div class=\"card\"><b>{skipped}</b><span>skipped</span></div>\n"
    ));
    out.push_str(&format!(
        "<div class=\"card\"><b>{total_tokens}</b><span>tokens</span></div>\n"
    ));
    out.push_str(&format!(
        "<div class=\"card\"><b>${total_cost:.4}</b><span>cost</span></div>\n"
    ));
    out.push_str("</section>\n");

    // Matrix.
    if !evals.is_empty() && !models.is_empty() {
        out.push_str("<h2>Matrix</h2>\n<table class=\"matrix\">\n<thead><tr><th>eval</th>");
        for m in &models {
            out.push_str(&format!("<th>{}</th>", html_escape(m)));
        }
        out.push_str("</tr></thead>\n<tbody>\n");
        for eval in &evals {
            out.push_str(&format!("<tr><td>{}</td>", html_escape(eval)));
            for model in &models {
                out.push_str(&format!(
                    "<td>{}</td>",
                    html_escape(&cell(results, eval, model))
                ));
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n</table>\n");
    }

    // Per-case detail.
    out.push_str("<h2>Cases</h2>\n");
    for r in results {
        let cls = if r.skipped {
            "skip"
        } else if is_na(r) {
            "na"
        } else if r.passed {
            "pass"
        } else {
            "fail"
        };
        let badge = if r.skipped {
            "SKIP"
        } else if is_na(r) {
            "N/A"
        } else if r.passed {
            "PASS"
        } else {
            "FAIL"
        };
        out.push_str(&format!("<details class=\"case {cls}\">\n<summary>"));
        out.push_str(&format!(
            "<span class=\"badge {cls}\">{badge}</span> <code>{}/{}@{}{}</code>",
            html_escape(&r.eval),
            html_escape(&r.sample),
            html_escape(&r.model),
            html_escape(&params_suffix(&r.params)),
        ));
        let t = &r.transcript;
        out.push_str(&format!(
            "<span class=\"metrics\">{} tok · ${:.4} · {}ms · {} tool calls</span>",
            t.usage.total_tokens(),
            t.usage.cost_usd,
            t.timing.duration_ms,
            t.tool_calls_count,
        ));
        out.push_str("</summary>\n");

        if !r.scores.is_empty() {
            out.push_str("<ul class=\"scores\">\n");
            for s in &r.scores {
                let (m, scls) = if s.na {
                    ("–", "na")
                } else if s.pass {
                    ("✓", "pass")
                } else {
                    ("✗", "fail")
                };
                out.push_str(&format!(
                    "<li class=\"{scls}\">{m} <b>{}</b> — {}</li>\n",
                    html_escape(&s.scorer),
                    html_escape(&s.reason),
                ));
            }
            out.push_str("</ul>\n");
        }
        if !t.tool_calls.is_empty() {
            out.push_str(&format!(
                "<p class=\"tools\"><b>tools:</b> {}</p>\n",
                html_escape(&t.tool_calls.join(", "))
            ));
        }
        if !t.metrics.is_empty() {
            out.push_str("<p class=\"meta\"><b>metrics:</b> ");
            let items: Vec<String> = t
                .metrics
                .iter()
                .map(|(k, v)| format!("{}={}", html_escape(k), v))
                .collect();
            out.push_str(&items.join(", "));
            out.push_str("</p>\n");
        }
        if !t.metadata.is_empty() {
            out.push_str("<p class=\"meta\"><b>metadata:</b> ");
            let items: Vec<String> = t
                .metadata
                .iter()
                .map(|(k, v)| {
                    let v = crate::metadata_display(v);
                    if v.starts_with("http://") || v.starts_with("https://") {
                        format!(
                            "{}=<a href=\"{}\">link</a>",
                            html_escape(k),
                            html_escape(&v)
                        )
                    } else {
                        format!("{}={}", html_escape(k), html_escape(&v))
                    }
                })
                .collect();
            out.push_str(&items.join(", "));
            out.push_str("</p>\n");
        }
        if let Some(err) = &t.error {
            out.push_str(&format!(
                "<pre class=\"error\">{}</pre>\n",
                html_escape(err)
            ));
        }
        if !t.final_response.is_empty() {
            out.push_str(&format!(
                "<pre class=\"response\">{}</pre>\n",
                html_escape(&t.final_response)
            ));
        }
        out.push_str("</details>\n");
    }

    // Embed the canonical JSON record for programmatic consumers.
    let json = serde_json::to_string(&results_json(results)).unwrap_or_default();
    out.push_str("<script type=\"application/json\" id=\"mira-data\">\n");
    out.push_str(&json.replace("</", "<\\/"));
    out.push_str("\n</script>\n");
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

const CSS: &str = "\
:root{color-scheme:light dark;--ok:#1a7f37;--bad:#cf222e;--warn:#9a6700;--mut:#57606a;--bg:#fff;--fg:#1f2328;--line:#d0d7de}
@media(prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--line:#30363d;--mut:#8b949e}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:960px;margin:0 auto;padding:2rem 1.25rem}
h1{font-size:1.6rem;margin:0 0 1rem}h2{font-size:1.15rem;margin:2rem 0 .75rem;border-bottom:1px solid var(--line);padding-bottom:.25rem}
.cards{display:flex;flex-wrap:wrap;gap:.75rem}
.card{flex:1;min-width:96px;border:1px solid var(--line);border-radius:8px;padding:.75rem 1rem;text-align:center}
.card b{display:block;font-size:1.5rem}.card span{color:var(--mut);font-size:.8rem;text-transform:uppercase;letter-spacing:.04em}
.card.ok b{color:var(--ok)}.card.bad b{color:var(--bad)}.card.warn b{color:var(--warn)}
table.matrix{border-collapse:collapse;width:100%}table.matrix th,table.matrix td{border:1px solid var(--line);padding:.4rem .6rem;text-align:center}
table.matrix td:first-child,table.matrix th:first-child{text-align:left}
.case{border:1px solid var(--line);border-radius:8px;margin:.5rem 0;padding:.25rem .75rem}
.case summary{cursor:pointer;display:flex;gap:.6rem;align-items:center;flex-wrap:wrap}
.case code{font-size:.92rem}.metrics{color:var(--mut);font-size:.8rem;margin-left:auto}
.badge{font-size:.7rem;font-weight:700;padding:.1rem .4rem;border-radius:4px;color:#fff}
.badge.pass{background:var(--ok)}.badge.fail{background:var(--bad)}.badge.skip{background:var(--mut)}.badge.na{background:var(--warn)}
.scores{list-style:none;padding:0;margin:.5rem 0}.scores li{padding:.15rem 0}.scores li.pass{color:var(--ok)}.scores li.fail{color:var(--bad)}.scores li.na{color:var(--mut)}
.tools,.meta{font-size:.85rem;color:var(--mut)}
pre{background:rgba(127,127,127,.1);border-radius:6px;padding:.6rem .75rem;overflow:auto;font-size:.85rem;white-space:pre-wrap}
pre.error{color:var(--bad)}a{color:#0969da}
";

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
            params: Default::default(),
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
        assert_eq!(v["summary"]["scored"], 2);
        assert_eq!(v["summary"]["passed"], 1);
        assert_eq!(v["summary"]["failed"], 1);
        assert_eq!(v["summary"]["na"], 0);
        assert_eq!(v["summary"]["skipped"], 1);
    }

    #[test]
    fn all_na_cell_counts_as_na_not_failed() {
        // pass, all-N/A (infra), skip → the N/A cell is excluded from pass/fail
        // and surfaced in its own bucket across every reporter.
        let mut na = result("greet", "hi", "opus", false, false);
        na.scores = vec![Score::na("infra", "provider 503")];
        let results = vec![
            result("greet", "hi", "sim", true, false),
            na,
            result("code", "a", "opus", false, true),
        ];
        let v = results_json(&results);
        assert_eq!(v["summary"]["scored"], 1); // only the sim pass is scored
        assert_eq!(v["summary"]["passed"], 1);
        assert_eq!(v["summary"]["failed"], 0); // the N/A cell is not a failure
        assert_eq!(v["summary"]["na"], 1);
        assert_eq!(v["summary"]["skipped"], 1);

        assert!(markdown(&results).contains("1 n/a"));
        let h = html(&results);
        assert!(h.contains("N/A"));
        assert!(h.contains("<span>n/a</span>"));
        assert!(h.contains("card warn")); // not green when only N/A remains
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
    fn junit_all_na_cell_is_skipped_not_failed() {
        // A cell that ran but whose only score is N/A must not emit an empty
        // <failure>; it counts as skipped instead.
        let mut r = result("greet", "hi", "opus", false, false);
        r.scores = vec![Score::na("judge", "unreachable")];
        let xml = junit_xml(std::slice::from_ref(&r));
        assert!(xml.contains("failures=\"0\""));
        assert!(xml.contains("skipped=\"1\""));
        assert!(xml.contains("<skipped/>"));
        assert!(!xml.contains("<failure"));
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
        assert_eq!(Format::from_str("html").unwrap(), Format::Html);
        assert_eq!(Format::Html.extension(), "html");
        assert!(Format::from_str("yaml").is_err());
    }

    #[test]
    fn html_is_self_contained() {
        let h = html(&sample_results());
        assert!(h.starts_with("<!doctype html>"));
        assert!(h.contains("Mira eval report"));
        assert!(h.contains("greet/hi@sim"));
        assert!(h.contains("id=\"mira-data\""));
        // No external asset references.
        assert!(!h.contains("http-equiv"));
        assert!(!h.contains("<link"));
        assert!(!h.contains("src=\"http"));
    }

    #[test]
    fn params_show_in_labels() {
        let mut r = result("greet", "hi", "sim", true, false);
        r.params.insert("effort".into(), "high".into());
        let xml = junit_xml(std::slice::from_ref(&r));
        assert!(xml.contains("name=\"hi@sim[effort=high]\""));
        let h = html(std::slice::from_ref(&r));
        assert!(h.contains("greet/hi@sim[effort=high]"));
    }
}
