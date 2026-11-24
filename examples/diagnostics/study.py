"""Offline diagnostic fixtures. Replace subjects and fixture judge with real agents."""
import os
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdks/python"))
import mira

study = mira.Study("diagnostics", version="1")
baseline = os.getenv("MIRA_DIAGNOSTICS_VARIANT") == "baseline"


def fixture_judge(sample, transcript):
    passed = transcript.final_response == sample.expected["answer"]
    score = mira.make_score("judge:fixture", float(passed), passed,
                            ("step_id=3: answer agrees with reviewed reference" if sample.id == "order" else "answer agrees with reviewed release reference"))
    score.grading = mira.GradingUsage(requests=1, errors=0, duration_ms=2,
                                    input_tokens=100, output_tokens=10, cost_usd=0.0002)
    return score


def tagged(id_, scenario, prompt, expected):
    return mira.Sample(id_, prompt=prompt, expected=expected, tags=["release"],
                       metadata={"scenario": scenario, "dimensions": ["correctness", "safety"]})


@study.eval(name="tools", samples=[tagged("order", "support", "Look up order 42.", {"answer": "Order 42: shipped"})],
            targets=[mira.target("sim")], scorers=[
                mira.tool_invocation("lookup_order", "/id", 42, "shipped"),
                mira.tool_sequence(["lookup_order", "respond"]),
                mira.scorer("judge:fixture", fixture_judge, configuration={"rubric": "reviewed order status", "version": 1})])
def tool_subject(sample, cx):
    answer = "Order 42: pending" if baseline else "Order 42: shipped"
    trajectory = {"schema_version": "ATIF-v1.7", "agent": {"name": "fixture", "version": "1"}, "steps": [
        {"step_id": 1, "source": "agent", "message": "Looking up the order", "tool_calls": [
            {"tool_call_id": "lookup-42", "function_name": "lookup_order", "arguments": {"id": 42}}]},
        {"step_id": 2, "source": "environment", "message": "", "observation": {"results": [
            {"source_call_id": "lookup-42", "content": "Order 42: shipped"}]}},
        {"step_id": 3, "source": "agent", "message": answer, "tool_calls": [
            {"tool_call_id": "response", "function_name": "respond", "arguments": {"answer": answer}}]}]}
    t = mira.from_trajectory(mira.parse_trajectory(trajectory))
    t.timing = mira.Timing(duration_ms=50)
    t.usage = mira.Usage(input_tokens=20, output_tokens=5, cost_usd=0.001)
    return t


@study.eval(name="retrieval", samples=[tagged("docs", "search", "Find the release docs.",
            {"relevant_ids": ["a", "b"], "answer": "Release 42 shipped"})],
            targets=[mira.target("sim")], scorers=[mira.retrieval_precision_at(3, 0.6),
                mira.retrieval_recall_at(3, 1.0), mira.retrieval_average_precision_at(3, 0.8),
                mira.scorer("judge:fixture", fixture_judge, configuration={"rubric": "reviewed release answer", "version": 1})])
def retrieval_subject(sample, cx):
    t = mira.transcript("Release 42 shipped", usage=mira.Usage(input_tokens=10, output_tokens=3, cost_usd=0.001))
    t.metadata["retrieved_ids"] = ["noise", "a", "b"] if baseline else ["a", "noise", "b"]
    t.timing = mira.Timing(duration_ms=30)
    return t


if __name__ == "__main__":
    study.serve()
