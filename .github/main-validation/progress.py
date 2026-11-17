#!/usr/bin/env python3
"""Retain validation output and report task lifetimes without scheduling tasks."""

import argparse
import datetime
import gzip
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request


LANES = {
    "prepare": "main-validation-prepare",
    "worker": "main-validation-workers",
    "service-build": "main-validation-services",
    "service": "main-validation-service-tests",
    "cli": "main-validation-cli-tests",
    "exclusive": "main-validation-exclusive",
}


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as output:
        temporary = Path(output.name)
        try:
            json.dump(value, output)
            output.flush()
            os.fsync(output.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


def task_record(lane, name):
    if lane not in LANES or not re.fullmatch(r"[a-zA-Z0-9_-]+", name):
        raise ValueError("invalid task identity")
    return {"id": f"{lane}--{name}", "lane": lane, "name": name, "status": "pending",
            "started_at": None, "completed_at": None, "pid": None, "exit_code": None,
            "log": f"logs/{lane}--{name}.log"}


def records(directory):
    return [json.loads(path.read_text()) for path in sorted((directory / "progress").glob("*.json"))]


def compress(path):
    compressed = path.with_suffix(path.suffix + ".gz")
    temporary = compressed.with_suffix(".gz.partial")
    try:
        with path.open("rb") as source, gzip.open(temporary, "wb") as target:
            shutil.copyfileobj(source, target)
        os.replace(temporary, compressed)
        path.unlink()
        return compressed
    finally:
        temporary.unlink(missing_ok=True)


def wait_for(process, process_group=False, tick=None):
    received = None
    deadline = None

    def stop(signum, _frame):
        nonlocal received, deadline
        received = signum
        deadline = time.monotonic() + 15
        try:
            if process_group:
                os.killpg(process.pid, signum)
            else:
                process.send_signal(signum)
        except ProcessLookupError:
            pass

    previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        while process.poll() is None:
            if deadline is not None and time.monotonic() >= deadline:
                try:
                    if process_group:
                        os.killpg(process.pid, signal.SIGKILL)
                    else:
                        process.kill()
                except ProcessLookupError:
                    pass
            if tick:
                tick()
            time.sleep(.2)
        code = process.wait()
        return (128 + received if received else 128 - code if code < 0 else code), received
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def run_task(directory, lane, name, command):
    task = task_record(lane, name)
    state_path = directory / "progress" / f'{task["id"]}.json'
    log = directory / task["log"]
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("wb") as output:
        task.update(status="running", started_at=now())
        try:
            process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT)
        except OSError as error:
            output.write(f"Could not start command: {error}\n".encode())
            task.update(status="failed", completed_at=now(), exit_code=127)
            save(state_path, task)
            return 127
        task["pid"] = process.pid
        save(state_path, task)
        code, interrupted = wait_for(process)
        task.update(status="interrupted" if interrupted else "passed" if code == 0 else "failed",
                    completed_at=now(), exit_code=code)
    try:
        task["log"] = str(compress(log).relative_to(directory))
    except OSError:
        print(f'[validation] Compression failed; retained {task["log"]}', flush=True)
    save(state_path, task)
    return code


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def publish(progress, created_at):
    url = os.environ.get("CI_DASHBOARD_URL", "").rstrip("/")
    token = os.environ.get("CI_DASHBOARD_PROGRESS_TOKEN", "")
    if not url or not token:
        return
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        print("[validation] Progress publication disabled: invalid dashboard HTTPS URL", flush=True)
        return
    payload = {
        "run_id": int(os.environ["GITHUB_RUN_ID"]),
        "attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
        "created_at": created_at,
        "source": os.environ["MAIN_VALIDATION_SOURCE"],
        "workflow_sha": os.environ["GITHUB_SHA"],
        "branch": os.environ["GITHUB_REF_NAME"],
        "event": os.environ["GITHUB_EVENT_NAME"],
        "context": os.environ["MAIN_VALIDATION_CONTEXT"],
        "progress": progress,
    }
    body = json.dumps(payload).encode()
    if json.dumps(token)[1:-1].encode() in body:
        print("[validation] Refusing progress publication containing a credential", flush=True)
        return
    request = urllib.request.Request(url + "/api/progress", data=body, method="POST",
                                     headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=5) as response:
            confirmation = json.loads(response.read(1024))
            if response.status != 200 or not isinstance(confirmation, dict) or confirmation.get("status") != "stored":
                raise ValueError("unconfirmed publication")
    except (urllib.error.URLError, http.client.HTTPException, TimeoutError, ValueError, OSError):
        print("[validation] Dashboard publication failed; local progress and logs are retained", flush=True)


