"""Deterministic fixtures for the evaluation workflow, not an agent benchmark."""
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdks" / "python"))
import mira

study = mira.Study("quality-workflow", version="1")
judge_id = os.getenv("MIRA_EXAMPLE_JUDGE", "judge:v1")


def judge(sample, transcript):
    if os.getenv("MIRA_EXAMPLE_JUDGE_NA"):
        return mira.make_score(judge_id, 0, False, "offline fixture", na=True)
    passed = transcript.final_response == sample.expected
    if os.getenv("MIRA_EXAMPLE_REVERSE"):
        passed = not passed
    return mira.make_score(judge_id, float(passed), passed, "deterministic reference check")


@study.eval(
    name="quality",
    samples=[
        mira.Sample("answer", prompt="What is six times seven?",
                    expected=os.getenv("MIRA_EXAMPLE_EXPECTED", "42")),
        mira.Sample("refusal", prompt="What number am I thinking of?", expected="I don't know"),
    ],
    targets=[mira.target("sim")],
    scorers=[mira.succeeded(), mira.scorer(judge_id, judge,
        configuration={"rubric": "match the reviewed reference", "version": judge_id})],
)
def subject(sample, cx):
    answer = "I don't know" if sample.id == "refusal" else "42"
    if sample.id == "answer" and os.getenv("MIRA_EXAMPLE_VARIANT") == "baseline":
        answer = "wrong"
    return mira.transcript(answer, usage=mira.Usage(input_tokens=8, output_tokens=2, cost_usd=0.001))


if __name__ == "__main__":
    study.serve()
