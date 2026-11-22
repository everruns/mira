"""Exercise the real CLI, study protocol, reports, and review loop offline."""
import argparse
import csv
import json
import os
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path


def smoke(mira, work):
    example = Path(__file__).resolve().parent
    config = 'default_launcher = "quality-workflow"\n[launchers.quality-workflow]\npython3 = ' + json.dumps(str(example / "study.py")) + "\n"
    (work / "mira.toml").write_text(config)
    log = work / "smoke.log"

    def command(*args, expected=0, **environment):
        result = subprocess.run([str(mira), *args], cwd=work, env={**{key: value for key, value in os.environ.items() if not key.startswith("MIRA_EXAMPLE_")}, **environment},
                                capture_output=True, text=True)
        with log.open("a") as stream:
            stream.write("$ mira " + " ".join(args) + "\n" + result.stdout + result.stderr + "\n")
        if result.returncode != expected:
            raise RuntimeError(f"{args}: expected exit {expected}, got {result.returncode}\n{result.stderr}")
        return result

    def newest():
        return max((p for p in (work / "results").iterdir() if (p / "meta.json").exists()),
                   key=lambda p: (p / "meta.json").stat().st_mtime_ns).name

    command("run", "--experiment", str(example / "experiment.toml"), "--require-scorer", "judge:*", "--min-coverage", "1")
    experiment = next((work / "results/experiments").iterdir())
    comparison = json.loads((experiment / "comparison.json").read_text())
    assert comparison["comparisons"]["candidate"]["improvements"] == 1
    baseline = Path(comparison["runs"]["baseline"]).name
    candidate = Path(comparison["runs"]["candidate"]).name
    command("compare", baseline, candidate, "--max-regressions", "0", "--out", "comparison.json")
    assert (work / "results" / baseline / "treatment-files.json").exists()

    command("calibrate", baseline, "--labels", str(example / "labels.jsonl"), "--scorer", "judge:v1",
            "--min-agreement", "1", "--max-false-pass-rate", "0", "--out", "calibration.json")
    command("failures", baseline, "--out", "review.jsonl")
    review = [json.loads(line) for line in (work / "review.jsonl").read_text().splitlines()]
    assert len(review) == 1 and review[0]["status"] == "needs_review"

    command("score", "--run-id", baseline, expected=1, MIRA_EXAMPLE_EXPECTED="wrong")
    command("score", "--run-id", baseline, "--use-current-samples", MIRA_EXAMPLE_EXPECTED="wrong")
    updated = work / "results" / newest() / "report.json"
    answer = next(case for case in json.loads(updated.read_text())["cases"] if case["sample"] == "answer")
    assert answer["transcript"]["metadata"]["mira.sample"]["expected"] == "wrong"
    command("score", "--run-id", baseline, expected=1, MIRA_EXAMPLE_JUDGE="judge:v2", MIRA_EXAMPLE_REVERSE="1")
    regraded = newest()
    command("calibrate", regraded, "--labels", str(example / "labels.jsonl"), "--scorer", "judge:v2",
            "--previous", baseline, "--previous-scorer", "judge:v1", "--out", "flips.json")
    assert len(json.loads((work / "flips.json").read_text())["verdict_flips"]) == 2

    command("run", "--require-scorer", "judge:*", "--min-coverage", "1", expected=2, MIRA_EXAMPLE_JUDGE_NA="1")
    incomplete = newest()
    for format_, suffix in [("json", "json"), ("html", "html"), ("md", "md"), ("junit", "xml"), ("jsonl", "jsonl"), ("csv", "csv")]:
        command("report", incomplete, "--format", format_, "--out", "coverage." + suffix)
    assert json.loads((work / "coverage.json").read_text())["evaluation"]["gates_passed"] is False
    assert "Evaluation incomplete" in (work / "coverage.html").read_text()
    assert ET.parse(work / "coverage.xml").find(".//testsuite[@name='mira.coverage']/testcase/failure") is not None
    with (work / "coverage.csv").open(newline="") as stream:
        assert json.loads(next(csv.DictReader(stream))["meta.mira.evaluation"])["gates_passed"] is False
    first = json.loads((work / "coverage.jsonl").read_text().splitlines()[0])
    assert first["transcript"]["metadata"]["mira.evaluation"]["gates_passed"] is False
    command("run", "--trials", "3", "--seed", "10")
    repeated = work / "results" / newest()
    assert len(list((repeated / "artifacts").glob("*.json"))) == 6
    trials = json.loads((repeated / "report.json").read_text())["cases"]
    assert {(case.get("trial", 0), case["trials"], case["seed"]) for case in trials} == {(0,3,10),(1,3,11),(2,3,12)}
    command("export", candidate, "--format", "atif", "--out", "atif")
    assert len(list((work / "atif").glob("*.atif.json"))) == 2
    print(f"PASS: experiments, comparison, provenance, rescoring, calibration, review, six report formats, ATIF export\nArtifacts: {work}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mira", default="mira", help="Path to the built CLI or a command on PATH")
    parser.add_argument("--out", type=Path, help="Keep artifacts in a new directory below this path")
    args = parser.parse_args()
    executable = Path(shutil.which(args.mira) or args.mira).resolve()
    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
        smoke(executable, Path(tempfile.mkdtemp(prefix="mira-quality-", dir=args.out)))
    else:
        with tempfile.TemporaryDirectory(prefix="mira-quality-") as directory:
            smoke(executable, Path(directory))
