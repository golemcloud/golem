"""Read one CTRF report using the contract shared by totals and task coverage."""
import json


def read_report(path):
    results = json.loads(path.read_text(encoding="utf-8"))["results"]
    tests = results["tests"]
    if not isinstance(tests, list) or not isinstance(results.get("tool") or {}, dict):
        raise ValueError("invalid CTRF structure")
    for test in tests:
        if not isinstance(test, dict) or not isinstance(test.get("status", "other"), str):
            raise ValueError("invalid CTRF test")
        extra = test.get("extra") or {}
        if not isinstance(extra, dict):
            raise ValueError("invalid CTRF extra")
        history = extra.get("testR") or {}
        if not isinstance(history, dict):
            raise ValueError("invalid test-r history")
        attempts = history.get("attempts") or []
        if not isinstance(attempts, list) or any(not isinstance(attempt, dict) for attempt in attempts):
            raise ValueError("invalid test-r attempts")
    return results
