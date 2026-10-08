"""Check actual CLI shard reports against the integration binary's test inventory."""

import argparse
from collections import Counter
import json
from pathlib import Path
import re


def validate(inventory, reports):
    names = [
        line for line in inventory.splitlines() if line.startswith("integration::")
    ]
    counts = re.findall(r"^(\d+) tests$", inventory, re.MULTILINE)
    if len(counts) != 1 or int(counts[0]) != len(names) or not names:
        raise ValueError("The CLI test inventory is empty or incomplete")
    if len(set(names)) != len(names):
        raise ValueError("The CLI test inventory contains duplicate names")
    if not reports or any(not tests for tests in reports.values()):
        raise ValueError("A CLI shard report is missing or empty")

    selected = Counter(name for tests in reports.values() for name in tests)
    missing = sorted(set(names) - selected.keys())
    unexpected = sorted(selected.keys() - set(names))
    duplicated = sorted(name for name, count in selected.items() if count != 1)
    if missing or unexpected or duplicated:
        raise ValueError(
            f"Missing tests: {missing}\nUnexpected tests: {unexpected}\n"
            f"Tests selected more than once: {duplicated}"
        )
    return len(names)


def latest_reports(directory):
    latest = {}
    for path in directory.glob(
        "cli-integration-tests-*-report-attempt*/ctrf-report.json"
    ):
        match = re.fullmatch(r"(.+)-report-attempt(\d+)", path.parent.name)
        shard, attempt = match.group(1), int(match.group(2))
        if shard not in latest or attempt > latest[shard][0]:
            latest[shard] = (attempt, path)
    return {
        shard: [
            test["name"] for test in json.loads(path.read_text())["results"]["tests"]
        ]
        for shard, (_, path) in latest.items()
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("reports", type=Path)
    args = parser.parse_args()
    reports = latest_reports(args.reports)
    count = validate(args.inventory.read_text(), reports)
    print(
        f"All {count} CLI tests belong to exactly one of {len(reports)} non-empty shards"
    )


if __name__ == "__main__":
    main()
