"""Gate and measure a Clippy/service slice on one worker with disjoint workspaces."""

import argparse
import collections
import importlib.util
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARNESS = Path(__file__).with_name("ci-experiment.py")
SPEC = importlib.util.spec_from_file_location("experiment", HARNESS)
experiment = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(experiment)
CLIPPY = "check-clippy-and-format"
SERVICES = ["it/integration-tests-group2", "it/integration-tests-group3"]
RESULTS = ROOT / "tmp/experiment-results"


def interrupt(*_):
    raise KeyboardInterrupt


def test_inventory(directory, stage):
    attempts = {}
    for step in stage["steps"]:
        # Report files use the ordinal command index, not retry-expanded steps.
        index = int(step["log"].split("-", 1)[0].split(".", 1)[0])
        attempts[index] = step["attempt"]
    tests = collections.Counter()
    for path in directory.glob("ctrf-*.json"):
        match = re.fullmatch(r"ctrf-(\d+)-(\d+)-(\d+)\.json", path.name)
        index, attempt, _ = map(int, match.groups())
        if attempts.get(index) != attempt:
            continue
        report = json.loads(path.read_text())["results"]
        if report["summary"]["tests"] != len(report["tests"]):
            raise ValueError(f"Inconsistent CTRF inventory: {path}")
        tests.update(
            (t.get("suite", ""), t["name"], t["status"]) for t in report["tests"]
        )
    return sorted([*key, count] for key, count in tests.items())


def complete_baseline(cold, warm, expected):
    def complete(report):
        ids = [s["id"] for s in report.get("stages", [])]
        return (
            bool(report.get("finished_at"))
            and report.get("success") is True
            and len(ids) == len(expected)
            and set(ids) == set(expected)
        )

    return (
        complete(cold)
        and complete(warm)
        and cold["source_sha"] == warm["source_sha"]
        and warm.get("fully_green") is True
        and warm.get("test_retries") == 0
        and warm.get("cli_test_threads") == 4
        and all(s["status"] == "passed" for s in warm["stages"])
    )


def gate():
    cold = json.loads((RESULTS / "cold/all-0.json").read_text())
    warm = json.loads((RESULTS / "warm/all-0.json").read_text())
    expected = [
        s["id"] for s in json.loads((RESULTS / "inventory.json").read_text())["stages"]
    ]
    decision = {"eligible": False, "reason": "baseline is not complete and clean"}
    if complete_baseline(cold, warm, expected):
        for before, after in zip(cold["stages"], warm["stages"]):
            if before["id"] != after["id"]:
                raise ValueError("Stage order differs between cold and warm")
            name = before["id"].replace("/", "--")
            inventory = test_inventory(RESULTS / "cold" / name, before)
            if inventory != test_inventory(RESULTS / "warm" / name, after):
                raise ValueError(f"Cold/warm test inventory differs: {name}")
            if (
                before["id"].startswith(
                    (
                        "worker-tests/",
                        "it/",
                        "it-cli/",
                        "it-cli-agents/",
                        "it-cli-streaming/",
                    )
                )
                and not inventory
            ):
                raise ValueError(f"Missing test inventory: {name}")
        seconds = {
            s["id"]: sum(t["seconds"] for t in s["steps"]) for s in warm["stages"]
        }
        hideable = min(seconds[CLIPPY], sum(seconds[s] for s in SERVICES))
        decision = {
            "eligible": hideable >= 120,
            "reason": "eligible"
            if hideable >= 120
            else "less than two minutes hideable",
            "maximum_hideable_seconds": hideable,
        }
    (RESULTS / "overlap-gate.json").write_text(json.dumps(decision, indent=2) + "\n")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        print(f"eligible={str(decision['eligible']).lower()}", file=output)
    print(json.dumps(decision), flush=True)


def resource_boundary():
    return {
        "listeners": sorted(
            subprocess.check_output(
                ["ss", "-H", "-ltn"], text=True, timeout=15
            ).splitlines()
        ),
        "containers": sorted(
            subprocess.check_output(
                ["docker", "ps", "-aq", "--no-trunc"], text=True, timeout=15
            ).splitlines()
        ),
    }


