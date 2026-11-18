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


def instant(value):
    bounded(value, r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:\d{2})", 40)
    return datetime.datetime.fromisoformat(date(value).replace("Z", "+00:00"))


def bounded(value, pattern, limit):
    if not isinstance(value, str) or not 1 <= len(value) <= limit or not re.fullmatch(pattern, value):
        raise ValueError("invalid progress field")
    return value


def public_progress(value):
    """A progress credential can publish structured lifetimes, not diagnostics."""
    identity = {key: value[key] for key in ("run_id", "attempt", "created_at", "source",
                                          "workflow_sha", "branch", "event", "context")}
    for key in ("run_id", "attempt"):
        if type(identity[key]) is not int or not 1 <= identity[key] <= 2**53 - 1:
            raise ValueError("invalid run identity")
    for key in ("source", "workflow_sha"):
        bounded(identity[key], r"[0-9a-f]{40}", 40)
    bounded(identity["branch"], r"[A-Za-z0-9_./-]+", 200)
    bounded(identity["event"], r"[a-z_]+", 80)
    if identity["context"] not in ("Main validation", "Main validation (acceptance)"):
        raise ValueError("invalid context")
    created = instant(identity["created_at"])
    progress = value["progress"]
    updated = instant(progress["updated_at"])
    if updated < created or updated > datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=5):
        raise ValueError("invalid progress timestamp")
    if progress["mode"] not in ("serial", "parallel"):
        raise ValueError("invalid execution mode")
    tasks = progress["tasks"]
    if not isinstance(tasks, list) or len(tasks) > 200:
        raise ValueError("too many tasks")
    clean, ids = [], set()
    for task in tasks:
        item = {key: bounded(task[key], pattern, limit) for key, pattern, limit in (
            ("id", r"[A-Za-z0-9_.-]+", 100), ("lane", r"[A-Za-z0-9_. -]+", 100),
            ("name", r"[A-Za-z0-9_./() :+-]+", 200), ("log", r"[A-Za-z0-9_./-]+", 250))}
        if any(part in ("", ".", "..") for part in item["log"].split("/")) or item["id"] in ids:
            raise ValueError("invalid task identity or relative log name")
        ids.add(item["id"])
        status = task["status"]
        if status not in ("pending", "running", "passed", "failed", "blocked", "interrupted"):
            raise ValueError("invalid task status")
        start, end = task["started_at"], task["completed_at"]
        if start is not None and not created <= instant(start) <= updated:
            raise ValueError("invalid start")
        if end is not None and not (instant(start) if start else created) <= instant(end) <= updated:
            raise ValueError("invalid completion")
        if (status == "pending" and (start or end)) or (status == "running" and (not start or end)):
            raise ValueError("invalid active interval")
        if status in ("passed", "failed") and (not start or not end):
            raise ValueError("missing completed interval")
        if status == "blocked" and start is not None:
            raise ValueError("blocked task has a process interval")
        if status in ("blocked", "interrupted") and not end:
            raise ValueError("missing terminal timestamp")
        pid, code = task["pid"], task["exit_code"]
        if pid is not None and (type(pid) is not int or not 1 <= pid <= 2**31 - 1):
            raise ValueError("invalid pid")
        if start is None and pid is not None:
            raise ValueError("pid without a process interval")
        if code is not None and (type(code) is not int or not -255 <= code <= 255):
            raise ValueError("invalid exit code")
        if status in ("pending", "running") and code is not None:
            raise ValueError("active task has exit code")
        clean.append({**item, "status": status, "started_at": start, "completed_at": end,
                      "pid": pid, "exit_code": code})
    return {**identity, "progress": {"updated_at": progress["updated_at"],
                                    "mode": progress["mode"], "tasks": clean}}


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
                          "subject": text(suspect.get("subject"), 500),
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
            runs = []
            skipped = 0
            for run in self.data["runs"]:
                if run["status"] == "completed" and (
                    run.get("selected") is False or run.get("classification") == "no-selection"
                ):
                    if run.get("event") == "schedule" and run["conclusion"] != "cancelled":
                        skipped += 1
                    continue
                if run["status"] in ("in_progress", "completed") and run["conclusion"] != "cancelled":
                    runs.append({key: value for key, value in run.items() if not key.startswith("_")})
            return json.dumps({**self.data, "runs": runs, "skipped_wakeups": skipped}).encode()

    def update(self, run=None, progress=False):
        with self.lock:
            runs = list(self.data["runs"])
            if run:
                run = dict(run)
                key = (run["run_id"], run["attempt"])
                existing = None
                for old in runs:
                    if (old["run_id"], old["attempt"]) == key:
                        existing = old
                if progress:
                    latest = run.pop("progress")
                    if existing:
                        for field in ("source", "workflow_sha", "branch", "event", "context"):
                            if existing.get(field) and existing[field] != run[field]:
                                raise ValueError("progress identity changed")
                        previous = existing.get("progress")
                        if previous:
                            if instant(latest["updated_at"]) < instant(previous["updated_at"]) or (
                                instant(latest["updated_at"]) == instant(previous["updated_at"]) and latest != previous
                            ) or latest["mode"] != previous["mode"]:
                                raise ValueError("out of order progress")
                            by_id = {task["id"]: task for task in latest["tasks"]}
                            for task in previous["tasks"]:
                                new = by_id.get(task["id"])
                                if not new or any(new[field] != task[field] for field in ("lane", "name")):
                                    raise ValueError("task history removed")
                                if task["status"] not in ("pending", "running") and new != task:
                                    raise ValueError("terminal task changed")
                                if new["log"] != task["log"] and not (
                                    task["status"] in ("pending", "running") and task["log"].endswith(".log")
                                    and new["log"] == task["log"] + ".gz"
                                ):
                                    raise ValueError("task log name changed")
                                if task["started_at"] and (new["started_at"] != task["started_at"] or new["status"] == "pending"):
                                    raise ValueError("task interval changed")
                        run = dict(existing)
                    else:
                        run = {**public_run({**run, "updated_at": run["created_at"],
                                             "status": "in_progress", "selected": True}), "_progress_only": True}
                    run["progress"] = latest
                elif existing:
                    if (existing["status"] == "completed" and run["status"] != "completed") or (
                        not existing.get("_progress_only") and instant(run["updated_at"]) < instant(existing["updated_at"])
                    ):
                        raise ValueError("refusing an older observation of this attempt")
                    if "progress" in existing:
                        run["progress"] = existing["progress"]
                runs = [old for old in runs if (old["run_id"], old["attempt"]) != key] + [run]
            runs.sort(key=lambda item: (item["run_id"], item["attempt"]), reverse=True)
            data = {"updated_at": self.data["updated_at"] if progress else timestamp(), "runs": runs[:200]}
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


