"""Publish an observation and its analysis, or a heartbeat, without running workload code."""

import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request

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
