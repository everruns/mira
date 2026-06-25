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

/// A `--group-by` view over a result set: the metadata key, plus the resolved
/// group value for each case (parallel to the results; `None` ⇒ the case had no
/// value for the key). The host resolves the values (it owns the listing join);
/// the reporters only bucket and render, so this module stays pure.
#[derive(Clone, Copy)]
pub struct Group<'a> {
    pub key: &'a str,
    pub values: &'a [Option<String>],
}

/// Render `results` in `format` to a string.
pub fn render(results: &[RunResult], format: Format) -> String {
    render_with_group(results, format, None)
}

/// Like [`render`], but also folds a `--group-by` breakdown into formats that can
/// carry it: a `groups` block in the JSON record (and the HTML viewer's embedded
/// copy), a section in Markdown, and a table in HTML. JUnit is unaffected.
pub fn render_with_group(
    results: &[RunResult],
    format: Format,
    group: Option<Group<'_>>,
) -> String {
    match format {
        Format::Json => {
            serde_json::to_string_pretty(&results_json_with_group(results, group.as_ref()))
                .unwrap_or_default()
        }
        Format::Junit => junit_xml(results),
        Format::Markdown => {
            let mut out = markdown(results);
            if let Some(g) = group {
                out.push_str(&group_markdown(results, g.key, g.values));
            }
            out
        }
        Format::Html => html_with_group(results, group),
    }
}

/// `[k=v,…]` for a case's axis params, or empty when there are none.
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

/// The full per-row key suffix: axis params (`[k=v,…]`) then the `#trial` index
/// when this case was repeated, so every trial's row is individually identifiable.
fn case_suffix(r: &RunResult) -> String {
    format!(
        "{}{}",
        params_suffix(&r.params),
        crate::trial_suffix(r.trial, r.trials)
    )
}

