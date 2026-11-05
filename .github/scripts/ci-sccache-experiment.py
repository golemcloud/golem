"""Measure parent-to-commit native rebuilds with independently primed Cargo outputs."""

import argparse
import importlib.util
import json
import os
import shlex
import shutil
import subprocess
import time
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
CASES = [
    {
        "name": "shard-manager",
        "parent": "074ebb0467a886117edec9f6e15ca29b32207b6c",
        "commit": "668b77ed789ed6b904b02b2df2e4d848d4e67897",
        "arms": ["cargo", "sccache"],
    },
    {
        "name": "shared-permissions",
        "parent": "cae862bdadf465846af2001223b11edcc06713ad",
        "commit": "143653309740e287e03e80b6a4abba53750e2ea7",
        "arms": ["sccache", "cargo"],
    },
]


def plan():
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yaml").read_text())
    publication = next(
        s
        for s in workflow["jobs"]["build-service-tests"]["steps"]
        if s.get("uses") == "./.github/actions/publish-native-binaries"
    )
    return {
        "cases": CASES,
        "selection": shlex.split(publication["with"]["cargo-args"]),
        "profile": "dev-ci",
        "rust": "1.99.1",
        "cargo_jobs": 20,
        "incremental": "false",
        "sccache": "0.19.0",
        "cache_capacity": "30G",
        "build_timeout_seconds": 2700,
        "wit_timeout_seconds": 900,
    }


