#!/usr/bin/env python3
"""Build the representative component-size suite and assemble one local report."""

import argparse
import json
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ANALYSIS = ROOT / "test-components" / "size-analysis"


def run(*command, stdout=None):
    print("+", " ".join(map(str, command)), flush=True)
    if stdout:
        with open(stdout, "wb") as output:
            subprocess.run(
                list(map(str, command)), cwd=ROOT, stdout=output, check=True
            )
    else:
        subprocess.run(
            list(map(str, command)),
            cwd=ROOT,
            check=True,
        )


def load(path):
    return json.loads(path.read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=ROOT / "tmp" / "component-size-report",
        help="new, non-existing output directory",
    )
    args = parser.parse_args()
    output = args.out.resolve()
    if output.exists():
        parser.error(f"output directory already exists: {output}")
    output.mkdir(parents=True)

    required = ("cargo", "node", "rustc", "wasm-opt", "wasm-tools")
    missing = [tool for tool in required if not shutil.which(tool)]
    if missing:
        parser.error("missing required tools: " + ", ".join(missing))

    run("python3", "-m", "unittest", "discover", "-s", ANALYSIS, "-v")

    fixture_root = output / "retention"
    fixture_manifest = ANALYSIS / "fixtures" / "Cargo.toml"
    fixture_packages = {
        "canonical-floor": "retention-canonical-floor",
        "empty": "retention-empty",
        "tool": "retention-non-reflective",
        "reflection": "retention-reflection",
        "tool-client": "retention-tool-client",
    }
    for label, package in fixture_packages.items():
        run(
            "python3",
            ANALYSIS / "analyze.py",
            "--manifest",
            fixture_manifest,
            "--package",
            package,
            "--out",
            fixture_root / label,
        )

    floor_world = fixture_root / "canonical-floor" / "world.json"
    empty_world = fixture_root / "empty" / "world.json"
    run(
        "wasm-tools",
        "component",
        "wit",
        "--json",
        fixture_root / "canonical-floor" / "current" / "component.wasm",
        stdout=floor_world,
    )
    run(
        "wasm-tools",
        "component",
        "wit",
        "--json",
        fixture_root / "empty" / "current" / "component.wasm",
        stdout=empty_world,
    )
    run(
        "python3",
        ANALYSIS / "compare-export-contract.py",
        floor_world,
        empty_world,
    )

    retention = output / "reflection-retention.json"
    run(
        "python3",
        ANALYSIS / "retention.py",
        "--negative",
        f"empty={fixture_root / 'empty' / 'current' / 'component.wasm'}",
        "--negative",
        f"tool={fixture_root / 'tool' / 'current' / 'component.wasm'}",
        "--positive",
        f"reflection={fixture_root / 'reflection' / 'current' / 'component.wasm'}",
        "--enforce-option-b",
        "--out",
        retention,
    )

    matrix = output / "release-profile-matrix"
    run(
        "python3",
        ANALYSIS / "analyze.py",
        "--manifest",
        ROOT / "test-components" / "agent-counters" / "Cargo.toml",
        "--matrix",
        "--optimize",
        "--out",
        matrix,
    )
    allocator = output / "allocator-performance.json"
    run(
        "node",
        ANALYSIS / "benchmark-core.mjs",
        matrix / "baseline-s" / "report" / "core-0" / "module.wasm",
        matrix / "strip" / "report" / "core-0" / "module.wasm",
        matrix / "cgu1" / "report" / "core-0" / "module.wasm",
        matrix / "z" / "report" / "core-0" / "module.wasm",
        matrix / "strip" / "optimized-report" / "core-0" / "module.wasm",
        stdout=allocator,
    )

    details = {
        "versions": load(matrix / "versions.json"),
        "fixtures": {
            label: load(directory / "summary.json")
            for label, directory in (
                (label, fixture_root / label) for label in fixture_packages
            )
        },
        "release_profile_matrix": load(matrix / "summary.json"),
        "reflection_retention": load(retention),
        "allocator_performance": load(allocator),
    }
    (output / "report.json").write_text(json.dumps(details, indent=2) + "\n")

    rows = [
        "# Component size report",
        "",
        "Comparisons are meaningful only with the same toolchain, lockfile, Cargo configuration, and environment.",
        "",
        "## Retention fixtures",
        "",
        "| Fixture | Component bytes | Code bytes | Data bytes | Custom bytes |",
        "|---|---:|---:|---:|---:|",
    ]
    for label, summaries in details["fixtures"].items():
        summary = summaries["current"]
        rows.append(
            f"| {label} | {summary['bytes']:,} | {summary['code']:,} | "
            f"{summary['data']:,} | {summary['custom']:,} |"
        )
    rows += [
        "",
        "## Release profile matrix",
        "",
        "| Variant | Component bytes | Code bytes | Data bytes | Custom bytes |",
        "|---|---:|---:|---:|---:|",
    ]
    for label, summary in details["release_profile_matrix"].items():
        rows.append(
            f"| {label} | {summary['bytes']:,} | {summary['code']:,} | "
            f"{summary['data']:,} | {summary['custom']:,} |"
        )
    rows += [
        "",
        "Reflection/model retention contracts passed.",
        "See `report.json` and the per-component directories for machine-readable details, symbols, sections, WIT, Cargo settings, and artifacts.",
        "",
    ]
    (output / "report.md").write_text("\n".join(rows))
    print(f"\nReport written to {output / 'report.md'}")


if __name__ == "__main__":
    main()
