#!/usr/bin/env python3
"""Run ordinary CI commands on a persistent worker, retaining per-stage evidence.

The workflow remains the test-selection source of truth. Setup and transport are
explicitly replaced; unknown jobs, actions, and expressions fail the inventory.
"""

import argparse
import ast
import copy
import ctypes
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import time
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
EXCLUDED = {
    "build-apfs-test-bin": "separate macOS job",
    "apfs-tests": "separate macOS job",
    "release-prerequisites": "release-only",
    "builtin-artifacts-release-tests": "release-only",
    "delete-sticky-disks": "replaced transport",
    "consolidate-test-reports": "reports retained per stage",
    "detect-skill-changes": "run local harness unit checks unconditionally",
    "list-test-component-groups": "expanded from build-components.sh",
}
PREPARE = {
    "builtin-artifacts",
    "build-and-store",
    "build-executor-tests",
    "build-service-tests",
    "build-cli-test-bins",
    "build-golem-ts",
    "build-golem-effect",
    "build-golem-scala",
    "build-test-components",
    "merge-test-components",
}
CLI_JOBS = {"it-cli", "it-cli-agents", "it-cli-streaming"}
EXPECTED = (
    PREPARE
    | set(EXCLUDED)
    | {
        "check-clippy-and-format",
        "unit-tests-and-checks",
        "config-checks",
        "middleware-unit-tests",
        "builtin-artifacts-embedded-tests",
        "worker-tests",
        "managed-xfs-tests",
        "it",
        "it-cli",
        "it-cli-agents",
        "it-cli-streaming",
        "scala-sdk-integration-tests",
        "golem-schema-guest",
        "golem-client",
        "build-golem-moonbit",
        "build-golem-rust",
        "build-golem-go",
        "skill-harness-unit-tests",
        "docker-image-checks",
    }
)
SETUP = {
    "actions/checkout",
    "actions/cache",
    "actions/setup-go",
    "actions/setup-java",
    "actions/setup-node",
    "coursier/cache-action",
    "hustcer/setup-moonbit",
    "oven-sh/setup-bun",
    "sbt/setup-sbt",
    "shogo82148/actions-setup-redis",
    "./.github/actions/setup-rust",
    "./.github/actions/setup-js-wasm-tools",
    "./.github/actions/setup-wasm-component-tools",
}
TRANSPORT = {
    "actions/upload-artifact",
    "actions/download-artifact",
    "useblacksmith/stickydisk",
    "./.github/actions/publish-test-report",
}
EXPRESSION = re.compile(r"\$\{\{\s*(.*?)\s*\}\}")


def resolve(key, context):
    value = context
    for part in key.split("."):
        if part not in value:
            # Optional properties in the source fixture matrix evaluate empty.
            if key.startswith("matrix."):
                return ""
            raise ValueError(f"Unsupported expression: {key}")
        value = value[part]
    return value


def render(value, context):
    if not isinstance(value, str):
        return str(value).lower() if isinstance(value, bool) else str(value)
    return EXPRESSION.sub(lambda m: str(resolve(m[1], context)), value)


def condition(value, context):
    if not value:
        return True
    text = str(value).removeprefix("${{").removesuffix("}}").strip()
    text = re.sub(
        r"(?:matrix|steps)\.[\w.-]+", lambda m: repr(resolve(m[0], context)), text
    )
    text = text.replace("&&", " and ").replace("||", " or ")
    tree = ast.parse(text, mode="eval")
    allowed = (
        ast.Expression,
        ast.BoolOp,
        ast.And,
        ast.Or,
        ast.Compare,
        ast.Eq,
        ast.NotEq,
        ast.Constant,
        ast.Call,
        ast.Name,
        ast.Load,
    )
    for node in ast.walk(tree):
        if not isinstance(node, allowed):
            raise TypeError(f"Unsupported condition: {value}")
        if isinstance(node, ast.Name) and node.id != "startsWith":
            raise ValueError(f"Unsupported condition: {value}")
        if isinstance(node, ast.Call) and (
            not isinstance(node.func, ast.Name) or node.func.id != "startsWith"
        ):
            raise ValueError(f"Unsupported condition: {value}")
    return bool(
        eval(
            compile(tree, "<condition>", "eval"),
            {"__builtins__": {}, "startsWith": lambda a, b: a.startswith(b)},
        )
    )


def dependencies(job):
    needs = job.get("needs", [])
    return [needs] if isinstance(needs, str) else needs


