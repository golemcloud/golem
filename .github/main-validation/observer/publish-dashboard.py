"""Publish an observation and its analysis, or a heartbeat, without running workload code."""

import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request


def changes_since_previous_run(run):
    """Enumerate Git's complete source range, independently of failure suspects."""
    result = {"available": False, "baseline": "", "commits": [], "note": ""}
    source = run.get("source", "")
    if not re.fullmatch(r"[0-9a-f]{40}", source) or not run.get("context"):
        result["note"] = "The tested source and context are not yet verified."
        return result
    state = Path(os.environ.get("OBSERVER_STATE_DIR", ".local/ci-observer"))
    previous = []
    try:
        created = datetime.datetime.fromisoformat(run["created_at"].replace("Z", "+00:00"))
        for path in (state / "seen").glob("*.json"):
            record = json.loads(path.read_text())
            if (record.get("run_id") == run["run_id"] or record.get("selected") is not True
                    or record.get("context") != run["context"]
                    or record.get("classification") in ("incomplete", "no-selection")
                    or record.get("conclusion") == "cancelled"
                    or not re.fullmatch(r"[0-9a-f]{40}", record.get("source", ""))):
                continue
            timestamp = datetime.datetime.fromisoformat(record["created_at"].replace("Z", "+00:00"))
            if timestamp < created:
                previous.append((timestamp, record["run_id"], record.get("attempt", 1), record["source"]))
        if not previous:
            result["note"] = "No previous comparable selected run with a verified source is available."
            return result
        baseline = max(previous)[3]
        result["baseline"] = baseline

        def git(*arguments):
            return subprocess.run(["git", *arguments], capture_output=True, text=True, timeout=30, check=True).stdout

        if git("rev-parse", "--is-shallow-repository").strip() != "false":
            result["note"] = "Full Git history is unavailable in the observer checkout."
            return result
        history = git("log", "--reverse", "--format=%H%x00%s", f"{baseline}..{source}", "--")
        for line in history.rstrip("\n").split("\n") if history else []:
            sha, subject = line.split("\x00", 1)
            result["commits"].append({"sha": sha, "subject": subject})
        ancestor = subprocess.run(["git", "merge-base", "--is-ancestor", baseline, source],
                                  capture_output=True, timeout=30)
        if ancestor.returncode not in (0, 1):
            raise ValueError("cannot establish source ancestry")
        result["available"] = True
        result["note"] = ("Oldest first. Commits reachable from this source but not the previous source."
                          if ancestor.returncode == 0 else
                          "The histories diverge. Listed commits are reachable from this source but not the previous source.")
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        result.update(available=False, commits=[], note="The complete source range is unavailable; the observer must fetch and verify its history.")
    return result


url = os.environ.get("CI_DASHBOARD_URL", "").rstrip("/")
token = os.environ.get("CI_DASHBOARD_TOKEN", "")
if not url or not token:
    raise SystemExit("CI_DASHBOARD_URL and CI_DASHBOARD_TOKEN must be configured")
parsed = urllib.parse.urlsplit(url)
if parsed.scheme != "https" or parsed.username or parsed.password or parsed.query or parsed.fragment:
    raise SystemExit("CI_DASHBOARD_URL must be an HTTPS URL without credentials or query parameters")

if sys.argv[1:] == ["--heartbeat"]:
    endpoint, payload, link = "/api/heartbeat", {}, url
elif len(sys.argv) == 3:
    payload = json.loads(Path(sys.argv[1]).read_text())
    report = json.loads(Path(sys.argv[2]).read_text())
    payload["classification"] = report["classification"]
    payload["analysis"] = report["analysis"]
    payload["changes"] = changes_since_previous_run(payload)
    endpoint = "/api/run"
    link = f'{url}/#run-{payload["run_id"]}-a{payload["attempt"]}'
else:
    raise SystemExit("usage: publish-dashboard.py <facts.json> <analysis.json> | --heartbeat")

body = json.dumps(payload).encode()
for name in ("CI_DASHBOARD_TOKEN", "SLACK_BENCHMARK_WEBHOOK_URL"):
    secret = os.environ.get(name)
    if secret and json.dumps(secret)[1:-1].encode() in body:
        raise SystemExit("refusing to publish a credential in dashboard evidence")
request = urllib.request.Request(url + endpoint, data=body, method="POST",
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        if response.status != 200 or json.load(response).get("status") != "stored":
            raise SystemExit("dashboard did not confirm storage; keep this observation pending")
except (urllib.error.URLError, TimeoutError):
    raise SystemExit("dashboard publication failed; retrying the same observation is safe")
print(link)