def handler(store, token, progress_token=""):
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
            credential = progress_token if self.path == "/api/progress" else token
            if not credential or not hmac.compare_digest(supplied, f"Bearer {credential}"):
                self.send(401, b'{"error":"unauthorized"}')
                return
            if self.path not in ("/api/run", "/api/heartbeat", "/api/progress"):
                self.send(404, b'{"error":"not found"}')
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if length < 0 or length > 2 * 1024 * 1024:
                    self.send(413, b'{"error":"payload too large"}')
                    return
                raw = self.rfile.read(length)
                payload = json.loads(raw) if self.path != "/api/heartbeat" else None
                # Canonical JSON makes escaped and literal credential text equivalent.
                if any(secret and json.dumps(secret)[1:-1] in json.dumps(payload)
                       for secret in (token, progress_token)):
                    raise ValueError("credential in public payload")
                is_progress = self.path == "/api/progress"
                run = public_progress(payload) if is_progress else public_run(payload) if payload is not None else None
                store.update(run, progress=is_progress)
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
                                 handler(Store(directory), os.environ.get("CI_DASHBOARD_TOKEN", ""),
                                         os.environ.get("CI_DASHBOARD_PROGRESS_TOKEN", "")))
    try:
        server.serve_forever()
    finally:
        server.server_close()
