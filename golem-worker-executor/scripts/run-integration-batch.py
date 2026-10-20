#!/usr/bin/env python3
"""Run an exact-name integration batch with thermal and elapsed-time protection.

This is an external test driver, not Worker or guest-runtime instrumentation.
It does not build, retry, throttle I/O or change concurrency inside a test.
"""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time


MAX_SECONDS = 20 * 60
THERMAL_LIMIT_C = 70.0
TERMINATION_GRACE_SECONDS = 5.0
DRIVER_SHA256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def become_subreaper():
    if sys.platform != "linux":
        raise RuntimeError("this driver requires Linux descendant tracking")
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "cannot enable descendant adoption")


def process_entry(pid):
    try:
        fields = (Path("/proc") / str(pid) / "stat").read_text().rpartition(")")[2].split()
        return {"parent": int(fields[1]), "state": fields[0], "start": int(fields[19])}
    except (OSError, ValueError, IndexError):
        return None


def process_table():
    result = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        item = process_entry(entry.name)
        if item is not None:
            result[int(entry.name)] = item
    return result


def descendants():
    processes = process_table()
    members = {os.getpid()}
    while True:
        additions = {pid for pid, item in processes.items() if item["parent"] in members}
        additions -= members
        if not additions:
            break
        members.update(additions)
    return {
        pid: processes[pid]
        for pid in members - {os.getpid()}
        if processes[pid]["state"] != "Z"
    }


def signal_descendants(kind):
    # Adoption covers children that changed session/group or outlived their original parent.
    # Verify start time immediately before signalling to avoid targeting a reused PID.
    members = descendants()
    for pid, item in members.items():
        current = process_entry(pid)
        if current and current["start"] == item["start"]:
            try:
                os.kill(pid, kind)
            except ProcessLookupError:
                pass


def reap_adopted_children(primary_pid):
    processes = process_table()
    for pid, item in processes.items():
        if pid != primary_pid and item["parent"] == os.getpid() and item["state"] == "Z":
            try:
                os.waitpid(pid, os.WNOHANG)
            except ChildProcessError:
                pass


def find_nvme_sensors():
    sensors = []
    for directory in sorted(Path("/sys/class/hwmon").glob("hwmon*")):
        if (directory / "name").read_text().strip() == "nvme":
            path = directory / "temp1_input"
            if not path.is_file():
                raise RuntimeError(f"NVMe composite sensor is missing at {directory}")
            sensors.append(path.resolve())
    if len(sensors) < 2:
        raise RuntimeError("both NVMe drive sensors must be visible")
    return sensors


def read_temperatures(sensors):
    readings = {}
    for path in sensors:
        prefix = path.name.removesuffix("_input")
        temperature = int(path.read_text().strip()) / 1000
        if not 0 < temperature < 150:
            raise RuntimeError(f"invalid NVMe temperature {temperature} C at {path}")
        alarm = path.with_name(prefix + "_alarm")
        if alarm.exists() and int(alarm.read_text().strip()) != 0:
            raise RuntimeError(f"NVMe alarm at {path}")
        ceiling = THERMAL_LIMIT_C
        maximum = path.with_name(prefix + "_max")
        if maximum.exists():
            value = int(maximum.read_text().strip()) / 1000
            if value > 0:
                ceiling = min(ceiling, value)
        if temperature >= ceiling:
            raise RuntimeError(f"NVMe temperature {temperature:.2f} C reached {ceiling:.2f} C at {path}")
        readings[str(path)] = temperature
    return readings


