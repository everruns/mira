"""Run diagnostics, policies, inventory, review, and fingerprinted labels offline."""
import argparse
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def smoke(mira, work):
    example = Path(__file__).resolve().parent
    (work / "mira.toml").write_text('default_launcher = "diagnostics"\n[launchers.diagnostics]\npython3 = ' + json.dumps(str(example / "study.py")) + "\n")
    log = work / "smoke.log"
    def cmd(*args, expected=0, variant="candidate"):
        result = subprocess.run([str(mira), *map(str, args)], cwd=work, env={**os.environ, "MIRA_DIAGNOSTICS_VARIANT": variant}, text=True, capture_output=True)
        with log.open("a") as stream:
            stream.write("$ mira " + " ".join(map(str, args)) + "\n" + result.stdout + result.stderr)
        assert result.returncode == expected, f"{args}: {result.returncode}, expected {expected}\n{result.stderr}"
        return result
    def newest():
        return max((work / "results").glob("*/meta.json"), key=lambda p:p.stat().st_mtime_ns).parent
    cmd("run", expected=1, variant="baseline")
    baseline = newest()
    cmd("run")
    candidate = newest()
    cmd("assess", candidate, "--policy", example / "policy.toml", "--out", work / "assessment.json")
    report = json.loads((work / "assessment.json").read_text())
    assert all(c["covered"] for c in report["inventory"])
    assert report["rules"][1]["grading_cost_usd"] == 0.0004
    cmd("assess", baseline, "--policy", example / "policy.toml", expected=1)
    missing = (example / "policy.toml").read_text().replace('["correctness", "safety"]', '["correctness", "safety", "robustness"]')
    (work / "missing.toml").write_text(missing)
    cmd("assess", candidate, "--policy", work / "missing.toml", expected=2)
    costly = (example / "policy.toml").read_text().replace('max_grading_cost_usd = 0.001', 'max_grading_cost_usd = 0.0001')
    (work / "budget.toml").write_text(costly)
    cmd("assess", candidate, "--policy", work / "budget.toml", expected=1)
    cmd("compare", baseline, candidate, "--max-regressions", "0", "--out", work / "comparison.json")
    labels = []
    for case in json.loads((candidate / "report.json").read_text())["cases"]:
        labels.append({"case_key":case["eval"]+"/"+case["sample"]+"@"+case["target"], "pass":True,
                       "execution_sha256":case["transcript"]["metadata"]["mira.execution_sha256"]})
    (work / "labels.jsonl").write_text("\n".join(map(json.dumps, labels))+"\n")
    cmd("calibrate", candidate, "--labels", work / "labels.jsonl", "--scorer", "judge:fixture", "--min-agreement", "1", "--out", work / "calibration.json")
    cmd("review", candidate, "--baseline", baseline, "--calibration", work / "calibration.json", "--out", work / "review.html")
    html = (work / "review.html").read_text()
    assert "lookup-42" in html and "Export human labels" in html and "Grading:" in html
    assert json.loads((candidate / "report.json").read_text())["grading"]["usage"]["requests"] == 2
    cmd("report", candidate.name, "--format", "html", "--out", work / "report.html")
    assert "Grading resources" in (work / "report.html").read_text()
    labels[0]["execution_sha256"] = "wrong-execution"
    (work / "wrong-labels.jsonl").write_text("\n".join(map(json.dumps, labels))+"\n")
    cmd("calibrate", candidate, "--labels", work / "wrong-labels.jsonl", "--scorer", "judge:fixture", expected=2)
    print(f"PASS: diagnostic scorers, slice policies, grading budget, coverage inventory, evidence review, execution-bound labels\nArtifacts: {work}")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--mira", default="mira")
    p.add_argument("--out", type=Path)
    args = p.parse_args()
    mira = Path(shutil.which(args.mira) or args.mira).resolve()
    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
        smoke(mira, Path(tempfile.mkdtemp(prefix="mira-diagnostics-", dir=args.out)))
    else:
        with tempfile.TemporaryDirectory(prefix="mira-diagnostics-") as directory:
            smoke(mira, Path(directory))