/// Print a per-case list, a target×eval matrix, and totals to stdout.
pub fn print_results(results: &[RunResult]) {
    println!("\n── cases ──");
    for r in results {
        let suffix = case_suffix(r);
        if r.skipped {
            println!("  [SKIP] {}/{}@{}{suffix}", r.eval, r.sample, r.target);
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
                r.eval, r.sample, r.target
            );
            continue;
        }
        let mark = if r.passed { "PASS" } else { "FAIL" };
        println!(
            "  [{mark}] {}/{}@{}{suffix}  ({:.0}%)",
            r.eval,
            r.sample,
            r.target,
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
    print_trials(results);

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

/// One row of a `--group-by` breakdown: a group value and its pass/scored tally.
pub struct GroupRate {
    pub value: String,
    pub passed: usize,
    pub scored: usize,
}

impl GroupRate {
    /// Pass rate in `[0,1]` (0 when nothing was scored).
    pub fn rate(&self) -> f64 {
        if self.scored == 0 {
            0.0
        } else {
            self.passed as f64 / self.scored as f64
        }
    }
}

/// Resolve-rate bucketed by a pre-resolved per-case group value (`values`
/// parallel to `results`; `None` ⇒ the `(unset)` bucket). Skipped and all-N/A
/// cases are excluded from `scored`, exactly like the matrix, so infra hiccups
/// don't skew a group's rate. Buckets are ordered by value, `(unset)` last.
pub fn group_rates(results: &[RunResult], values: &[Option<String>]) -> Vec<GroupRate> {
    use std::collections::BTreeMap;
    // `values` is resolved per result by the host and must be parallel to it;
    // a mismatch would silently truncate the tally via `zip`. Fail fast in debug.
    debug_assert_eq!(
        results.len(),
        values.len(),
        "group_rates: values must be parallel to results"
    );
    let mut buckets: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut unset = (0usize, 0usize);
    for (r, value) in results.iter().zip(values) {
        if r.skipped || is_na(r) {
            continue;
        }
        let entry = match value {
            Some(v) => buckets.entry(v.clone()).or_default(),
            None => &mut unset,
        };
        entry.1 += 1;
        if r.passed {
            entry.0 += 1;
        }
    }
    let mut out: Vec<GroupRate> = buckets
        .into_iter()
        .map(|(value, (passed, scored))| GroupRate {
            value,
            passed,
            scored,
        })
        .collect();
    if unset.1 > 0 {
        out.push(GroupRate {
            value: "(unset)".into(),
            passed: unset.0,
            scored: unset.1,
        });
    }
    out
}

/// Print a `--group-by` breakdown (resolve rate per group value) to stdout.
pub fn print_group_breakdown(results: &[RunResult], key: &str, values: &[Option<String>]) {
    let rates = group_rates(results, values);
    if rates.is_empty() {
        return;
    }
    println!("\n── resolve rate by {key} (passed/scored) ──");
    let w = rates
        .iter()
        .map(|r| r.value.len())
        .max()
        .unwrap_or(0)
        .max(key.len());
    for r in &rates {
        println!(
            "  {:w$}  {}/{}  ({:.0}%)",
            r.value,
            r.passed,
            r.scored,
            r.rate() * 100.0,
            w = w,
        );
    }
}

/// Escape a value for a Markdown table case: a `|` would start a new column and a
/// newline would break the row. Group values come from free-form metadata, so
/// neutralize both before rendering (otherwise a stray `|` corrupts the table and
/// any CI job summary built from it).
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\r', '\n'], " ")
}

/// Markdown section for a `--group-by` breakdown (empty when nothing scored).
fn group_markdown(results: &[RunResult], key: &str, values: &[Option<String>]) -> String {
    let rates = group_rates(results, values);
    if rates.is_empty() {
        return String::new();
    }
    let key = md_cell(key);
    let mut out = format!(
        "\n### Resolve rate by {key}\n\n| {key} | passed | scored | rate |\n|---|---|---|---|\n"
    );
    for r in &rates {
        out.push_str(&format!(
            "| {} | {} | {} | {:.0}% |\n",
            md_cell(&r.value),
            r.passed,
            r.scored,
            r.rate() * 100.0
        ));
    }
    out
}

/// The `groups` block for the JSON record: `{ key: { value: {passed, scored} } }`.
fn groups_json(results: &[RunResult], key: &str, values: &[Option<String>]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for r in group_rates(results, values) {
        map.insert(
            r.value,
            serde_json::json!({ "passed": r.passed, "scored": r.scored }),
        );
    }
    serde_json::json!({ key: serde_json::Value::Object(map) })
}

/// Distinct evals and targets, in first-seen order.
fn axes(results: &[RunResult]) -> (Vec<String>, Vec<String>) {
    let mut evals: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for r in results {
        if !evals.contains(&r.eval) {
            evals.push(r.eval.clone());
        }
        if !targets.contains(&r.target) {
            targets.push(r.target.clone());
        }
    }
    (evals, targets)
}

/// A compact pass-rate grid: evals down the side, targets across the top.
fn print_matrix(results: &[RunResult]) {
    let (evals, targets) = axes(results);
    if evals.is_empty() || targets.is_empty() {
        return;
    }

    println!("\n── matrix (passed/scored) ──");
    let label_w = evals.iter().map(|e| e.len()).max().unwrap_or(4).max(4);
    let col_w = targets.iter().map(|m| m.len()).max().unwrap_or(6).max(7);

    print!("  {:label_w$}", "eval", label_w = label_w);
    for m in &targets {
        print!("  {:>col_w$}", m, col_w = col_w);
    }
    println!();

    for eval in &evals {
        print!("  {:label_w$}", eval, label_w = label_w);
        for target in &targets {
            print!("  {:>col_w$}", case(results, eval, target), col_w = col_w);
        }
        println!();
    }
}

/// When any case was repeated (trials > 1), print a per-case aggregation: trials
/// passed/scored, pass-rate, the pass@k spread, and the score's standard
/// deviation (the reproducibility signal). Silent when nothing was repeated.
fn print_trials(results: &[RunResult]) {
    let aggs: Vec<_> = crate::aggregate::aggregate_trials(results)
        .into_iter()
        .filter(|a| a.repeated())
        .collect();
    if aggs.is_empty() {
        return;
    }
    println!("\n── trials (pass@k over repetitions) ──");
    let key_w = aggs.iter().map(|a| a.key.len()).max().unwrap_or(4).max(4);
    for a in &aggs {
        // pass@1 and pass@n bracket the spread; n is the scored trial count.
        let n = a.scored;
        let mut parts = vec![
            format!("{}/{} pass", a.passed, a.scored),
            format!("rate {:.0}%", a.pass_rate * 100.0),
            format!("pass@1 {:.2}", a.pass_at_k(1)),
        ];
        if n > 1 {
            parts.push(format!("pass@{n} {:.2}", a.pass_at_k(n)));
        }
        parts.push(format!("σ {:.3}", a.std_dev));
        if a.na > 0 || a.skipped > 0 {
            parts.push(format!("({} n/a, {} skip)", a.na, a.skipped));
        }
        println!("  {:key_w$}  {}", a.key, parts.join(" · "), key_w = key_w);
    }
}

/// The `passed/scored` case for one (eval, target), or `—` if absent. Skipped and
/// all-N/A cases are excluded from the denominator (see [`is_na`]).
fn case(results: &[RunResult], eval: &str, target: &str) -> String {
    let cases: Vec<_> = results
        .iter()
        .filter(|r| r.eval == eval && r.target == target && !r.skipped && !is_na(r))
        .collect();
    if cases.is_empty() {
        "—".to_string()
    } else {
        format!(
            "{}/{}",
            cases.iter().filter(|r| r.passed).count(),
            cases.len()
        )
    }
}

/// Canonical machine-readable JSON record over the collected results. Carries
/// per-case usage/timing (each `RunResult.transcript`) plus rolled-up totals,
/// so the HTML viewer and trend aggregation consume one stable shape. When any
/// case was repeated (trials > 1), a `trials` array of per-case
/// [`TrialAggregate`](crate::aggregate::TrialAggregate)s (pass@k / pass-rate /
/// variance) is added.
pub fn results_json(results: &[RunResult]) -> serde_json::Value {
    let mut record = serde_json::json!({
        "summary": crate::run::RunSummary::of(results),
        "cases": results,
    });
    if crate::aggregate::has_trials(results) {
        let trials = crate::aggregate::aggregate_trials(results)
            .into_iter()
            .filter(|a| a.repeated())
            .collect::<Vec<_>>();
        // Fail loudly: a TrialAggregate is plain owned data, so serialization
        // can't fail — turning it into `null` would only mask a real bug.
        record["trials"] = serde_json::to_value(trials).expect("trial aggregates serialize");
    }
    record
}

/// [`results_json`] plus an optional `groups` block (resolve rate per group
/// value) when a `--group-by` view is supplied.
pub fn results_json_with_group(
    results: &[RunResult],
    group: Option<&Group<'_>>,
) -> serde_json::Value {
    let mut record = results_json(results);
    if let Some(g) = group
        && let Some(obj) = record.as_object_mut()
    {
        obj.insert("groups".into(), groups_json(results, g.key, g.values));
    }
    record
}

/// True when a case ran but every score was N/A — nothing could be evaluated
/// (an unreachable judge, or an infrastructure failure that short-circuited
/// scoring). Such a case is **neither passed nor failed**: it's excluded from
/// the pass-rate, like a skip, so infra hiccups don't masquerade as real
/// failures. The host retries infra-errored cases; one that stays N/A is
/// reported as such rather than counted against the target.
pub fn is_na(r: &RunResult) -> bool {
    !r.scores.is_empty() && r.scores.iter().all(|s| s.na)
}

/// JUnit XML: one `<testcase>` per case (`eval` ⇒ classname, `sample@target` ⇒
/// name), a failed case carries `<failure>` with the failing scorers, a skipped
/// case carries `<skipped>`. A case that was not executed or whose scores are
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
            xml_escape(&r.target),
            xml_escape(&case_suffix(r)),
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
    let (evals, targets) = axes(results);
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
    if evals.is_empty() || targets.is_empty() {
        return out;
    }
    out.push_str("| eval |");
    for m in &targets {
        out.push_str(&format!(" {m} |"));
    }
    out.push_str("\n|---|");
    for _ in &targets {
        out.push_str("---|");
    }
    out.push('\n');
    for eval in &evals {
        out.push_str(&format!("| {eval} |"));
        for target in &targets {
            out.push_str(&format!(" {} |", case(results, eval, target)));
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
/// programmatic consumers. Renders a summary banner, the eval×target matrix, and
/// a per-case breakdown (scores, usage, timing, tools, metadata links, error,
/// final response). Open it straight from a CI artifact.
pub fn html(results: &[RunResult]) -> String {
    html_with_group(results, None)
}

/// [`html`] with an optional `--group-by` table and a group-aware embedded JSON
/// record.
pub fn html_with_group(results: &[RunResult], group: Option<Group<'_>>) -> String {
    let (evals, targets) = axes(results);
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
    if !evals.is_empty() && !targets.is_empty() {
        out.push_str("<h2>Matrix</h2>\n<table class=\"matrix\">\n<thead><tr><th>eval</th>");
        for m in &targets {
            out.push_str(&format!("<th>{}</th>", html_escape(m)));
        }
        out.push_str("</tr></thead>\n<tbody>\n");
        for eval in &evals {
            out.push_str(&format!("<tr><td>{}</td>", html_escape(eval)));
            for target in &targets {
                out.push_str(&format!(
                    "<td>{}</td>",
                    html_escape(&case(results, eval, target))
                ));
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n</table>\n");
    }

    // Group-by breakdown (resolve rate per metadata group value).
    if let Some(g) = &group {
        let rates = group_rates(results, g.values);
        if !rates.is_empty() {
            out.push_str(&format!(
                "<h2>Resolve rate by {}</h2>\n<table class=\"matrix\">\n\
                 <thead><tr><th>{}</th><th>passed</th><th>scored</th><th>rate</th></tr></thead>\n<tbody>\n",
                html_escape(g.key),
                html_escape(g.key),
            ));
            for r in &rates {
                out.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{:.0}%</td></tr>\n",
                    html_escape(&r.value),
                    r.passed,
                    r.scored,
                    r.rate() * 100.0,
                ));
            }
            out.push_str("</tbody>\n</table>\n");
        }
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
            html_escape(&r.target),
            html_escape(&case_suffix(r)),
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
    let json = serde_json::to_string(&results_json_with_group(results, group.as_ref()))
        .unwrap_or_default();
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

    fn result(eval: &str, sample: &str, target: &str, passed: bool, skipped: bool) -> RunResult {
        RunResult {
            eval: eval.into(),
            sample: sample.into(),
            target: target.into(),
            params: Default::default(),
            trial: 0,
            trials: 0,
            seed: None,
            input: Vec::new(),
            expected: None,
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
    fn all_na_case_counts_as_na_not_failed() {
        // pass, all-N/A (infra), skip → the N/A case is excluded from pass/fail
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
        assert_eq!(v["summary"]["failed"], 0); // the N/A case is not a failure
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
    fn junit_all_na_case_is_skipped_not_failed() {
        // A case that ran but whose only score is N/A must not emit an empty
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
    fn trials_aggregate_in_json_and_labels() {
        // Three trials of one case, 2/3 passing.
        let mut results = Vec::new();
        for t in 0..3 {
            let mut r = result("flaky", "a", "sim", t != 0, false);
            r.trial = t;
            r.trials = 3;
            results.push(r);
        }
        let v = results_json(&results);
        let trials = v["trials"].as_array().expect("trials array present");
        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0]["key"], "flaky/a@sim");
        assert_eq!(trials[0]["passed"], 2);
        assert_eq!(trials[0]["scored"], 3);

        // A single-trial run adds no `trials` block.
        let single = vec![result("greet", "hi", "sim", true, false)];
        assert!(results_json(&single).get("trials").is_none());

        // Per-trial rows are individually addressable via the `#index` suffix.
        let xml = junit_xml(&results);
        assert!(xml.contains("name=\"a@sim#0\""));
        assert!(xml.contains("name=\"a@sim#2\""));
        let h = html(&results);
        assert!(h.contains("flaky/a@sim#1"));
    }

    #[test]
    fn group_rates_bucket_and_exclude_skips() {
        // pass@easy, fail@easy, pass@hard, skip (unset). The skip is excluded
        // from scored; the unset bucket only appears when it has scored cases.
        let results = vec![
            result("e", "a", "sim", true, false),
            result("e", "b", "sim", false, false),
            result("e", "c", "sim", true, false),
            result("e", "d", "sim", false, true),
        ];
        let values = vec![
            Some("easy".to_string()),
            Some("easy".to_string()),
            Some("hard".to_string()),
            None,
        ];
        let rates = group_rates(&results, &values);
        assert_eq!(rates.len(), 2); // easy, hard — the skipped (None) case drops out
        assert_eq!(rates[0].value, "easy");
        assert_eq!((rates[0].passed, rates[0].scored), (1, 2));
        assert_eq!(rates[0].rate(), 0.5);
        assert_eq!(rates[1].value, "hard");
        assert_eq!((rates[1].passed, rates[1].scored), (1, 1));
    }

    #[test]
    fn unset_bucket_when_scored() {
        let results = vec![result("e", "a", "sim", true, false)];
        let rates = group_rates(&results, &[None]);
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].value, "(unset)");
        assert_eq!((rates[0].passed, rates[0].scored), (1, 1));
    }

    #[test]
    fn group_renders_into_every_format() {
        let results = vec![
            result("e", "a", "sim", true, false),
            result("e", "b", "sim", false, false),
        ];
        let values = vec![Some("easy".to_string()), Some("hard".to_string())];
        let group = Group {
            key: "difficulty",
            values: &values,
        };

        let md = render_with_group(&results, Format::Markdown, Some(group));
        assert!(md.contains("Resolve rate by difficulty"));
        assert!(md.contains("| easy | 1 | 1 | 100% |"));

        let json = render_with_group(&results, Format::Json, Some(group));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["groups"]["difficulty"]["easy"]["passed"], 1);
        assert_eq!(v["groups"]["difficulty"]["hard"]["scored"], 1);

        let html = render_with_group(&results, Format::Html, Some(group));
        assert!(html.contains("Resolve rate by difficulty"));
        // The embedded JSON record carries the groups too.
        assert!(html.contains("\"groups\""));
    }

    #[test]
    fn group_markdown_escapes_pipe_and_newline() {
        // A free-form metadata value with a pipe / newline must not corrupt the
        // Markdown table (or a CI job summary built from it).
        let results = vec![result("e", "a", "sim", true, false)];
        let values = vec![Some("a|b\nc".to_string())];
        let md = render_with_group(
            &results,
            Format::Markdown,
            Some(Group {
                key: "repo",
                values: &values,
            }),
        );
        assert!(md.contains("| a\\|b c |"), "got: {md}");
        assert!(!md.contains("a|b"));
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