def inventory(workflow, chunks):
    jobs = workflow["jobs"]
    if set(jobs) != EXPECTED:
        raise ValueError(f"CI job inventory changed: {set(jobs) ^ EXPECTED}")
    stages = []
    remaining = {k: v for k, v in jobs.items() if k not in EXCLUDED}
    completed = set(EXCLUDED)
    # Preparation first makes publications available for consumer-only trials.
    while remaining:
        ready = [k for k, j in remaining.items() if set(dependencies(j)) <= completed]
        if not ready:
            raise ValueError("Unresolved CI dependencies")
        ready.sort(key=lambda k: k not in PREPARE)
        name = ready[0]
        job = remaining.pop(name)
        matrix = job.get("strategy", {}).get("matrix", {})
        if name == "build-test-components":
            matrix = {"chunk": chunks}
        if len(matrix) > 1:
            raise ValueError(f"Unsupported multidimensional matrix: {name}")
        dimension, entries = next(iter(matrix.items()), ("", [{}]))
        for entry in entries:
            suffix = f"/{entry['name']}" if dimension else ""
            stages.append(
                {
                    "id": name + suffix,
                    "job": name,
                    "definition": job,
                    "matrix": {dimension: entry} if dimension else {},
                }
            )
        completed.add(name)
    return stages


def select_stages(stages, phase, workers=1, shard=0):
    cli_closure = set(CLI_JOBS)
    for stage in reversed(stages):
        if stage["job"] in cli_closure:
            cli_closure.update(dependencies(stage["definition"]))
    selected = [
        s
        for s in stages
        if phase == "all"
        or (phase == "cli-prime" and s["job"] in cli_closure)
        or (phase == "cli" and s["job"] in CLI_JOBS)
        or (
            phase in {"prepare", "checks"}
            and (s["job"] in PREPARE) == (phase == "prepare")
        )
    ]
    if phase == "checks":
        selected = [s for i, s in enumerate(selected) if i % workers == shard]
    return selected


def cli_baseline_eligible(report, stages):
    expected = {s["id"] for s in stages if s["job"] in CLI_JOBS}
    passed = {s["id"] for s in report.get("stages", []) if s["status"] == "passed"}
    return (
        bool(expected)
        and bool(report.get("finished_at"))
        and report.get("cli_test_threads") == 4
        and expected <= passed
    )


def commands(stage, context, cli_test_threads=4):
    """Translate only setup/publication boundaries; preserve validation commands."""
    job = stage["job"]
    for step in stage["definition"]["steps"]:
        action = step.get("uses", "").split("@")[0]
        name = step.get("name", action)
        if action in SETUP | TRANSPORT:
            continue
        if name in {"Compute built-in artifact cache key", "Identify fixture compiler"}:
            continue
        # Producer publication blocks follow all validation in these jobs.
        if name.startswith("Identify ") and job in PREPARE:
            break
        # These unpack runner-to-runner transports; data already exists locally.
        if name.startswith("Extract ") or name == "Add golem to PATH":
            continue
        if not condition(step.get("if"), context):
            continue
        if action == "./.github/actions/publish-native-binaries":
            selection = step["with"]["cargo-args"]
            yield {
                "name": "Build and archive native selection",
                "run": f"cargo make --profile ci wit\ncargo build --profile dev-ci {selection}\n"
                f"cargo nextest archive --archive-file tmp/experiment-products/{job}.tar.zst "
                f"--zstd-level 1 --cargo-profile dev-ci {selection}",
            }
        elif action == "./.github/actions/restore-binaries":
            producer = re.search(r"needs\.([\w-]+)\.outputs", step["with"]["disk-key"])[
                1
            ]
            command = (
                "rm -rf target/nextest-archive\n"
                f"cargo-test-r reuse-nextest-archive --archive-file tmp/experiment-products/{producer}.tar.zst"
            )
            if step["with"].get("copy-to-target") == "true":
                # Restored binaries must not inherit fingerprints from a different
                # feature build. Keep the native dev-ci compilation cache separate.
                command += (
                    "\nrm -rf target/debug\nmkdir -p target/debug\nfind target/nextest-archive/target/dev-ci "
                    "-maxdepth 1 -type f -exec cp -t target/debug/ {} +"
                )
            yield {"name": "Select exact native archive", "run": command}
        elif action == "./.github/actions/restore-cli-test-binary":
            destination = step["with"]["destination"]
            yield {
                "name": "Install optimized CLI",
                "run": f"mkdir -p {destination}\ncp tmp/experiment-products/golem{{,-cli}} {destination}/",
            }
        elif action == "./.github/actions/restore-test-components":
            yield {
                "name": "Install merged fixtures",
                "run": "cp tmp/experiment-products/fixtures/*.wasm test-components/",
            }
        elif action == "./.github/actions/restore-builtin-artifacts":
            yield {
                "name": "Select prebuilt built-ins",
                "run": "true",
                "env": {"GOLEM_BUILTIN_ARTIFACTS_PREBUILT": "1"},
            }
        elif action == "./.github/actions/restore-binary-publication":
            if step["with"]["file"] != "scala-sdk-ivy-local.tar.gz":
                raise ValueError(f"Unknown publication: {step}")
            # The producing Scala stage already published into ~/.ivy2/local.
            continue
        elif action == "./.github/actions/check-scala-sdk-generated":
            yield {"name": name, "run": "git diff --exit-code sdks/scala/wit/dts"}
        elif action == "golangci/golangci-lint-action":
            yield {
                "name": name,
                "run": "golangci-lint run",
                "working-directory": step["with"]["working-directory"],
            }
        elif action:
            raise ValueError(f"Unsupported action in {job}: {action}")
        elif "run" in step:
            command = copy.deepcopy(step)
            if (
                job in CLI_JOBS
                and cli_test_threads != 4
                and "cargo make --profile ci " in command["run"]
            ):
                # The ci profile overrides --env. Source restoration resets this
                # experiment-only edit before each logical stage.
                command["run"] = f"""python3 - <<'PY'
from pathlib import Path
path = Path('Makefile.toml')
source = path.read_text()
old = 'CLI_TEST_THREADS = "4"'
assert source.count(old) == 1, 'CLI concurrency baseline changed'
path.write_text(source.replace(old, 'CLI_TEST_THREADS = "{cli_test_threads}"'))
PY
""" + command["run"]
            yield command
        else:
            raise ValueError(f"Unclassified step in {job}: {step}")


