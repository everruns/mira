"use strict";
const data = JSON.parse(document.getElementById("data").textContent),
  labels = new Map(),
  $ = (id) => document.getElementById(id);
function el(tag, text, cls) {
  const e = document.createElement(tag);
  if (text !== undefined) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}
function identity(entry) {
  return entry?.result.transcript.metadata?.["mira.execution_sha256"];
}
function verdict(entry, judge) {
  const scores = (entry?.result.scores || []).filter(
    (s) => !judge || s.scorer === judge,
  );
  return scores.length && scores.every((s) => !s.na)
    ? scores.every((s) => s.pass)
    : null;
}
const runName = (path) => path.split(/[\\/]/).filter(Boolean).pop();
$("run").textContent =
  `Candidate: ${runName(data.run)}${data.baseline_run ? " · Baseline: " + runName(data.baseline_run) : ""}`;
for (const name of [
  ...new Set(
    Object.values(data.candidate).flatMap((e) =>
      e.result.scores.map((s) => s.scorer),
    ),
  ),
].sort()) {
  const o = el("option", name);
  o.value = name;
  $("judge").append(o);
}
for (const j of data.calibration?.judgments || [])
  labels.set(j.case_key, j.human_pass);
if (data.comparison?.incomplete?.length) {
  $("notice").hidden = false;
  $("notice").textContent =
    "Comparison evidence incomplete: " + data.comparison.incomplete.join("; ");
}
function panel(entry, side, index) {
  const p = el("section", undefined, "panel");
  p.append(el("h3", side === "candidate" ? "Candidate" : "Baseline"));
  if (!entry) {
    p.append(el("p", "Case missing from this run.", "muted"));
    return p;
  }
  const r = entry.result,
    t = r.transcript;
  p.append(
    el(
      "p",
      `${r.skipped ? "Skipped" : r.scores.length && r.scores.every((s) => s.na) ? "N/A" : r.passed ? "Passed" : "Failed"} · ${t.usage.input_tokens + t.usage.output_tokens} tokens · target $${t.usage.cost_usd} · ${t.timing?.duration_ms ?? "unmeasured"} ms`,
    ),
  );
  for (const s of r.scores) {
    const b = el(
      "button",
      `${s.na ? "N/A" : s.pass ? "PASS" : "FAIL"}${s.na ? "" : ` (${s.value.toFixed(3)})`} ${s.scorer}: ${s.reason}`,
      `score ${s.na ? "na" : s.pass ? "pass" : "fail"}`,
    );
    b.title = "Jump to the cited trajectory step";
    b.onclick = () => {
      const match = s.reason.match(/step_id\s*[=:]?\s*(\d+)/);
      const step =
        match && document.getElementById(`${side}-${index}-${match[1]}`);
      if (step) {
        step.scrollIntoView({ block: "center" });
        step.classList.add("highlight");
      } else {
        const first = p.querySelector(".step");
        if (first) first.scrollIntoView({ block: "center" });
      }
    };
    p.append(b);
    if (s.grading)
      p.append(
        el(
          "small",
          `Grading: ${s.grading.requests} requests · ${s.grading.errors} errors · ${s.grading.input_tokens ?? "?"} in / ${s.grading.output_tokens ?? "?"} out · ${s.grading.duration_ms} ms · ${s.grading.cost_usd == null ? "unpriced" : "$" + s.grading.cost_usd}`,
        ),
      );
  }
  const sample = el("details");
  sample.append(
    el("summary", "Archived sample"),
    el(
      "pre",
      JSON.stringify(
        entry.sample ?? { input: r.input, expected: r.expected },
        null,
        2,
      ),
    ),
  );
  p.append(sample);
  const measured = el("details");
  measured.append(
    el("summary", "Measured evidence and metrics"),
    el(
      "pre",
      JSON.stringify(
        {
          metrics: t.metrics ?? {},
          metadata: Object.fromEntries(
            Object.entries(t.metadata ?? {}).filter(
              ([key]) => !key.startsWith("mira."),
            ),
          ),
        },
        null,
        2,
      ),
    ),
  );
  p.append(measured);
  if (t.error) p.append(el("p", t.error, "fail"));
  p.append(el("h3", "Final response"), el("pre", t.final_response));
  p.append(el("h3", "Trajectory"));
  if (!entry.trajectory)
    p.append(
      el(
        "p",
        "No archived trajectory. Arguments and observations cannot be verified here.",
        "muted",
      ),
    );
  for (const step of entry.trajectory?.steps || []) {
    const d = el("details", undefined, "step");
    d.id = `${side}-${index}-${step.step_id}`;
    d.open = true;
    d.append(
      el("summary", `Step ${step.step_id} · ${step.source}`),
      el("pre", JSON.stringify(step, null, 2)),
    );
    p.append(d);
  }
  if (entry.files && Object.keys(entry.files).length) {
    const f = el("details");
    f.append(
      el("summary", "Captured files"),
      el("pre", JSON.stringify(entry.files, null, 2)),
    );
    p.append(f);
  }
  return p;
}
function render() {
  const root = $("cases");
  root.replaceChildren();
  let shown = 0;
  const judge = $("judge").value;
  const keys = [
    ...new Set([...Object.keys(data.candidate), ...Object.keys(data.baseline)]),
  ].sort();
  keys.forEach((key, index) => {
    if (!key.includes($("search").value)) return;
    const c = data.candidate[key],
      b = data.baseline[key],
      v = verdict(c, judge),
      human = labels.get(key),
      different =
        human !== undefined
          ? v !== null && v !== human
          : b &&
            v !== null &&
            verdict(b, judge) !== null &&
            v !== verdict(b, judge);
    if ($("disagree").checked && !different) return;
    const box = el("article", undefined, "case");
    box.append(el("h2", key));
    const cols = el("div", undefined, "columns");
    if (data.baseline_run) cols.append(panel(b, "baseline", index));
    else cols.style.gridTemplateColumns = "1fr";
    cols.append(panel(c, "candidate", index));
    box.append(cols);
    if (c && !c.result.skipped) {
      const controls = el("div", undefined, "labels");
      controls.append(
        el(
          "strong",
          human === undefined
            ? "Human verdict: unreviewed"
            : `Human verdict: ${human ? "pass" : "fail"}`,
        ),
      );
      for (const [text, value] of [
        ["Pass", true],
        ["Fail", false],
        ["Clear", undefined],
      ]) {
        const button = el("button", text);
        button.setAttribute(
          "aria-pressed",
          String(human !== undefined && human === value),
        );
        button.onclick = () => {
          if (value === undefined) labels.delete(key);
          else labels.set(key, value);
          render();
        };
        controls.append(button);
      }
      if (human !== undefined && v !== null)
        controls.append(
          el(
            "span",
            different ? "Disagrees with judge" : "Agrees with judge",
            different ? "fail" : "pass",
          ),
        );
      box.append(controls);
    }
    root.append(box);
    shown++;
  });
  $("status").textContent = `${shown} cases · ${labels.size} labeled`;
}
for (const id of ["search", "judge", "disagree"])
  $(id).addEventListener("input", render);
