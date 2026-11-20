"""Extract task evidence from retained artifacts without executing their contents.

Output is private evidence until the observer reviews excerpts for public disclosure.
"""
import gzip
import json
from pathlib import Path
import re
import runpy
import sys

read_report = runpy.run_path(str(Path(__file__).with_name("ctrf-report.py")))["read_report"]


def artifact_file(directory, relative):
    path = (directory / relative).resolve()
    if not path.is_relative_to(directory.resolve()):
        raise ValueError("artifact path escapes its directory")
    return path


def collect(root):
    evidence = {"tasks": [], "versions": [], "limits": []}
    directories = {path.parent for path in root.rglob("progress.json")}
    directories.update(path.parent.parent for path in root.rglob("progress/*.json"))
    for directory in sorted(directories):
        tasks = {}
        snapshot = directory / "progress.json"
        try:
            if snapshot.exists():
                tasks.update((task["id"], task) for task in json.loads(snapshot.read_text())["tasks"])
            for path in sorted((directory / "progress").glob("*.json")):
                task = json.loads(path.read_text())
                tasks[task["id"]] = task
        except (OSError, ValueError, KeyError, TypeError):
            evidence["limits"].append(f"Unreadable task progress: {directory.relative_to(root)}")
        for task in tasks.values():
            failed = task.get("status") == "failed"
            build = task.get("name") == "build-cli-test-bins"
            if not failed and not build:
                continue
            item = {key: task.get(key) for key in ("id", "lane", "name", "exit_code")}
            item.update(artifact=str(directory.relative_to(root)), log=task.get("log"),
                        reports=[], first_error=None, after_summary_error=None,
                        log_failures=[], log_state="available")
            names, report_names, after_summary = set(), set(), False
            try:
                path = artifact_file(directory, task["log"])
                opener = gzip.open if path.suffix == ".gz" else open
                with opener(path, "rt", encoding="utf-8", errors="replace") as log:
                    for number, raw in enumerate(log, 1):
                        line = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", raw).strip()
                        for field in ("first_error", "after_summary_error"):
                            error = item[field]
                            if error and error["line"] == number - 1 and (
                                "panicked at" in error["message"] or re.search(r"(?i:\berror:)", line)
                            ):
                                error["message"] = (error["message"] + "\n" + line)[:2000]
                        report = re.search(r"Logging to (.+[/\\])?(ctrf[^/\\]*\.json)$", line)
                        if report:
                            report_names.add(report[2])
                        failure = re.search(r"Finished test: (.+) \[FAILED\]$", line)
                        if failure:
                            names.add(failure[1])
                        if line.startswith("test result: FAILED"):
                            after_summary = True
                        if re.search(r"(?i:\berror(?:\[[^]]+\])?:|\bpanicked at\b)|\bERROR\b", line):
                            field = "after_summary_error" if after_summary else "first_error"
                            if item[field] is None:
                                item[field] = {"line": number, "message": line[:2000]}
                        if build and re.fullmatch(r"golem(?:-cli)? [A-Za-z0-9._+() /-]{1,200}", line):
                            evidence["versions"].append({"value": line, "log": task["log"],
                                                         "artifact": item["artifact"], "line": number})
                for name in sorted(report_names):
                    report = {"file": name, "state": "missing", "tests": None}
                    report_path = artifact_file(directory, name)
                    if report_path.exists():
                        try:
                            tests = read_report(report_path)["tests"]
                            report.update(state="reported" if tests else "empty", tests=len(tests))
                        except (OSError, ValueError, KeyError, TypeError):
                            report["state"] = "unreadable"
                    item["reports"].append(report)
            except (OSError, ValueError, KeyError, TypeError, EOFError):
                item["log_state"] = "unavailable"
            item["log_failures"] = sorted(names)
            if failed:
                evidence["tasks"].append(item)
    if not directories:
        evidence["limits"].append("Retained task progress is unavailable; task coverage cannot be determined.")
    return evidence


if __name__ == "__main__":
    json.dump(collect(Path(sys.argv[1])), sys.stdout, indent=1)
