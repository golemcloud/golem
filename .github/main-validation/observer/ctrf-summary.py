"""Summarizes CTRF reports under a directory: totals, failed tests and recovered (flaky) tests.

Usage: python3 -I ctrf-summary.py <artifact-dir>
Reads only *.json files; never executes anything from the directory.
"""
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
reports, failed, recovered, unreadable = [], [], [], []
totals = {"tests": 0, "passed": 0, "failed": 0, "skipped": 0, "pending": 0, "other": 0}

for path in sorted(root.rglob("*.json")):
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        results = data["results"]
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
    except Exception as error:  # noqa: BLE001
        unreadable.append({"file": str(path.relative_to(root)), "error": type(error).__name__})
        continue
    rel = str(path.relative_to(root))
    reports.append({"file": rel, "tool": (results.get("tool") or {}).get("name"), "summary": results.get("summary")})
    for test in tests:
        status = test.get("status", "other")
        totals["tests"] += 1
        totals[status if status in totals else "other"] += 1
        # test-r records every attempt; a passing test with many passing attempts is a repeated
        # test, not a recovered one, so recovery needs a failed attempt or the CTRF flaky flag.
        attempts = [a.get("status") for a in ((test.get("extra") or {}).get("testR") or {}).get("attempts") or []]
        entry = {
            "name": test.get("name"),
            "suite": test.get("suite"),
            "report": rel,
            "attempts": attempts,
        }
        if status == "failed":
            message = str(test.get("message") or "").strip()
            entry["message"] = message[:4000]
            failed.append(entry)
        elif status == "passed" and (test.get("flaky") is True or "failed" in attempts):
            recovered.append(entry)

json.dump(
    {"reports": reports, "unreadable": unreadable, "totals": totals, "failed": failed, "recovered": recovered},
    sys.stdout,
    indent=1,
)