def run_slice(mode, clippy_root):
    experiment.adopt_orphans()
    directory = RESULTS / mode
    directory.mkdir()
    plan = [("clippy", clippy_root, [CLIPPY]), ("services", ROOT, SERVICES)]
    running = []
    outputs = []
    record = {"mode": mode, "started_at": time.time(), "samples": [], "lanes": []}
    low_memory = 0
    deadline = time.monotonic() + 35 * 60

    def save():
        (directory / "slice.json").write_text(json.dumps(record, indent=2) + "\n")

    def launch(lane, workspace, stages):
        command = [
            sys.executable,
            "-u",
            str(HARNESS),
            "--workspace",
            str(workspace),
            "--cycle",
            mode,
        ]
        for stage in stages:
            command.extend(["--stage", stage])
        output = (directory / f"{lane}.log").open("w")
        outputs.append(output)
        process = subprocess.Popen(
            command, stdout=output, stderr=subprocess.STDOUT, start_new_session=True
        )
        running.append(process)
        record["lanes"].append(
            {"lane": lane, "pid": process.pid, "started_at": time.time()}
        )

    initial = experiment.host_sample(ROOT)
    if (
        initial["memory_available_kib"] < initial["memory_total_kib"] * 0.20
        or initial["disk_free_bytes"] < initial["disk_total_bytes"] * 0.20
    ):
        raise RuntimeError("Insufficient memory/disk headroom for slice admission")
    record["resources_before"] = resource_boundary()
    try:
        launch(*plan[0])
        if mode == "overlap":
            launch(*plan[1])
        while True:
            sample = {"time": time.time(), **experiment.host_sample(ROOT)}
            table = experiment.processes()
            sample["owned_processes"] = {
                str(p): table[p]
                for process in running
                for p in experiment.descendants(process.pid, table)
                if p in table
            }
            record["samples"].append(sample)
            save()
            low_memory = (
                low_memory + 1
                if sample["memory_available_kib"] < sample["memory_total_kib"] * 0.15
                else 0
            )
            if (
                low_memory >= 3
                or sample["disk_free_bytes"] < sample["disk_total_bytes"] * 0.05
                or any(
                    line.startswith(("oom ", "oom_kill "))
                    and line not in initial["oom_events"].splitlines()
                    for line in sample["oom_events"].splitlines()
                )
            ):
                raise RuntimeError("Slice resource safety guard triggered")
            if time.monotonic() >= deadline:
                raise TimeoutError("Slice exceeded 35-minute limit")
            if any(p.poll() not in (None, 0) for p in running):
                raise RuntimeError("Slice child failed")
            if all(p.poll() is not None for p in running):
                if mode == "serial-slice" and len(running) == 1:
                    record["lanes"][0]["finished_at"] = time.time()
                    launch(*plan[1])
                else:
                    break
            time.sleep(5)
        record["resources_after"] = resource_boundary()
        if record["resources_before"] != record["resources_after"]:
            raise RuntimeError(
                "Slice left changed listeners or containers; refusing to continue"
            )
        record["passed"] = True
    except BaseException as error:
        record["passed"] = False
        record["error"] = str(error) or type(error).__name__
        raise
    finally:
        handlers = {
            s: signal.signal(s, signal.SIG_IGN) for s in (signal.SIGTERM, signal.SIGINT)
        }
        cleanup_errors = []
        try:
            try:
                experiment.terminate_children(running)
            except (OSError, RuntimeError) as error:
                cleanup_errors.append(str(error))
            try:
                boundary = resource_boundary()
                record["resources_before_cleanup"] = boundary
                # This VM is exclusive to the experiment. Preserve every pre-existing container.
                containers = set(boundary["containers"]) - set(
                    record["resources_before"]["containers"]
                )
                if containers:
                    subprocess.run(
                        ["docker", "rm", "-f", *sorted(containers)],
                        check=True,
                        timeout=30,
                    )
                record["resources_final"] = resource_boundary()
                if record["resources_before"] != record["resources_final"]:
                    cleanup_errors.append("Listeners/containers differ after cleanup")
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                cleanup_errors.append(str(error))
            for output in outputs:
                output.close()
            record["finished_at"] = time.time()
            record["seconds"] = record["finished_at"] - record["started_at"]
            for lane in record["lanes"]:
                lane["exit_code"] = next(
                    p.returncode for p in running if p.pid == lane["pid"]
                )
            record["cleanup_errors"] = cleanup_errors
            if cleanup_errors:
                record["passed"] = False
            # Keep the Clippy child's complete raw reports in the uploaded source tree.
            try:
                child_results = clippy_root / "tmp/experiment-results" / mode
                if child_results.exists():
                    shutil.copytree(child_results, directory / "clippy-results")
            finally:
                save()
            if cleanup_errors:
                raise RuntimeError(f"Slice cleanup failed: {cleanup_errors}")
        finally:
            for sig, handler in handlers.items():
                signal.signal(sig, handler)


