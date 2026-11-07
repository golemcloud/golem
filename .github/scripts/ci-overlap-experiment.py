"""Gate and measure the CLI/SDK prerequisite window in disjoint workspaces."""

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
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARNESS = Path(__file__).with_name("ci-experiment.py")
SPEC = importlib.util.spec_from_file_location("experiment", HARNESS)
experiment = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(experiment)
CLI = experiment.PREREQUISITES[0]
SDKS = list(experiment.SDK_PRODUCTS)
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
        and bool(warm.get("sdk_workspace"))
        and cold.get("sdk_workspace") == warm["sdk_workspace"]
        and cold.get("prerequisite_mode") == warm.get("prerequisite_mode") == "serial"
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
        hideable = min(seconds[CLI], sum(seconds[s] for s in SDKS))
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


def run_window(
    root, sdk_root, directory, mode, retries=0, publication_id=None, clippy_root=None
):
    if mode not in {"serial", "overlap"}:
        raise ValueError(f"Unknown prerequisite mode: {mode}")
    roots = [root.resolve(), sdk_root.resolve()]
    if clippy_root:
        roots.append(Path(clippy_root).resolve())
    for i, left in enumerate(roots):
        for right in roots[i + 1 :]:
            if left.is_relative_to(right) or right.is_relative_to(left):
                raise ValueError("Prerequisite and Clippy workspaces must be disjoint")
    source_sha = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    if (
        subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=sdk_root, text=True
        ).strip()
        != source_sha
    ):
        raise ValueError("Prerequisite workspace source SHAs differ")
    workflow = experiment.yaml.safe_load(
        (root / ".github/workflows/ci.yaml").read_text()
    )
    if any(
        experiment.dependencies(workflow["jobs"][job])
        for job in experiment.PREREQUISITES
    ):
        raise ValueError(
            "Prerequisite window now has dependencies; review the allowlist"
        )
    experiment.adopt_orphans()
    directory.mkdir(parents=True)
    plan = [("cli", root, [CLI]), ("sdk", sdk_root, SDKS)]
    publication_id = publication_id or str(uuid.uuid4())
    for workspace in (root, sdk_root):
        products = workspace / "tmp/experiment-products"
        products.mkdir(parents=True, exist_ok=True)
        (products / "sdk-publication-id").write_text(publication_id)
        for job, files in experiment.SDK_PRODUCTS.items():
            (products / f"{job}.json").unlink(missing_ok=True)
            for name in files:
                (products / name).unlink(missing_ok=True)
    cycle = directory.parent.name + "-prerequisites"
    running = []
    outputs = []
    record = {
        "mode": mode,
        "passed": False,
        "source_sha": source_sha,
        "publication_id": publication_id,
        "started_at": time.time(),
        "samples": [],
        "lanes": [],
    }
    low_memory = 0
    deadline = time.monotonic() + 90 * 60

    def save():
        (directory / "slice.json").write_text(json.dumps(record, indent=2) + "\n")

    def launch(lane, workspace, stages):
        shutil.rmtree(workspace / "tmp/experiment-results" / cycle, ignore_errors=True)
        command = [
            sys.executable,
            "-u",
            str(HARNESS),
            "--workspace",
            str(workspace),
            "--cycle",
            cycle,
            "--test-retries",
            str(retries),
        ]
        for stage in stages:
            command.extend(["--stage", stage])
        output = (directory / f"{lane}.log").open("w")
        outputs.append(output)
        process = subprocess.Popen(
            command,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            env={**os.environ, "CI_PREREQUISITE_ID": publication_id},
        )
        running.append(process)
        record["lanes"].append(
            {"lane": lane, "pid": process.pid, "started_at": time.time()}
        )

    initial = experiment.host_sample(root)
    if (
        initial["memory_available_kib"] < initial["memory_total_kib"] * 0.20
        or initial["disk_free_bytes"] < initial["disk_total_bytes"] * 0.20
    ):
        raise RuntimeError(
            "Insufficient memory/disk headroom for prerequisite admission"
        )
    record["resources_before"] = resource_boundary()
    try:
        launch(*plan[0])
        if mode == "overlap":
            launch(*plan[1])
        while True:
            sample = {"time": time.time(), **experiment.host_sample(root)}
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
                raise TimeoutError("Prerequisite window exceeded 90-minute limit")
            for process, lane in zip(running, record["lanes"]):
                if process.poll() is not None and "finished_at" not in lane:
                    lane["finished_at"] = time.time()
            if all(p.poll() is not None for p in running):
                if mode == "serial" and len(running) == 1:
                    launch(*plan[1])
                else:
                    break
            time.sleep(5)
        record["resources_after"] = resource_boundary()
        if record["resources_before"] != record["resources_after"]:
            raise RuntimeError(
                "Slice left changed listeners or containers; refusing to continue"
            )
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
            try:
                for lane, workspace, _ in plan:
                    child_results = workspace / "tmp/experiment-results" / cycle
                    if child_results.exists():
                        shutil.copytree(child_results, directory / f"{lane}-results")
            finally:
                save()
            if cleanup_errors:
                raise RuntimeError(f"Slice cleanup failed: {cleanup_errors}")
        finally:
            for sig, handler in handlers.items():
                signal.signal(sig, handler)

    stages = []
    for (lane, _, expected), outcome in zip(plan, record["lanes"]):
        evidence = directory / f"{lane}-results"
        report = json.loads((evidence / "all-0.json").read_text())
        if (
            report["source_sha"] != source_sha
            or not report.get("finished_at")
            or report["cli_test_threads"] != 4
            or report["test_retries"] != retries
            or [s["id"] for s in report["stages"]] != expected
            or outcome["exit_code"] != int(not report["success"])
        ):
            raise ValueError(f"Incomplete or mismatched prerequisite report: {lane}")
        stages.extend(report["stages"])
        for stage in report["stages"]:
            name = stage["id"].replace("/", "--")
            shutil.copytree(evidence / name, directory.parent / name)
            if stage["id"] in SDKS and stage["status"] in {
                "passed",
                "passed_after_retry",
            }:
                origin = sdk_root / "tmp/experiment-products"
                products = root / "tmp/experiment-products"
                for name in (
                    *experiment.SDK_PRODUCTS[stage["id"]],
                    f"{stage['id']}.json",
                ):
                    shutil.copy2(origin / name, products / name)
                experiment.restore_sdk(stage["id"], root)
                record.setdefault("sdk_publications", {})[stage["id"]] = json.loads(
                    (products / f"{stage['id']}.json").read_text()
                )
    record["passed"] = all(
        s["status"] in {"passed", "passed_after_retry"} for s in stages
    )
    record["finished_at"] = time.time()
    record["seconds"] = record["finished_at"] - record["started_at"]
    save()
    return stages


