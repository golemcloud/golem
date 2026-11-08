"""Temporary, dispatch-only prime A -> warm A -> changed B campaign.

Launch-source selection remains a human decision: confirm A contains the reviewed
HTTP repair, and select another real reviewed native Rust change for B. Review
source.diff to rule out a comment-only or touch-only change; Git cannot prove that.
There is no cache-availability assumption and no full-cycle retry.
"""

import argparse
import hashlib
import importlib.util
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARNESS = Path(__file__).with_name("ci-experiment.py")
CONTRACTS = (".github/workflows/ci.yaml", "test-components/build-components.sh")
PHASES = (("prime", 330, 300), ("warm", 210, 180), ("changed", 210, 180))


def load_harness():
    spec = importlib.util.spec_from_file_location("campaign_experiment", HARNESS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def validate_hashes(a, b):
    if not all(re.fullmatch(r"[0-9a-fA-F]{40}", sha) for sha in (a, b)):
        raise ValueError("source_a and source_b must be full 40-hex commit SHAs")
    if a.lower() == b.lower():
        raise ValueError("A and B must differ")


def preflight(a, b, output):
    validate_hashes(a, b)
    for sha in (a, b):
        # Fetch only the supplied object, never a moving branch or tag.
        subprocess.run(
            ["git", "fetch", "--no-tags", "origin", sha], cwd=ROOT, check=True
        )
        if git("rev-parse", f"{sha}^{{commit}}") != sha.lower():
            raise ValueError("Input is not an exact commit")
    if git("rev-parse", "--is-shallow-repository") == "true":
        subprocess.run(
            ["git", "fetch", "--no-tags", "--unshallow", "origin", a, b],
            cwd=ROOT,
            check=True,
        )
    subprocess.run(["git", "merge-base", "--is-ancestor", a, b], cwd=ROOT, check=True)
    for path in CONTRACTS:
        if git("rev-parse", f"{a}:{path}") != git("rev-parse", f"{b}:{path}"):
            raise ValueError(f"Frozen workload contract changed: {path}")
        if git("rev-parse", f"{a}:{path}") != git("rev-parse", f"HEAD:{path}"):
            raise ValueError(f"Driver/workload contract mismatch: {path}")
    changed = git("diff", "--name-only", a, b).splitlines()
    native = [
        path
        for path in changed
        if path.endswith(".rs")
        and path.startswith(("golem-", "cli/"))
        and git("diff", "--no-ext-diff", a, b, "--", path)
    ]
    if not native:
        raise ValueError("B needs a nonempty native Rust source diff")
    output.mkdir(parents=True, exist_ok=True)
    (output / "source.diff").write_text(git("diff", "--binary", a, b))
    (output / "preflight.json").write_text(
        json.dumps(
            {
                "source_a": a.lower(),
                "source_b": b.lower(),
                "native_rust_changes": native,
                "launch_selection": "Human must confirm A contains the reviewed HTTP repair and B is another real reviewed change, not comments/touches",
            },
            indent=2,
        )
    )


def phase_seconds(start, total_minutes, phase_minutes, now):
    # Wall clock starts before checkout/setup, not at the first workload.
    return max(0, min(phase_minutes * 60, start + (total_minutes - 15) * 60 - now))


def minimum_seconds(platform, minimum):
    # APFS is a smaller workload within its own 120-minute allocation.
    return (minimum if platform == "linux" else 15) * 60


def reconcile_inventories(phase, inventory, baseline):
    if phase == "warm" and inventory != baseline:
        raise ValueError("Prime A and warm A test identities differ")
    if phase != "changed":
        return {}

    def counts(stages):
        return Counter(
            {
                (stage, suite, name): count
                for stage, tests in stages.items()
                for suite, name, count in tests
            }
        )

    before, after = counts(baseline), counts(inventory)
    return {
        "added": [
            list(key) + [count] for key, count in sorted((after - before).items())
        ],
        "removed": [
            list(key) + [count] for key, count in sorted((before - after).items())
        ],
    }


def admit(report, expected, sha, directory, report_jobs):
    stages = report.get("stages", [])
    ids = [stage["id"] for stage in stages]
    if not (
        report.get("success") is True
        and report.get("finished_at")
        and report.get("source_sha") == sha
        and report.get("test_retries") == 2
        and report.get("cli_test_threads") == 4
        and report.get("prerequisite_mode") == "serial"
        and report.get("phase") == "all"
        and report.get("workers") == 1
        and report.get("shard") == 0
        and len(ids) == len(expected)
        and set(ids) == set(expected)
    ):
        raise ValueError("Incomplete or mismatched campaign report")
    for stage in stages:
        if stage.get("status") not in {"passed", "passed_after_retry"} or not stage.get(
            "finished_at"
        ):
            raise ValueError(f"Unaccepted stage: {stage['id']}")
        if stage.get("validation_error") or not stage.get("steps"):
            raise ValueError(f"Missing or invalid command evidence: {stage['id']}")
        if any(step.get("attempt", -1) not in (0, 1, 2) for step in stage["steps"]):
            raise ValueError("Unbounded retry evidence")
        final_commands = {}
        for step in stage["steps"]:
            if step.get("validation_error"):
                raise ValueError(f"Invalid command evidence: {stage['id']}")
            index = step["log"].split("-", 1)[0].split(".", 1)[0]
            final_commands[index] = step
        if any(step.get("exit_code") != 0 for step in final_commands.values()):
            raise ValueError(f"Unrecovered command exit: {stage['id']}")
        if stage["id"].split("/")[0] not in report_jobs:
            continue
        initial = list(
            (directory / stage["id"].replace("/", "--")).glob("ctrf-*-0-*.json")
        )
        if not initial:
            raise ValueError(f"Missing initial test reports: {stage['id']}")
        executed = False
        for path in initial:
            results = json.loads(path.read_text())["results"]
            if results["summary"]["tests"] != len(results["tests"]):
                raise ValueError(f"Missing actual initial execution: {path}")
            executed |= any(
                t["status"] in {"passed", "failed"} for t in results["tests"]
            )
        if not executed:
            raise ValueError(f"Missing actual initial execution: {stage['id']}")
    # The immutable harness validates initial+focused-retry coverage before success.
    # fully_green is deliberately not an admission criterion.


def mac_execute(command, env, cwd, log, timeout):
    started = time.monotonic()
    if timeout <= 0:
        log.write_text("Allocation budget exhausted before invocation\n")
        return {"exit_code": 124, "seconds": 0, "log": log.name}
    with log.open("w") as output:
        process = subprocess.Popen(
            ["bash", "-euo", "pipefail", "-c", command],
            cwd=cwd,
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            code = process.wait(timeout=max(0, timeout))
        except subprocess.TimeoutExpired:
            code = 124
        finally:
            # Stop the test as well as its wrapper so the EXIT trap can detach.
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    pass
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
    return {"exit_code": code, "seconds": time.monotonic() - started, "log": log.name}


def mac_phase(workspace, directory, env, deadline):
    directory.mkdir(parents=True, exist_ok=True)
    attempts = []

    def run(command, name):
        observation = mac_execute(
            command, env, workspace, directory / name, deadline - time.time() - 15
        )
        observation["source_sha"] = env["GITHUB_SHA"]
        attempts.append(observation)
        (directory / "attempts.json").write_text(json.dumps(attempts, indent=2))
        return observation["exit_code"]

    cargo = shlex.quote(str(directory / "cargo.jsonl"))
    command = (
        "cargo test --locked -j 4 --profile dev-ci -p golem-worker-executor "
        "--features test-utils --lib --no-run --message-format=json-render-diagnostics "
        f"> {cargo}"
    )
    if run(command, "build.log"):
        return False
    selector = "integration-tests/scripts/managed-filesystem/"
    selection = (
        f"{selector}test-select-cargo-test-executables.py && "
        f'python3 {selector}select-cargo-test-executables.py {cargo} "$PWD" lib '
        f"> {shlex.quote(str(directory / 'executable.tsv'))}"
    )
    if run(selection, "select.log"):
        return False
    fields = (directory / "executable.tsv").read_text().strip().split("\t")
    if len(fields) != 2 or fields[0] != "lib":
        return False
    command = (
        "bash integration-tests/scripts/apfs/run-native-tests.sh "
        f"{shlex.quote(fields[1])} :tag:native_apfs --include-ignored "
        "--test-threads 1 --report-time"
    )
    for attempt in range(3):
        name = f"tests-{attempt}.log"
        code = run(command, name)
        text = (directory / name).read_text(errors="replace")
        if code == 0:
            return bool(re.search(r"test result: ok; [1-9]\d* passed;", text))
        if code not in (1, 101, 105) or not re.search(
            r"test result: FAILED;[^\n]*; [1-9]\d* failed;", text
        ):
            return False
    return False


def campaign(args):
    validate_hashes(args.source_a, args.source_b)
    output = ROOT / "tmp/experiment-results"
    output.mkdir(parents=True, exist_ok=True)
    started = float(args.allocation_start.read_text())
    experiment = load_harness()
    workflow = experiment.yaml.safe_load((ROOT / CONTRACTS[0]).read_text())
    chunks = json.loads(
        subprocess.check_output(
            ["bash", "build-components.sh", "list-groups"], cwd=ROOT / "test-components"
        )
    )
    stages = experiment.inventory(workflow, chunks)
    report_jobs = {
        name
        for name, job in workflow["jobs"].items()
        if any(
            step.get("uses", "").startswith("./.github/actions/publish-test-report")
            for step in job.get("steps", [])
        )
    }
    sources = [args.source_a.lower(), args.source_a.lower(), args.source_b.lower()]
    workspaces = {
        name: ROOT.parent / f"{ROOT.name}-campaign-{name}"
        for name in (
            ("main-suite", "sdk", "clippy")
            if args.platform == "linux"
            else ("main-suite",)
        )
    }
    summary = {
        "driver_sha": git("rev-parse", "HEAD"),
        "driver_digest": {
            path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in (
                Path(__file__),
                HARNESS,
                HARNESS.with_name("ci-overlap-experiment.py"),
            )
        },
        "platform": args.platform,
        "source_a": sources[0],
        "source_b": sources[2],
        "expected_stages": [s["id"] for s in stages]
        if args.platform == "linux"
        else ["native-apfs"],
        "phases": [],
        "success": False,
    }
    summary_path = output / f"campaign-{args.platform}.json"
    try:
        for sha in (sources[0], sources[2]):
            subprocess.run(
                ["git", "fetch", "--no-tags", "origin", sha], cwd=ROOT, check=True
            )
            for path in CONTRACTS:
                if git("rev-parse", f"{sha}:{path}") != git(
                    "rev-parse", f"HEAD:{path}"
                ):
                    raise ValueError(f"Driver/workload contract mismatch: {path}")
        for name, path in workspaces.items():
            # sbt's JGit needs a standalone repository, not a linked worktree.
            subprocess.run(
                ["git", "clone", "--shared", "--no-checkout", str(ROOT), str(path)],
                cwd=ROOT,
                check=True,
            )
        for (phase, cap, minimum), sha in zip(PHASES, sources):
            budget = phase_seconds(
                started, 720 if args.platform == "linux" else 120, cap, time.time()
            )
            record = {
                "phase": phase,
                "source_sha": sha,
                "budget_seconds": budget,
                "minimum_seconds": minimum_seconds(args.platform, minimum),
                "status": "incomplete_budget",
            }
            summary["phases"].append(record)
            summary_path.write_text(json.dumps(summary, indent=2))
            if budget < record["minimum_seconds"]:
                return 1
            record["status"] = "failed"
            deadline = time.time() + budget
            for path in workspaces.values():
                subprocess.run(
                    ["git", "checkout", "--force", "--detach", sha],
                    cwd=path,
                    check=True,
                )
                if git("-C", str(path), "rev-parse", "HEAD") != sha:
                    raise ValueError("Worktree source mismatch")
            for name in ("main-suite", "sdk"):
                if name in workspaces:
                    for relative in (
                        "tmp/experiment-products",
                        "tmp/test-components-wasm",
                    ):
                        product = workspaces[name] / relative
                        if product.exists():
                            shutil.rmtree(product)
            main = workspaces["main-suite"]
            env = {**os.environ, "GITHUB_SHA": sha, "GITHUB_WORKSPACE": str(main)}
            directory = output / phase
            (output / f"{phase}-inventory.json").write_text(
                json.dumps(
                    {
                        "source_sha": sha,
                        "expected_stages": summary["expected_stages"],
                        "workspaces": {
                            name: str(path) for name, path in workspaces.items()
                        },
                        "driver_digest": summary["driver_digest"],
                    },
                    indent=2,
                )
            )
            if args.platform == "linux":
                if phase == "prime":
                    sdk = workspaces["sdk"]
                    check = experiment.execute(
                        "sbt -batch -mem 4096 'show version'",
                        {**env, "GITHUB_WORKSPACE": str(sdk)},
                        sdk / "sdks/scala",
                        output / "scala-project-load.log",
                        max(0, min(600, deadline - time.time() - 30)),
                    )
                    summary["scala_project_load"] = {**check, "source_sha": sha}
                    summary_path.write_text(json.dumps(summary, indent=2))
                    if check["exit_code"]:
                        return 1
                command = shlex.join(
                    [
                        sys.executable,
                        "-u",
                        str(HARNESS),
                        "--workspace",
                        str(main),
                        "--cycle",
                        phase,
                        "--test-retries",
                        "2",
                        "--cli-test-threads",
                        "4",
                        "--clippy-workspace",
                        str(workspaces["clippy"]),
                        "--sdk-workspace",
                        str(workspaces["sdk"]),
                        "--prerequisite-mode",
                        "serial",
                    ]
                )
                observation = experiment.execute(
                    command,
                    env,
                    main,
                    output / f"{phase}.log",
                    max(0, deadline - time.time() - 30),
                )
                record["execution"] = observation
                actual = main / "tmp/experiment-results" / phase
                if actual.exists():
                    shutil.copytree(actual, directory)
                if observation["exit_code"]:
                    record["status"] = (
                        "incomplete_budget"
                        if observation["exit_code"] == 124
                        else "failed"
                    )
                    return 1
                report = json.loads((directory / "all-0.json").read_text())
                admit(
                    report,
                    summary["expected_stages"],
                    sha,
                    directory,
                    report_jobs,
                )
                inventory = {
                    stage["id"]: experiment.resolved_test_inventory(
                        directory / stage["id"].replace("/", "--"), stage
                    )
                    for stage in report["stages"]
                }
                (output / f"{phase}-test-inventory.json").write_text(
                    json.dumps(inventory, indent=2)
                )
                baseline = json.loads(
                    (output / "prime-test-inventory.json").read_text()
                )
                record["inventory_changes"] = reconcile_inventories(
                    phase, inventory, baseline
                )
                accepted = True
            else:
                accepted = mac_phase(main, directory, env, deadline)
            record["status"] = "accepted" if accepted else "failed"
            if (
                not accepted
                and (directory / "attempts.json").exists()
                and any(
                    a["exit_code"] == 124
                    for a in json.loads((directory / "attempts.json").read_text())
                )
            ):
                record["status"] = "incomplete_budget"
            if not accepted:
                return 1
        summary["success"] = True
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        summary["error"] = str(error)
        return 1
    finally:
        summary["finished_at"] = time.time()
        summary_path.write_text(json.dumps(summary, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-a", required=True)
    parser.add_argument("--source-b", required=True)
    parser.add_argument("--preflight", action="store_true")
    parser.add_argument("--platform", choices=("linux", "macos"), default="linux")
    parser.add_argument("--allocation-start", type=Path)
    args = parser.parse_args()
    if args.preflight:
        preflight(args.source_a, args.source_b, ROOT / "tmp/experiment-results")
        return 0
    if not args.allocation_start:
        parser.error("--allocation-start is required for workers")

    def interrupt(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupt)
    return campaign(args)


if __name__ == "__main__":
    raise SystemExit(main())
