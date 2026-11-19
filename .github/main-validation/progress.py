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


def wait_for(process, process_group=False, tick=None, timeout_seconds=None, on_timeout=None):
    received = None
    deadline = None
    expires = time.monotonic() + timeout_seconds if timeout_seconds is not None else None
    timed_out = False

    def send(signum):
        try:
            if process_group:
                os.killpg(process.pid, signum)
            else:
                process.send_signal(signum)
        except ProcessLookupError:
            pass

    def stop(signum, _frame):
        nonlocal received, deadline
        received = signum
        deadline = time.monotonic() + 15
        send(signum)

    previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        while process.poll() is None:
            if expires is not None and time.monotonic() >= expires and deadline is None:
                timed_out = True
                deadline = time.monotonic() + 15
                if on_timeout:
                    on_timeout()
                send(signal.SIGTERM)
            if deadline is not None and time.monotonic() >= deadline:
                send(signal.SIGKILL)
            if tick:
                tick()
            time.sleep(.2)
        code = process.wait()
        return (128 + received if received else 124 if timed_out else 128 - code if code < 0 else code), received
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def run_task(directory, lane, name, command, timeout_seconds=None):
    task = task_record(lane, name)
    # Test budgets include dependency construction and teardown. Builds retain
    # the workflow deadline because cold compilation can take several hours.
    if timeout_seconds is None and lane in ("worker", "service", "cli"):
        timeout_seconds = 60 * 60
    state_path = directory / "progress" / f'{task["id"]}.json'
    log = directory / task["log"]
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("wb") as output:
        task.update(status="running", started_at=now())
        gate_read, gate_write = os.pipe()
        try:
            # Publish the new process group before it can start the workload.
            # If this wrapper dies during publication, EOF releases the launcher
            # without executing anything the observer cannot yet find.
            process = subprocess.Popen(
                [sys.executable, __file__, "exec", str(gate_read), "--", *command],
                stdout=output, stderr=subprocess.STDOUT, start_new_session=True,
                pass_fds=(gate_read,))
            task["pid"] = process.pid
            save(state_path, task)
            os.write(gate_write, b"1")
        except OSError as error:
            output.write(f"Could not start command: {error}\n".encode())
            task.update(status="failed", completed_at=now(), exit_code=127)
            save(state_path, task)
            return 127
        finally:
            os.close(gate_read)
            os.close(gate_write)

        def timed_out():
            output.write(f"\n[validation] Task exceeded {timeout_seconds:g}s; terminating its process group\n".encode())
            output.flush()

        code, interrupted = wait_for(process, process_group=True,
                                     timeout_seconds=timeout_seconds, on_timeout=timed_out)
        if code == 124 or interrupted:
            # A parent can exit on SIGTERM while a descendant ignores it.
            # Do not leave that descendant holding ports for the next group.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
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


def resource_sample():
    cpu = [int(value) for value in Path("/proc/stat").read_text().splitlines()[0].split()[1:9]]
    memory = {}
    for line in Path("/proc/meminfo").read_text().splitlines():
        name, value = line.split(":", 1)
        if name in ("MemTotal", "MemAvailable", "SwapTotal", "SwapFree"):
            memory[name] = int(value.split()[0]) * 1024
    disks = {}
    for name, path in (("worktree", Path.cwd()), ("target", Path(os.environ.get("CARGO_TARGET_DIR", "target")))):
        if path.exists():
            usage = shutil.disk_usage(path)
            disks[name] = {"total_bytes": usage.total, "free_bytes": usage.free}
    pressure = {}
    for name in ("cpu", "memory", "io"):
        path = Path("/proc/pressure") / name
        if path.exists():
            pressure[name] = path.read_text().strip()
    # Keep cumulative device counters so elapsed I/O time and queueing can be
    # compared with task intervals, even on runners without pressure metrics.
    diskstats = Path("/proc/diskstats")
    diskstats = diskstats.read_text() if diskstats.exists() else None
    return {"timestamp": now(), "monotonic_seconds": time.monotonic(), "scope": "runner-host",
            "available_cpus": len(os.sched_getaffinity(0)), "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "cpu_ticks": dict(zip(("user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"), cpu)),
            "memory_bytes": memory, "load_average": os.getloadavg(), "disks": disks,
            "pressure": pressure, "diskstats": diskstats}


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
    last_resource_sample = 0
    resource_sampling = True
    previous = {}
    (directory / "logs").mkdir(exist_ok=True)
    resources = directory / "resources.jsonl"

    def report(final=False):
        nonlocal last_heartbeat, last_resource_sample, resource_sampling, previous
        if resource_sampling and (final or time.monotonic() - last_resource_sample >= 5):
            try:
                sample = resource_sample()
                with resources.open("w" if last_resource_sample == 0 else "a") as output:
                    output.write(json.dumps(sample) + "\n")
                last_resource_sample = time.monotonic()
            except (OSError, ValueError, IndexError, AttributeError):
                print("[validation] Resource sampling unavailable; validation continues without resource evidence", flush=True)
                resource_sampling = False
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

    # Omit the credential from the child environment, but this is not isolation:
    # a same-user workload can still read the observer's environment.
    environment = {key: value for key, value in os.environ.items() if key != "CI_DASHBOARD_PROGRESS_TOKEN"}
    with (directory / "logs/controller.log").open("wb") as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT,
                                   env=environment, start_new_session=True)
        code, interrupted = wait_for(process, process_group=True, tick=report)
    if interrupted:
        # The controller may exit before a wrapper that ignores SIGTERM.
        # Stop those wrappers before inspecting their last published task PIDs.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    for task in records(directory):
        if task["status"] in ("pending", "running"):
            if task["status"] == "running" and task["pid"] is not None:
                # Controller escalation can kill a task wrapper before it cleans
                # up its separate process group. The observer owns that cleanup.
                try:
                    os.killpg(task["pid"], signal.SIGKILL)
                except ProcessLookupError:
                    pass
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
    parser.add_argument("operation", choices=("observe", "run", "exec"))
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    arguments = args.arguments
    if args.operation == "exec" and len(arguments) > 2 and arguments[1] == "--":
        gate = int(arguments[0])
        try:
            released = os.read(gate, 1) == b"1"
        finally:
            os.close(gate)
        if not released:
            return 143
        try:
            os.execvp(arguments[2], arguments[2:])
        except OSError as error:
            print(f"Could not start command: {error}", file=sys.stderr)
            return 127
    directory = Path(os.environ["MAIN_VALIDATION_REPORT_DIR"]).resolve()
    if args.operation == "observe" and arguments:
        return observe(directory, arguments)
    if args.operation == "run" and len(arguments) > 3 and arguments[2] == "--":
        return run_task(directory, arguments[0], arguments[1], arguments[3:])
    parser.error("observe -- command ... | run lane task -- command ...")


if __name__ == "__main__":
    sys.exit(main())