def failure_excerpt(path):
    tail = b""
    opener = gzip.open if path.suffix == ".gz" else open
    try:
        with opener(path, "rb") as source:
            while chunk := source.read(65536):
                tail = (tail + chunk)[-8192:]
        for line in tail.decode("utf-8", errors="replace").splitlines()[-15:]:
            print(f"[log excerpt] {line[:500]}", flush=True)
    except OSError:
        print("[validation] Log excerpt unavailable; inspect retained evidence", flush=True)


def observe(directory, command):
    directory.mkdir(parents=True, exist_ok=True)
    progress_dir = directory / "progress"
    progress_dir.mkdir(exist_ok=True)
    for path in progress_dir.glob("*.json"):
        path.unlink()
    with Path("Makefile.main-validation.toml").open("rb") as source:
        config = tomllib.load(source)
    for lane, name in LANES.items():
        for task_name in config["tasks"][name].get("env", {}).get("MAIN_VALIDATION_TASKS", "").split():
            task = task_record(lane, task_name)
            save(progress_dir / f'{task["id"]}.json', task)
    created_at = now()
    mode = "parallel" if os.environ.get("MAIN_VALIDATION_PARALLEL", "true") == "true" else "serial"
    last_heartbeat = 0
    previous = {}
    (directory / "logs").mkdir(exist_ok=True)

    def report(final=False):
        nonlocal last_heartbeat, previous
        tasks = records(directory)
        for task in tasks:
            if task["status"] != previous.get(task["id"], "pending"):
                print(f'[validation] {task["lane"]}: {task["name"]} {task["status"]}; log: {task["log"]}', flush=True)
                if task["status"] == "failed":
                    failure_excerpt(directory / task["log"])
        previous = {task["id"]: task["status"] for task in tasks}
        if final or time.monotonic() - last_heartbeat >= 30:
            counts = {status: sum(t["status"] == status for t in tasks)
                      for status in ("passed", "failed", "running", "pending", "blocked", "interrupted")}
            print(f'[validation] Requested mode: {mode}; ' + "; ".join(f"{n} {s}" for s, n in counts.items()), flush=True)
            for task in tasks:
                if task["status"] == "running":
                    elapsed = (datetime.datetime.now(datetime.timezone.utc) - datetime.datetime.fromisoformat(task["started_at"])).total_seconds()
                    print(f'[validation] Active: {task["lane"]}/{task["name"]}, {int(elapsed)}s, PID {task["pid"]}', flush=True)
            progress = {"updated_at": now(), "mode": mode, "tasks": tasks}
            save(directory / "progress.json", progress)
            try:
                publish(progress, created_at)
            except (KeyError, ValueError):
                print("[validation] Missing Actions identity; dashboard publication skipped", flush=True)
            last_heartbeat = time.monotonic()

    # The workload never receives the dashboard write credential.
    environment = {key: value for key, value in os.environ.items() if key != "CI_DASHBOARD_PROGRESS_TOKEN"}
    with (directory / "logs/controller.log").open("wb") as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT,
                                   env=environment, start_new_session=True)
        code, interrupted = wait_for(process, process_group=True, tick=report)
    for task in records(directory):
        if task["status"] in ("pending", "running"):
            task["status"] = "blocked" if task["status"] == "pending" else "interrupted"
            task["completed_at"] = now()
            save(progress_dir / f'{task["id"]}.json', task)
    report(final=True)
    if code != 0:
        failure_excerpt(directory / "logs/controller.log")
    try:
        compress(directory / "logs/controller.log")
    except OSError:
        print("[validation] Controller log compression failed; raw file retained", flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as output:
            output.write(f"\n## Validation task progress\n\nRequested mode: {mode}. Intervals record process lifetimes, not CPU utilization.\n\n")
            output.write("| Lane | Task | Result | Started | Finished | Log |\n|---|---|---|---|---|---|\n")
            for task in records(directory):
                output.write("| " + " | ".join(str(task[field] or "—") for field in
                             ("lane", "name", "status", "started_at", "completed_at", "log")) + " |\n")
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("observe", "run"))
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    arguments = args.arguments
    directory = Path(os.environ["MAIN_VALIDATION_REPORT_DIR"]).resolve()
    if args.operation == "observe" and arguments:
        return observe(directory, arguments)
    if args.operation == "run" and len(arguments) > 3 and arguments[2] == "--":
        return run_task(directory, arguments[0], arguments[1], arguments[3:])
    parser.error("observe -- command ... | run lane task -- command ...")


if __name__ == "__main__":
    sys.exit(main())