def slices(clippy_root, selected_mode="overlap"):
    if (
        ROOT == clippy_root
        or ROOT in clippy_root.parents
        or clippy_root in ROOT.parents
    ):
        raise ValueError("Clippy and suite workspaces must be disjoint")
    warm = json.loads((RESULTS / "warm/all-0.json").read_text())
    for root in (ROOT, clippy_root):
        if (
            subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=root, text=True
            ).strip()
            != warm["source_sha"]
        ):
            raise ValueError("Slice source differs from the admitted baseline")
    decision = json.loads((RESULTS / "overlap-gate.json").read_text())
    if not decision["eligible"]:
        raise RuntimeError("Overlap gate did not admit a comparison")
    start = int((RESULTS / "allocation-start").read_text())
    # Reserve each bounded slice and at least 15 minutes for finalization.
    reserve = 90 if selected_mode == "serial-slice" else 45
    if time.time() - start > (12 * 60 - reserve) * 60:
        (RESULTS / "overlap-skipped.json").write_text(
            '{"reason":"insufficient remaining job budget"}\n'
        )
        return
    run_slice(selected_mode, clippy_root)
    if selected_mode == "serial-slice":
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            print("completed=true", file=output)
        return
    inventories = []
    caches = []
    for mode in ("serial-slice", "overlap"):
        stages = {}
        for root in (RESULTS / mode, RESULTS / mode / "clippy-results"):
            report = json.loads((root / "all-0.json").read_text())
            if (
                not report["fully_green"]
                or report["test_retries"]
                or report["source_sha"] != warm["source_sha"]
            ):
                raise ValueError(f"Slice did not pass cleanly: {mode}")
            for stage in report["stages"]:
                stages[stage["id"]] = test_inventory(
                    root / stage["id"].replace("/", "--"), stage
                )
                if stage["id"] == CLIPPY:
                    caches.append(stage["cache_before"])
        if set(stages) != {CLIPPY, *SERVICES} or any(not stages[s] for s in SERVICES):
            raise ValueError(f"Incomplete slice: {mode}")
        for stage in warm["stages"]:
            if stage["id"] in SERVICES and stages[stage["id"]] != test_inventory(
                RESULTS / "warm" / stage["id"].replace("/", "--"), stage
            ):
                raise ValueError(
                    f"Slice differs from admitted warm test inventory: {mode} {stage['id']}"
                )
        inventories.append(stages)
    if inventories[0] != inventories[1]:
        raise ValueError("Serial/overlap test inventories differ")
    (RESULTS / "slice-comparison.json").write_text(
        json.dumps(
            {
                "inventories_match": True,
                "clippy_rebuild_markers_match": caches[0] == caches[1],
            },
            indent=2,
        )
        + "\n"
    )
    if caches[0] != caches[1]:
        raise ValueError(
            "Clippy compilation cache changed between serial and overlap inputs"
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("gate", "serial-slice", "overlap"))
    parser.add_argument("--clippy-workspace", type=Path, required=True)
    args = parser.parse_args()
    if os.getenv("GITHUB_ACTIONS") != "true":
        raise SystemExit("Execution requires the isolated experiment worker")
    signal.signal(signal.SIGTERM, interrupt)
    if args.mode == "gate":
        gate()
    else:
        slices(args.clippy_workspace.resolve(), args.mode)