def slices(sdk_root, selected_mode="overlap"):
    warm = json.loads((RESULTS / "warm/all-0.json").read_text())
    if warm.get("sdk_workspace") != str(sdk_root):
        raise ValueError("SDK workspace differs from the admitted baseline")
    for root in (ROOT, sdk_root):
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
    reserve = 195 if selected_mode == "serial-slice" else 105
    if time.time() - start > (12 * 60 - reserve) * 60:
        (RESULTS / "overlap-skipped.json").write_text(
            '{"reason":"insufficient remaining job budget"}\n'
        )
        return
    stages = run_window(
        ROOT,
        sdk_root,
        RESULTS / selected_mode / "prerequisites",
        "serial" if selected_mode == "serial-slice" else "overlap",
    )
    (RESULTS / selected_mode / "all-0.json").write_text(
        json.dumps(
            {
                "source_sha": warm["source_sha"],
                "finished_at": time.time(),
                "fully_green": all(s["status"] == "passed" for s in stages),
                "cli_test_threads": 4,
                "test_retries": 0,
                "stages": stages,
            },
            indent=2,
        )
    )
    if any(s["status"] != "passed" for s in stages):
        raise ValueError("Prerequisite slice did not pass cleanly")
    if selected_mode == "serial-slice":
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            print("completed=true", file=output)
        return
    inventories = []
    caches = []
    for mode in ("serial-slice", "overlap"):
        stages = {}
        for root in (RESULTS / mode,):
            report = json.loads((root / "all-0.json").read_text())
            if (
                not report["fully_green"]
                or report["test_retries"]
                or report["cli_test_threads"] != 4
                or report["source_sha"] != warm["source_sha"]
                or not report.get("finished_at")
                or any(s["status"] != "passed" for s in report["stages"])
            ):
                raise ValueError(f"Slice did not pass cleanly: {mode}")
            for stage in report["stages"]:
                stages[stage["id"]] = test_inventory(
                    root / stage["id"].replace("/", "--"), stage
                )
        if set(stages) != {CLI, *SDKS} or len(report["stages"]) != len(
            experiment.PREREQUISITES
        ):
            raise ValueError(f"Incomplete slice: {mode}")
        for stage in warm["stages"]:
            if stage["id"] in experiment.PREREQUISITES and stages[
                stage["id"]
            ] != test_inventory(
                RESULTS / "warm" / stage["id"].replace("/", "--"), stage
            ):
                raise ValueError(
                    f"Slice differs from admitted warm test inventory: {mode} {stage['id']}"
                )
        inventories.append(stages)
        caches.append({s["id"]: s["cache_before"] for s in report["stages"]})
    if inventories[0] != inventories[1]:
        raise ValueError("Serial/overlap test inventories differ")
    (RESULTS / "slice-comparison.json").write_text(
        json.dumps(
            {
                "inventories_match": True,
                "cache_input_markers_match": caches[0] == caches[1],
                "cache_evidence_limit": "Fingerprint markers are necessary, not complete cache identity proof; inspect dependency/cache warming before attributing a speedup.",
                "serial_seconds": json.loads(
                    (RESULTS / "serial-slice/prerequisites/slice.json").read_text()
                )["seconds"],
                "overlap_seconds": json.loads(
                    (RESULTS / "overlap/prerequisites/slice.json").read_text()
                )["seconds"],
            },
            indent=2,
        )
        + "\n"
    )
    if caches[0] != caches[1]:
        raise ValueError("Prerequisite cache inputs changed between serial and overlap")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("gate", "serial-slice", "overlap"))
    parser.add_argument("--sdk-workspace", type=Path, required=True)
    args = parser.parse_args()
    if os.getenv("GITHUB_ACTIONS") != "true":
        raise SystemExit("Execution requires the isolated experiment worker")
    signal.signal(signal.SIGTERM, interrupt)
    if args.mode == "gate":
        gate()
    else:
        slices(args.sdk_workspace.resolve(), args.mode)
