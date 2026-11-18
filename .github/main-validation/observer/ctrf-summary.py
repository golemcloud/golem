"""Summarizes CTRF reports under a directory: totals, failed tests and recovered (flaky) tests.

Usage: python3 -I ctrf-summary.py <artifact-dir>
Reads only ctrf*.json reports; never executes anything from the directory.
"""
import json
import pathlib
import runpy
import sys

read_report = runpy.run_path(str(pathlib.Path(__file__).with_name("ctrf-report.py")))["read_report"]
root = pathlib.Path(sys.argv[1])
reports, failed, recovered, unreadable = [], [], [], []
totals = {"tests": 0, "passed": 0, "failed": 0, "skipped": 0, "pending": 0, "other": 0}

for path in sorted(root.rglob("ctrf*.json")):
    try:
        results = read_report(path)
        tests = results["tests"]
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
