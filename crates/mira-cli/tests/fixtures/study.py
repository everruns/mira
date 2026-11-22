"""Offline study exercising the real Python SDK and host workflows."""
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "sdks" / "python"))
import mira

study = mira.Study("workflow", version="1")
judge_name = os.getenv("MIRA_TEST_JUDGE", "judge:v1")


def judge(sample, transcript):
    if os.getenv("MIRA_TEST_JUDGE_NA"):
        return mira.make_score(judge_name, 0, False, "offline", na=True)
    passed = transcript.final_response == sample.expected
    if os.getenv("MIRA_TEST_REVERSE"):
        passed = not passed
    return mira.make_score(judge_name, float(passed), passed, "offline reference check")


@study.eval(
    name="quality",
    samples=[mira.Sample("answer", prompt="What is the answer?",
                         expected=os.getenv("MIRA_TEST_EXPECTED", "42"), tags=["regression"])],
    targets=[mira.target("sim", available=not os.getenv("MIRA_TEST_UNAVAILABLE"))],
    scorers=[mira.succeeded(), mira.scorer(judge_name, judge)],
)
def quality(sample, cx):
    return mira.transcript(os.getenv("MIRA_TEST_ANSWER", "42"),
                           usage=mira.Usage(input_tokens=1, output_tokens=1, cost_usd=0.01))


if __name__ == "__main__":
    study.serve()