def run():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Run on the isolated experiment worker; use --plan locally")
    spec = importlib.util.spec_from_file_location(
        "experiment", Path(__file__).with_name("ci-experiment.py")
    )
    experiment = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(experiment)
    configuration = plan()
    results = ROOT / "tmp/sccache-results"
    results.mkdir(parents=True, exist_ok=True)
    source = ROOT / "tmp/sccache-source"
    cache = ROOT / "tmp/sccache-cache"
    if source.exists() or cache.exists():
        raise RuntimeError("Refusing to reuse an existing experiment source or cache")
    report = {"configuration": configuration, "arms": []}

    def save():
        (results / "results.json").write_text(json.dumps(report, indent=2) + "\n")

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True)

    for case in CASES:
        if git("rev-parse", f"{case['commit']}^").strip() != case["parent"]:
            raise ValueError(f"Not a parent-to-commit transition: {case}")
        # Do not confound the source-change comparison with dependency/config changes.
        git(
            "diff",
            "--exit-code",
            case["parent"],
            case["commit"],
            "--",
            "Cargo.toml",
            "Cargo.lock",
            ".cargo",
        )
    save()
    git("worktree", "add", "--detach", str(source), CASES[0]["parent"])
    try:
        for case in CASES:
            for arm in case["arms"]:
                directory = results / case["name"] / arm
                directory.mkdir(parents=True)
                entry = {"case": case["name"], "arm": arm, "phases": []}
                report["arms"].append(entry)
                # Only the disposable worktree is cleaned. Registry downloads can be
                # shared; compiler outputs and sccache entries cannot cross arms.
                git("-C", str(source), "reset", "--hard", case["parent"])
                git("-C", str(source), "clean", "-ffdx")
                shutil.rmtree(cache, ignore_errors=True)
                env = {
                    k: v
                    for k, v in os.environ.items()
                    if not k.startswith("SCCACHE_")
                    and k
                    not in {
                        "RUSTC_WRAPPER",
                        "RUSTC_WORKSPACE_WRAPPER",
                        "CARGO_TARGET_DIR",
                    }
                }
                env.update(
                    RUSTUP_TOOLCHAIN=configuration["rust"],
                    CARGO_BUILD_JOBS=str(configuration["cargo_jobs"]),
                    CARGO_INCREMENTAL=configuration["incremental"],
                    CI="true",
                    QUIET="true",
                    GITHUB_WORKSPACE=str(source),
                )
                if arm == "sccache":
                    config = directory / "sccache.toml"
                    config.write_text(
                        f'[cache.disk]\ndir = "{cache}"\nsize = 32212254720\n'
                    )
                    env.update(
                        RUSTC_WRAPPER=shutil.which("sccache"),
                        SCCACHE_CONF=str(config),
                        SCCACHE_CACHED_CONF=str(directory / "cached-config"),
                        SCCACHE_DIR=str(cache),
                        SCCACHE_CACHE_SIZE=configuration["cache_capacity"],
                        SCCACHE_IDLE_TIMEOUT="0",
                    )

                def sccache(*args, env=env):
                    return subprocess.check_output(
                        ["sccache", *args], cwd=source, env=env, text=True, timeout=30
                    )

                # Start outside execute's build process group so counters survive
                # build cleanup and cover the complete WIT + native build phase.
                try:
                    if arm == "sccache":
                        (directory / "server-start.txt").write_text(
                            sccache("--start-server")
                        )
                    for phase, sha in [
                        ("prime", case["parent"]),
                        ("changed", case["commit"]),
                    ]:
                        git("-C", str(source), "reset", "--hard", sha)
                        env["GITHUB_SHA"] = sha
                        phase_dir = directory / phase
                        phase_dir.mkdir()
                        record = {"phase": phase, "sha": sha, "steps": []}
                        entry["phases"].append(record)
                        timing = source / "target/cargo-timings/cargo-timing.html"
                        timing.unlink(missing_ok=True)
                        if arm == "sccache":
                            sccache("--zero-stats")
                        started = time.monotonic()
                        commands = [
                            (
                                "wit",
                                "cargo make --profile ci wit",
                                configuration["wit_timeout_seconds"],
                            ),
                            (
                                "native",
                                shlex.join(
                                    [
                                        "cargo",
                                        "build",
                                        "--locked",
                                        "--profile",
                                        configuration["profile"],
                                        *configuration["selection"],
                                        "--timings",
                                        "--message-format=json-render-diagnostics",
                                    ]
                                ),
                                configuration["build_timeout_seconds"],
                            ),
                        ]
                        for name, command, timeout in commands:
                            print(
                                f"{case['name']} / {arm} / {phase} / {name}", flush=True
                            )
                            step = experiment.execute(
                                command, env, source, phase_dir / f"{name}.log", timeout
                            )
                            record["steps"].append(
                                {"name": name, "command": command, **step}
                            )
                            save()
                            if step["exit_code"]:
                                break
                        record["seconds"] = time.monotonic() - started
                        record["passed"] = all(
                            s["exit_code"] == 0 for s in record["steps"]
                        )
                        if arm == "sccache":
                            stats = sccache("--show-stats", "--stats-format", "json")
                            record["sccache"] = json.loads(stats)
                            (phase_dir / "sccache.json").write_text(stats)
                            (phase_dir / "cache-size.txt").write_text(
                                subprocess.check_output(
                                    ["du", "-sb", str(cache)], text=True, timeout=30
                                )
                            )
                        if timing.exists():
                            shutil.copy2(timing, phase_dir / "cargo-timing.html")
                        (phase_dir / "source.diff").write_text(
                            git("-C", str(source), "diff")
                        )
                        save()
                        if not record["passed"]:
                            break
                finally:
                    if arm == "sccache":
                        (directory / "server-stop.txt").write_text(
                            sccache("--stop-server")
                        )
                entry["passed"] = len(entry["phases"]) == 2 and all(
                    p["passed"] for p in entry["phases"]
                )
                save()
    finally:
        git("worktree", "remove", "--force", str(source))
        save()
    return 0 if all(a["passed"] for a in report["arms"]) else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--plan", action="store_true", help="Print the non-mutating experiment plan"
    )
    args = parser.parse_args()
    if args.plan:
        print(json.dumps(plan(), indent=2))
    else:
        raise SystemExit(run())