$("export").onclick = () => {
  const rows = [...labels]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([case_key, pass]) => ({
      case_key,
      pass,
      execution_sha256: identity(data.candidate[case_key]),
    }));
  const url = URL.createObjectURL(
    new Blob([rows.map((r) => JSON.stringify(r)).join("\n") + "\n"], {
      type: "application/x-ndjson",
    }),
  );
  const a = el("a");
  a.href = url;
  a.download = "human-labels.jsonl";
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
};
$("import").onchange = async () => {
  try {
    const text = await $("import").files[0].text(),
      rows = text
        .split(/\r?\n/)
        .filter((s) => s.trim())
        .map((s) => JSON.parse(s));
    if (new Set(rows.map((r) => r.case_key)).size !== rows.length)
      throw Error("Duplicate case keys in labels.");
    for (const row of rows) {
      const c = data.candidate[row.case_key];
      if (
        !c ||
        c.result.skipped ||
        typeof row.pass !== "boolean" ||
        (row.execution_sha256 && row.execution_sha256 !== identity(c))
      )
        throw Error(
          "Labels must match candidate execution keys and fingerprints.",
        );
    }
    for (const row of rows) labels.set(row.case_key, row.pass);
    render();
  } catch (error) {
    $("status").textContent = error.message;
  }
};
render();