def run_guarded(command, cwd, output, read_sensors, seconds=MAX_SECONDS,
                interval=1.0, grace=TERMINATION_GRACE_SECONDS, interrupted=None):
    started = time.monotonic()
    deadline = started + seconds
    stop_file = output / "STOP"
    receipt = {
        "command": command,
        "working_directory": str(cwd),
        "guard_pid_in_namespace": os.getpid(),
        "stop_file": str(stop_file),
        "maximum_seconds": seconds,
        "driver_sha256": DRIVER_SHA256,
        "thermal_limit_c": THERMAL_LIMIT_C,
        "peak_temperatures_c": {},
        "result": "not_started",
    }

    def observe():
        readings = read_sensors()
        for name, value in readings.items():
            previous = receipt["peak_temperatures_c"].get(name, value)
            receipt["peak_temperatures_c"][name] = max(previous, value)

    def save():
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")

    try:
        observe()
    except (OSError, ValueError, RuntimeError) as error:
        receipt.update(result="blocked", reason=str(error))
        save()
        return receipt

    save()
    print(json.dumps({"event": "starting", "stop_from_host": f"touch {stop_file}",
                      "log": str(output / "test.log"), "maximum_seconds": seconds}), flush=True)
    with (output / "test.log").open("w") as log:
        child = subprocess.Popen(command, cwd=cwd, env=os.environ.copy(),
                                 stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        receipt["test_pid_in_namespace"] = child.pid
        reason = None
        kill_at = None
        killed_at = None
        try:
            save()
            while True:
                now = time.monotonic()
                code = child.poll()
                reap_adopted_children(child.pid)
                members = descendants()
                if code is not None and not members:
                    receipt["exit_code"] = code
                    break
                if reason is None:
                    try:
                        observe()
                    except (OSError, ValueError, RuntimeError) as error:
                        reason = f"thermal monitoring: {error}"
                    if stop_file.exists():
                        reason = "operator stop file"
                    elif interrupted and interrupted[0] is not None:
                        reason = f"guard received signal {interrupted[0]}"
                    elif now >= deadline - grace:
                        reason = "execution deadline"
                    elif code is not None and members:
                        reason = "test exited with live helper processes"
                    if reason:
                        receipt.update(result="aborted", reason=reason)
                        kill_at = min(now + grace, deadline)
                        signal_descendants(signal.SIGTERM)
                        save()
                elif now >= kill_at:
                    signal_descendants(signal.SIGKILL)
                    if killed_at is None:
                        killed_at = now
                    # Do not describe an uninterruptible kernel wait as confirmed cleanup.
                    if now - killed_at >= 5 and descendants():
                        receipt["lingering_pids_in_namespace"] = sorted(descendants())
                        receipt["exit_code"] = child.poll()
                        break
                time.sleep(min(interval, max(0.01, deadline - time.monotonic())))
        except BaseException:
            try:
                signal_descendants(signal.SIGKILL)
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
            raise

    receipt["elapsed_seconds"] = time.monotonic() - started
    if reason is None:
        receipt["result"] = "passed" if receipt["exit_code"] == 0 else "failed"
    save()
    print(json.dumps({
        "event": "process_finished", "result": receipt["result"],
        "elapsed_seconds": receipt["elapsed_seconds"],
        "peak_temperature_c": max(receipt["peak_temperatures_c"].values(), default=None),
        "reason": receipt.get("reason"), "receipt": str(output / "receipt.json"),
    }), flush=True)
    return receipt


def self_check(output):
    """Exercise only small Python parent/child workloads, never the Rust test binary."""
    cases = []
    healthy = lambda: {"synthetic_self_check_sensor": 35.0}

    def run(name, code, read=healthy, seconds=1.0):
        directory = output / name
        directory.mkdir()
        result = run_guarded([sys.executable, "-c", code], output, directory, read,
                             seconds=seconds, interval=0.02, grace=0.1)
        assert not descendants(), f"self-check leaked descendants: {name}"
        cases.append({"name": name, "result": result["result"], "reason": result.get("reason")})
        return result

    assert run("normal", "print('normal exit')")["result"] == "passed"
    assert run("deadline", "import time; time.sleep(60)", seconds=0.4)["reason"] == "execution deadline"
    stubborn = "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)"
    stopped = run("kill_escalation", stubborn, seconds=0.4)
    assert stopped["reason"] == "execution deadline" and stopped["exit_code"] == -signal.SIGKILL

    calls = 0

    def hot():
        nonlocal calls
        calls += 1
        if calls >= 3:
            raise RuntimeError("synthetic thermal limit")
        return healthy()

    assert "thermal monitoring" in run("thermal", "import time; time.sleep(60)", hot)["reason"]
    calls = 0

    def missing():
        nonlocal calls
        calls += 1
        if calls >= 3:
            raise OSError("synthetic sensor loss")
        return healthy()

    assert "thermal monitoring" in run("sensor_loss", "import time; time.sleep(60)", missing)["reason"]
    stop = output / "operator" / "STOP"
    code = f"from pathlib import Path; import time; Path({str(stop)!r}).touch(); time.sleep(60)"
    assert run("operator", code)["reason"] == "operator stop file"
    escape = ("import subprocess, sys, os; "
              "subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'], start_new_session=True); "
              "os._exit(0)")
    assert run("orphan_session", escape)["reason"] == "test exited with live helper processes"
    sensor = output / "sensor_fixture"
    sensor.mkdir()
    path = sensor / "temp1_input"
    path.write_text("35000\n")
    (sensor / "temp1_max").write_text("72850\n")
    (sensor / "temp1_alarm").write_text("0\n")
    assert read_temperatures([path]) == {str(path): 35.0}
    path.write_text("70000\n")
    try:
        read_temperatures([path])
    except RuntimeError as error:
        assert "reached 70.00 C" in str(error)
    else:
        raise AssertionError("temperature cutoff was not enforced")
    path.write_text("35000\n")
    (sensor / "temp1_alarm").write_text("1\n")
    try:
        read_temperatures([path])
    except RuntimeError as error:
        assert "NVMe alarm" in str(error)
    else:
        raise AssertionError("hardware alarm was not enforced")
    cases.append({"name": "temperature_parser", "result": "passed"})
    (output / "self-check.json").write_text(json.dumps(cases, indent=2) + "\n")
    print(json.dumps({"self_check": "passed", "cases": cases}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--manifest", type=Path)
    group.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    become_subreaper()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    if args.self_check:
        self_check(output)
        return 0

    manifest = json.loads(args.manifest.read_text())
    binary = Path(manifest["binary"])
    cwd = Path(manifest["working_directory"])
    names = manifest["tests"]
    if not binary.is_absolute() or not binary.is_file() or not cwd.is_dir():
        raise ValueError("an existing absolute binary and working directory are required")
    if not names or len(names) != len(set(names)) or any(
        not re.fullmatch(r"\w+(?:::\w+)+", name) or name.startswith("integration::") for name in names
    ):
        raise ValueError("provide unique exact test names without the integration:: prefix")
    threads = manifest.get("test_threads", 4)
    if not isinstance(threads, int) or not 1 <= threads <= 4:
        raise ValueError("this local profile permits one to four simultaneous test cases")
    sensors = find_nvme_sensors()
    os.environ["RUST_MIN_STACK"] = "33554432"
    os.environ["RUST_LOG"] = "warn"
    command = [str(binary), "--exact", *names, f"--test-threads={threads}",
               "--nocapture", "--report-time", "--color=never"]
    if manifest.get("drop_dac_capabilities", True):
        command = ["setpriv", "--bounding-set=-dac_override,-dac_read_search", *command]
    command = ["timeout", "--signal=TERM", "--kill-after=5s",
               f"{int(MAX_SECONDS - TERMINATION_GRACE_SECONDS)}s", *command]
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    interrupted = [None]
    for kind in [signal.SIGTERM, signal.SIGINT, signal.SIGHUP]:
        signal.signal(kind, lambda number, _frame: interrupted.__setitem__(0, number))
    result = run_guarded(command, cwd, output, lambda: read_temperatures(sensors),
                         interrupted=interrupted)
    if result["result"] == "passed":
        summaries = []
        with (output / "test.log").open() as log:
            for line in log:
                summaries.extend(re.findall(r"test result: ok; (\d+) passed; 0 failed; 0 ignored;", line))
        if summaries != [str(len(names))]:
            result.update(result="failed", reason="missing or unexpected executed-test count")
            (output / "receipt.json").write_text(json.dumps(result, indent=2) + "\n")
            return 1
        result["verified_test_count"] = len(names)
        (output / "receipt.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"event": "batch_verified", "passed": len(names),
                          "receipt": str(output / "receipt.json")}), flush=True)
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