def cache_fingerprints(root):
    return {
        str(path.relative_to(root)): [
            hashlib.sha256(path.read_bytes()).hexdigest(),
            path.stat().st_mtime_ns,
        ]
        for path in root.glob("**/target/debug/.fingerprint/*/invoked.timestamp")
    }


def interrupt(*_):
    raise KeyboardInterrupt


def adopt_orphans():
    # Detached test dependencies must remain owned after their launcher exits.
    if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "Cannot enable child subreaper")


def processes():
    result = {}
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            text = path.read_text()
            fields = text[text.rindex(")") + 2 :].split()
            result[int(path.parent.name)] = {
                "parent": int(fields[1]),
                "state": fields[0],
                "start": fields[19],
            }
        except (FileNotFoundError, ProcessLookupError):
            continue
    return result


def descendants(pid, table):
    owned = {pid}
    while True:
        found = owned | {p for p, info in table.items() if info["parent"] in owned}
        if found == owned:
            return owned - {pid}
        owned = found


def terminate_children(leaders):
    handlers = {
        s: signal.signal(s, signal.SIG_IGN) for s in (signal.SIGTERM, signal.SIGINT)
    }
    try:
        for sig in (signal.SIGTERM, signal.SIGKILL):
            deadline = time.monotonic() + 3
            while True:
                for process in leaders:
                    process.poll()
                table = processes()
                owned = descendants(os.getpid(), table)
                for pid in owned:
                    try:
                        if table[pid]["state"] == "Z":
                            if pid not in {p.pid for p in leaders}:
                                os.waitpid(pid, os.WNOHANG)
                        elif (
                            processes().get(pid, {}).get("start") == table[pid]["start"]
                        ):
                            os.kill(pid, sig)
                    except (ProcessLookupError, ChildProcessError):
                        pass
                if not owned:
                    return
                if time.monotonic() >= deadline:
                    break
                time.sleep(0.1)
        if descendants(os.getpid(), processes()):
            raise RuntimeError("Owned test processes survived bounded cleanup")
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def host_sample(cwd):
    memory = dict(
        line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines()
    )
    total = int(memory["MemTotal"].split()[0])
    available = int(memory["MemAvailable"].split()[0])
    group = Path("/sys/fs/cgroup")
    for line in Path("/proc/self/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            candidate = group / line[3:].lstrip("/")
            if candidate.exists():
                group = candidate
    for parent in (group, *group.parents):
        if not parent.is_relative_to("/sys/fs/cgroup"):
            break
        if (parent / "memory.max").exists():
            limit = (parent / "memory.max").read_text().strip()
            if limit != "max":
                total = min(total, int(limit) // 1024)
                available = min(
                    available,
                    max(0, int(limit) - int((parent / "memory.current").read_text()))
                    // 1024,
                )
    disk = shutil.disk_usage(cwd)
    return {
        "memory_used_kib": int(memory["MemTotal"].split()[0])
        - int(memory["MemAvailable"].split()[0]),
        "memory_available_kib": available,
        "memory_total_kib": total,
        "swap_used_kib": int(memory["SwapTotal"].split()[0])
        - int(memory["SwapFree"].split()[0]),
        "disk_used_bytes": disk.used,
        "disk_free_bytes": disk.free,
        "disk_total_bytes": disk.total,
        "cpu_ticks": [
            int(n) for n in Path("/proc/stat").read_text().splitlines()[0].split()[1:]
        ],
        "pressure": {
            name: Path(f"/proc/pressure/{name}").read_text()
            for name in ("cpu", "memory", "io")
            if Path(f"/proc/pressure/{name}").exists()
        },
        "oom_events": (group / "memory.events").read_text()
        if (group / "memory.events").exists()
        else "",
    }


def execute(command, env, cwd, log, timeout):
    adopt_orphans()
    started = time.monotonic()
    samples = []
    with log.open("w") as output:
        process = subprocess.Popen(
            [
                "/usr/bin/time",
                "-v",
                "bash",
                "--noprofile",
                "--norc",
                "-euo",
                "pipefail",
                "-c",
                command,
            ],
            cwd=cwd,
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            while True:
                samples.append(
                    {
                        "seconds": time.monotonic() - started,
                        **host_sample(cwd),
                    }
                )
                remaining = timeout - (time.monotonic() - started)
                if remaining <= 0:
                    raise subprocess.TimeoutExpired(command, timeout)
                try:
                    result = process.wait(timeout=min(5, remaining))
                    break
                except subprocess.TimeoutExpired:
                    if len(samples) % 8 == 0:
                        print(
                            f"Still running after {time.monotonic() - started:.0f}s: {log.name}",
                            flush=True,
                        )
        except subprocess.TimeoutExpired:
            result = 124
        finally:
            terminate_children([process])
    return {
        "exit_code": result,
        "seconds": time.monotonic() - started,
        "resources": samples,
    }


def run(args):
    if args.cli_test_threads != 4 and args.test_retries:
        raise ValueError("CLI concurrency comparisons must not retry tests")
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yaml").read_text())
    chunks = json.loads(
        subprocess.check_output(
            ["bash", "build-components.sh", "list-groups"], cwd=ROOT / "test-components"
        )
    )
    stages = inventory(workflow, chunks)
    if args.check_cli_baseline:
        path = Path(args.check_cli_baseline)
        report = json.loads(path.read_text()) if path.is_file() else {}
        eligible = cli_baseline_eligible(report, stages)
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            print(f"eligible={str(eligible).lower()}", file=output)
        return 0
    context = {
        "github": {
            "workspace": str(ROOT),
            "token": os.getenv("GH_TOKEN", ""),
            "sha": os.environ.get("GITHUB_SHA", ""),
            "run_attempt": os.getenv("GITHUB_RUN_ATTEMPT", "1"),
        },
        "runner": {"temp": os.getenv("RUNNER_TEMP", "/tmp")},
        "env": {**workflow.get("env", {}), **os.environ},
        "steps": {"cache": {"outputs": {"cache-hit": "false"}}},
    }
    # Inventory mode must classify every step before any billable trial.
    for stage in stages:
        stage_context = {**context, "matrix": stage["matrix"]}
        for command in commands(stage, stage_context, args.cli_test_threads):
            for field in ("run", "working-directory", "name"):
                render(command.get(field, ""), stage_context)
            for value in command.get("env", {}).values():
                render(value, stage_context)
    if args.inventory:
        print(
            json.dumps(
                {
                    "stages": [
                        {
                            "id": s["id"],
                            "phase": "prepare" if s["job"] in PREPARE else "checks",
                        }
                        for s in stages
                    ],
                    "excluded": EXCLUDED,
                },
                indent=2,
            )
        )
        return 0

    if os.getenv("GITHUB_ACTIONS") != "true":
        raise ValueError(
            "Execution requires an isolated Actions checkout; use --inventory locally"
        )
    evidence = ROOT / "tmp/experiment-results" / args.cycle
    evidence.mkdir(parents=True, exist_ok=True)
    products = ROOT / "tmp/experiment-products"
    products.mkdir(parents=True, exist_ok=True)
    selected = select_stages(stages, args.phase, args.workers, args.shard)
    if getattr(args, "stage", None):
        wanted = set(args.stage)
        if wanted - {s["id"] for s in selected}:
            raise ValueError(f"Unknown selected stage: {wanted}")
        selected = [s for s in selected if s["id"] in wanted]
    clippy_root = getattr(args, "clippy_workspace", None)
    if clippy_root:
        clippy_root = Path(clippy_root).resolve()
        if (
            ROOT == clippy_root
            or ROOT in clippy_root.parents
            or clippy_root in ROOT.parents
        ):
            raise ValueError("Clippy and suite workspaces must be disjoint")
        source_sha = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=clippy_root, text=True
        ).strip()
        if (
            source_sha
            != subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
            ).strip()
        ):
            raise ValueError("Clippy and suite source SHAs differ")
        clippy = [s for s in selected if s["job"] == "check-clippy-and-format"]
        if clippy:
            selected = [s for s in selected if s not in clippy]
            index = next(
                i
                for i, s in enumerate(selected)
                if s["id"] == "it/integration-tests-group2"
            )
            selected[index:index] = clippy
    report = {
        "source_sha": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "cycle": args.cycle,
        "phase": args.phase,
        "workers": args.workers,
        "shard": args.shard,
        "cli_test_threads": args.cli_test_threads,
        "test_retries": args.test_retries,
        "started_at": time.time(),
        "stages": [],
    }
    failures = set()
    for stage in selected:
        stage_root = (
            clippy_root
            if clippy_root and stage["job"] == "check-clippy-and-format"
            else ROOT
        )
        stage_context = {
            **context,
            "github": {**context["github"], "workspace": str(stage_root)},
            "matrix": stage["matrix"],
        }
        stage_dir = evidence / stage["id"].replace("/", "--")
        stage_dir.mkdir(exist_ok=True)
        result = {
            "id": stage["id"],
            "steps": [],
            "status": "running",
            "started_at": time.time(),
            "workspace": str(stage_root),
        }
        if stage["job"] == "check-clippy-and-format":
            result["cache_before"] = cache_fingerprints(stage_root)
        report["stages"].append(result)
        deadline = (
            time.monotonic() + stage["definition"].get("timeout-minutes", 360) * 60
        )
        report_path = evidence / f"{args.phase}-{args.shard}.json"
        report_path.write_text(json.dumps(report, indent=2))
        needs = dependencies(stage["definition"])
        if set(needs) & failures:
            result["status"] = "blocked"
            failures.add(stage["job"])
        else:
            # Restore tracked source without removing compiler/dependency caches.
            subprocess.run(
                ["git", "restore", "--worktree", "."], cwd=stage_root, check=True
            )
            for path in stage_root.glob("**/target/ctrf*.json"):
                path.unlink()
            if stage["job"] == "build-test-components":
                shutil.rmtree(ROOT / "tmp/test-components-wasm", ignore_errors=True)
            if stage["job"] == "merge-test-components":
                for path in (products / "fixtures").glob("*.wasm"):
                    shutil.copy2(path, ROOT / "tmp/test-components-wasm" / path.name)
            env = {
                **os.environ,
                **{
                    k: render(v, stage_context)
                    for k, v in workflow.get("env", {}).items()
                },
                **{
                    k: render(v, stage_context)
                    for k, v in stage["definition"].get("env", {}).items()
                },
            }
            env["PATH"] = f"{stage_root}/target/debug:{env['PATH']}"
            env["GITHUB_WORKSPACE"] = str(stage_root)
            # GITHUB_ENV/PATH writes are scoped to the logical stage, not the host job.
            for key in ("GITHUB_ENV", "GITHUB_PATH", "GITHUB_OUTPUT"):
                env[key] = str(stage_dir / key)
                Path(env[key]).write_text("")
            for i, step in enumerate(
                commands(stage, stage_context, args.cli_test_threads)
            ):
                step_env = {
                    **env,
                    **{
                        k: render(v, stage_context)
                        for k, v in step.get("env", {}).items()
                    },
                }
                if step["name"] == "Select prebuilt built-ins":
                    env["GOLEM_BUILTIN_ARTIFACTS_PREBUILT"] = "1"
                name = render(step.get("name", f"step-{i}"), stage_context)
                for attempt in range(args.test_retries + 1):
                    suffix = f"-retry{attempt}" if attempt else ""
                    log = stage_dir / f"{i:02}{suffix}.log"
                    print(
                        f"::group::{args.cycle} {stage['id']} — {name}{suffix}",
                        flush=True,
                    )
                    timeout = max(
                        0,
                        min(
                            step.get("timeout-minutes", 90) * 60,
                            deadline - time.monotonic(),
                        ),
                    )
                    observation = execute(
                        render(step["run"], stage_context),
                        step_env,
                        stage_root
                        / render(step.get("working-directory", "."), stage_context),
                        log,
                        timeout,
                    )
                    observation.update(
                        name=name,
                        attempt=attempt,
                        log=log.name,
                        timeout_seconds=timeout,
                    )
                    result["steps"].append(observation)
                    for j, path in enumerate(stage_root.glob("**/target/ctrf*.json")):
                        shutil.move(path, stage_dir / f"ctrf-{i}-{attempt}-{j}.json")
                    report_path.write_text(json.dumps(report, indent=2))
                    print(
                        json.dumps(
                            {k: v for k, v in observation.items() if k != "resources"}
                        ),
                        flush=True,
                    )
                    print("::endgroup::", flush=True)
                    if not observation["exit_code"]:
                        break
                    output = log.read_text(errors="replace")
                    print(output[-20000:], flush=True)
                    # Retry only confirmed test failures. Compiler errors, killed
                    # processes, and timeouts do not qualify as flaky test evidence.
                    if (
                        attempt == args.test_retries
                        # cargo-make wraps a test-r exit of 101 as 105.
                        or observation["exit_code"] not in {1, 101, 105}
                        or not re.search(
                            r"ℹ fail [1-9]\d*\b|test result: FAILED;", output
                        )
                    ):
                        break
                    time.sleep(2**attempt)
                if observation["exit_code"]:
                    result["status"] = "failed"
                    failures.add(stage["job"])
                    break
            if result["status"] == "running":
                result["status"] = (
                    "passed_after_retry"
                    if any(step["attempt"] for step in result["steps"])
                    else "passed"
                )
            with (stage_dir / "source.diff").open("w") as output:
                subprocess.run(
                    ["git", "diff", "--binary"],
                    cwd=stage_root,
                    stdout=output,
                    check=True,
                )
            if result["status"] in {"passed", "passed_after_retry"}:
                if stage["job"] == "build-cli-test-bins":
                    for binary in ("golem", "golem-cli"):
                        shutil.copy2(
                            ROOT / "target/dev-release-ci" / binary, products / binary
                        )
                if stage["job"] == "build-test-components":
                    fixtures = products / "fixtures"
                    fixtures.mkdir(exist_ok=True)
                    for path in (ROOT / "tmp/test-components-wasm").glob("*.wasm"):
                        shutil.copy2(path, fixtures / path.name)
                if stage["job"] == "merge-test-components":
                    for path in (products / "fixtures").glob("*.wasm"):
                        shutil.copy2(path, ROOT / "test-components" / path.name)
        result["finished_at"] = time.time()
        if stage["job"] == "check-clippy-and-format":
            result["cache_after"] = cache_fingerprints(stage_root)
        report["updated_at"] = time.time()
        (evidence / f"{args.phase}-{args.shard}.json").write_text(
            json.dumps(report, indent=2)
        )
    report["finished_at"] = time.time()
    report["success"] = not failures
    report["fully_green"] = not failures and all(
        stage["status"] == "passed" for stage in report["stages"]
    )
    (evidence / f"{args.phase}-{args.shard}.json").write_text(
        json.dumps(report, indent=2)
    )
    return int(bool(failures))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", action="store_true")
    parser.add_argument("--cycle", default="cold")
    parser.add_argument("--check-cli-baseline", metavar="REPORT")
    parser.add_argument(
        "--phase",
        choices=("all", "prepare", "checks", "cli-prime", "cli"),
        default="all",
    )
    parser.add_argument("--cli-test-threads", type=int, choices=(4, 6, 8), default=4)
    parser.add_argument("--test-retries", type=int, choices=(0, 1, 2), default=0)
    parser.add_argument("--workers", type=int, choices=(1, 2, 3), default=1)
    parser.add_argument("--shard", type=int, default=0)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--clippy-workspace", type=Path)
    parser.add_argument("--stage", action="append")
    args = parser.parse_args()
    if args.workspace:
        ROOT = args.workspace.resolve()
    # Cancellation must unwind execute's owned-process cleanup.
    signal.signal(signal.SIGTERM, interrupt)
    raise SystemExit(run(args))
