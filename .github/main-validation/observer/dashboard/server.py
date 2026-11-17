"""Serve public CI observations; accept updates only from an authenticated observer."""

import datetime
import hmac
import json
import os
from pathlib import Path
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import tempfile
import threading


def timestamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def date(value):
    if not isinstance(value, str):
        raise ValueError("timestamp must be an ISO string")
    parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("timestamp must include its timezone")
    return value


def text(value, limit=4000):
    return str(value or "")[:limit]


def public_run(value):
    """Keep only public dashboard fields, never raw logs, environment or receipts."""
    run_id, attempt = value["run_id"], value["attempt"]
    if any(type(n) is not int or n < 1 for n in (run_id, attempt)):
        raise ValueError("run and attempt must be positive integers")
    source = value.get("source") or ""
    if source and not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("source must be an exact commit SHA")
    context = value.get("context") or ""
    if context not in ("", "Main validation", "Main validation (acceptance)"):
        raise ValueError("unknown validation context")
    tests = value.get("tests") or {}
    analysis = value.get("analysis") or {}
    totals = tests.get("totals") or {}
    if any(type(n) is not int or n < 0 for n in totals.values()):
        raise ValueError("test totals must be nonnegative integers")
    return {
        "run_id": run_id, "attempt": attempt,
        "url": f"https://github.com/golemcloud/golem/actions/runs/{run_id}/attempts/{attempt}",
        "created_at": date(value["created_at"]), "updated_at": date(value["updated_at"]),
        "source": source, "context": context,
        **{key: text(value.get(key), 200) for key in
           ("workflow_sha", "branch", "event", "status", "conclusion", "classification")},
        "selected": value.get("selected") if type(value.get("selected")) is bool else None,
        "jobs": [{"name": text(job.get("name")), "conclusion": text(job.get("conclusion")),
                  "started_at": job.get("started_at"), "completed_at": job.get("completed_at"),
                  "failed_steps": [text(step) for step in job.get("failed_steps", [])]}
                 for job in value.get("jobs", [])],
        "tests": {
            "totals": {key: totals.get(key, 0) for key in
                       ("tests", "passed", "failed", "skipped", "pending", "other")},
            "reports": [{"file": text(report.get("file")), "tool": text(report.get("tool"))}
                        for report in tests.get("reports", [])],
            "unreadable": [{"file": text(report.get("file")), "error": text(report.get("error"))}
                           for report in tests.get("unreadable", [])],
            **{key: [{**{field: text(test.get(field)) for field in ("name", "suite", "report", "message")},
                      "attempts": [text(status, 30) for status in test.get("attempts", [])]}
                     for test in tests.get(key, [])] for key in ("failed", "recovered")},
        },
        "analysis": {
            "summary": text(analysis.get("summary"), 20000),
            "baseline": text(analysis.get("baseline")),
            "limits": [text(item) for item in analysis.get("limits", [])],
            "suspects": [{"sha": suspect["sha"], "confidence": text(suspect.get("confidence"), 20),
                          "reason": text(suspect.get("reason"))}
                         for suspect in analysis.get("suspects", [])
                         if re.fullmatch(r"[0-9a-f]{40}", suspect.get("sha", ""))],
        },
    }


class Store:
    def __init__(self, directory):
        self.path = Path(directory) / "data.json"
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.lock = threading.Lock()
        self.data = json.loads(self.path.read_text()) if self.path.exists() else {"updated_at": None, "runs": []}

    def snapshot(self):
        with self.lock:
            runs = [run for run in self.data["runs"]
                    if run["status"] in ("in_progress", "completed") and run["conclusion"] != "cancelled"]
            return json.dumps({**self.data, "runs": runs}).encode()

    def update(self, run=None):
        with self.lock:
            runs = list(self.data["runs"])
            if run:
                key = (run["run_id"], run["attempt"])
                for old in runs:
                    if (old["run_id"], old["attempt"]) == key:
                        if (old["status"] == "completed" and run["status"] != "completed") or (
                            datetime.datetime.fromisoformat(run["updated_at"].replace("Z", "+00:00")) <
                            datetime.datetime.fromisoformat(old["updated_at"].replace("Z", "+00:00"))
                        ):
                            raise ValueError("refusing an older observation of this attempt")
                runs = [old for old in runs if (old["run_id"], old["attempt"]) != key] + [run]
            runs.sort(key=lambda item: (item["run_id"], item["attempt"]), reverse=True)
            data = {"updated_at": timestamp(), "runs": runs[:200]}
            temporary = None
            try:
                with tempfile.NamedTemporaryFile(mode="w", dir=self.path.parent, delete=False) as output:
                    temporary = output.name
                    json.dump(data, output)
                    output.flush()
                    os.fsync(output.fileno())
                os.replace(temporary, self.path)
            finally:
                if temporary and os.path.exists(temporary):
                    os.unlink(temporary)
            self.data = data


def handler(store, token):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(15)

        def send(self, status, payload, content_type="application/json"):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'none'")
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self):
            if self.path == "/":
                self.send(200, Path(__file__).with_name("index.html").read_bytes(), "text/html; charset=utf-8")
            elif self.path == "/brand.png":
                self.send(200, Path(__file__).with_name("brand.png").read_bytes(), "image/png")
            elif self.path == "/data.json":
                self.send(200, store.snapshot())
            elif self.path == "/healthz":
                self.send(200, b'{"status":"ok"}')
            else:
                self.send(404, b'{"error":"not found"}')

        def do_POST(self):
            supplied = self.headers.get("Authorization", "")
            if not token or not hmac.compare_digest(supplied, f"Bearer {token}"):
                self.send(401, b'{"error":"unauthorized"}')
                return
            if self.path not in ("/api/run", "/api/heartbeat"):
                self.send(404, b'{"error":"not found"}')
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if length < 0 or length > 2 * 1024 * 1024:
                    self.send(413, b'{"error":"payload too large"}')
                    return
                raw = self.rfile.read(length)
                payload = json.loads(raw) if self.path == "/api/run" else None
                # Canonical JSON makes escaped and literal credential text equivalent.
                if json.dumps(token)[1:-1] in json.dumps(payload):
                    raise ValueError("credential in public payload")
                run = public_run(payload) if payload is not None else None
                store.update(run)
            except (ValueError, KeyError, TypeError, AttributeError):
                self.send(400, b'{"error":"invalid observation"}')
                return
            except OSError:
                self.send(503, b'{"error":"could not persist observation"}')
                return
            self.send(200, b'{"status":"stored"}')

    return Handler


if __name__ == "__main__":
    directory = os.environ.get("AMP_DEPLOYMENT_DATA_DIRECTORY") or os.environ.get("DASHBOARD_DATA_DIR")
    if not directory:
        raise SystemExit("set DASHBOARD_DATA_DIR or AMP_DEPLOYMENT_DATA_DIRECTORY")
    server = ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))),
                                 handler(Store(directory), os.environ.get("CI_DASHBOARD_TOKEN", "")))
    try:
        server.serve_forever()
    finally:
        server.server_close()
