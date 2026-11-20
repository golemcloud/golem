#!/usr/bin/env python3
"""Reports what a merge or rebase can have broken and which builds cover it.

The script compares three commits:

- ours:   the branch tip before the merge (default: HEAD^1)
- theirs: the incoming tip that was merged (default: HEAD^2)
- result: the merged tree (default: HEAD; use --worktree for uncommitted resolutions)

It prints:

1. The files that both sides changed since their merge base. Conflict
   resolutions live here.
2. Possible reverts in those files: lines one side removed that are back in the
   result, and lines one side added that are missing from the result.
3. The affected projects and the commands that build or check each of them.

For a rebase, pass --ours ORIG_HEAD --theirs <upstream> and the rebased tip as
--result (default HEAD).
"""

import argparse
import collections
import json
import re
import subprocess
import sys
from pathlib import Path


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True
    ).stdout


def changed_files(a: str, b: str | None) -> set[str]:
    args = ["diff", "--name-only", a] + ([b] if b else [])
    return {line for line in git(*args).splitlines() if line}


def line_changes(a: str, b: str | None, path: str) -> tuple[set[str], set[str]]:
    """Returns the significant (added, removed) lines of `path` between a and b."""
    args = ["diff", "-U0", a] + ([b] if b else []) + ["--", path]
    added, removed = set(), set()
    for line in git(*args).splitlines():
        if line.startswith(("+++", "---")):
            continue
        if line[:1] in "+-":
            text = line[1:].strip()
            if len(text) > 8 and re.search(r"[A-Za-z]", text):
                (added if line[0] == "+" else removed).add(text)
    return added, removed


def possible_reverts(base, ours, theirs, result, path):
    """Lines a side changed that the result undoes, per side."""
    findings = {}
    res_added, res_removed = line_changes(base, result, path)
    for side, ref in (("theirs", theirs), ("ours", ours)):
        side_added, side_removed = line_changes(base, ref, path)
        # A line the side removed is back if the result still differs from the
        # base without removing it, i.e. the result kept or re-added it.
        back = {l for l in side_removed if l not in res_removed}
        dropped = {l for l in side_added if l not in res_added}
        if back or dropped:
            findings[side] = (back, dropped)
    return findings


def root_workspace_members(repo: Path) -> dict[str, str]:
    """Maps member directory (relative) to crate name."""
    try:
        meta = json.loads(
            subprocess.run(
                ["cargo", "metadata", "--no-deps", "--format-version", "1"],
                cwd=repo, check=True, capture_output=True, text=True,
            ).stdout
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return {}
    members = {}
    for package in meta["packages"]:
        directory = Path(package["manifest_path"]).parent.relative_to(repo)
        members[str(directory)] = package["name"]
    return members


def component_groups(repo: Path) -> dict[str, str]:
    """Maps a test-component app directory to its build-components.sh group."""
    script = (repo / "test-components/build-components.sh").read_text()
    groups = {}
    for group, apps in re.findall(r"^(\w+?)_(?:test_)?apps=\(([^)]*)\)", script, re.M):
        group = "benchmarks" if group == "benchmark" else group
        for app in re.findall(r'"([^"]+)"', apps):
            groups[app] = group
    return groups


MIDDLEWARE_UNIT_TEST_CRATES = {"audit-middleware", "output-redaction", "rate-limit-middleware"}

SDK_CHECKS = {
    "rust": "cargo make validate-sdk-rust-check",
    "ts": "cargo make validate-sdk-ts",
    "effect": "cargo make validate-sdk-effect",
    "scala": "cargo make validate-sdk-scala",
    "moonbit": "cargo make validate-sdk-moonbit",
    "go": "cargo make validate-sdk-go",
}


def classify(path, members, groups, checks, notes, sdks_changed):
    parts = path.split("/")
    top = parts[0]

    if top == "sdks" and len(parts) > 1:
        sdk = parts[1]
        if sdk in SDK_CHECKS:
            if sdk == "scala" and len(parts) > 3 and parts[2] == "test-agents" and parts[3] == "samples":
                notes["Scala REPL samples run only in the Scala integration suite; "
                      "load golem-scala-integration-tests"].add(path)
                return
            if sdk == "go" and "internal/wit/" in path:
                checks["cargo make generate-sdk-go-bindings  # regenerate, then diff"].add(path)
            checks[SDK_CHECKS[sdk]].add(path)
            sdks_changed.add(sdk)
            return

    if top == "test-components" and len(parts) > 1:
        app = parts[1]
        if app in MIDDLEWARE_UNIT_TEST_CRATES:
            checks["cargo make test-component-middleware-unit-tests"].add(path)
        if app in groups:
            checks[f"./test-components/build-components.sh {groups[app]}"].add(path)
        elif app not in MIDDLEWARE_UNIT_TEST_CRATES and len(parts) > 2:
            notes[f"test-components/{app} is in no build-components.sh group; "
                  "find how it is built before skipping it"].add(path)
        return

    if top == "wit":
        checks["cargo make wit && cargo make check-wit"].add(path)
        return
    if top == "openapi":
        checks["cargo make check-openapi"].add(path)
        return
    if path.startswith("golem-skills/skills/"):
        checks["cargo make check-docs-skills"].add(path)
        return
    if top == "docs":
        notes["docs/ changed; follow docs/AGENTS.md for its build"].add(path)
        return
    if path.startswith("cli/golem-cli/tests/") and not path.endswith(".rs"):
        notes["CLI integration test fixture; run the test that uses it with a "
              "targeted cargo-test-r filter (testing skill)"].add(path)

    # Root workspace crate: the deepest member directory that contains the path.
    best = None
    for directory in members:
        if directory == "." or path == directory or path.startswith(directory + "/"):
            if best is None or len(directory) > len(best):
                best = directory
    if best is not None and best != "." and path.endswith((".rs", ".toml", ".proto", ".wit", ".sql")):
        checks["cargo make check-clippy  # whole root workspace, all targets"].add(path)
        return
    if path in ("Cargo.toml", "Cargo.lock"):
        checks["cargo make check-clippy  # whole root workspace, all targets"].add(path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ours", default="HEAD^1")
    parser.add_argument("--theirs", default="HEAD^2")
    parser.add_argument("--result", default="HEAD")
    parser.add_argument("--worktree", action="store_true",
                        help="use the working tree as the result (uncommitted resolution)")
    parser.add_argument("--samples", type=int, default=3, help="sample lines per finding")
    args = parser.parse_args()

    repo = Path(git("rev-parse", "--show-toplevel").strip())
    result = None if args.worktree else args.result
    result_ref = "HEAD" if args.worktree else args.result
    base = git("merge-base", args.ours, args.theirs).strip()

    both = changed_files(base, args.ours) & changed_files(base, args.theirs)
    affected = changed_files(args.ours, result) | both

    print(f"merge base: {base[:12]}  ours: {args.ours}  theirs: {args.theirs}  "
          f"result: {'worktree' if args.worktree else args.result}\n")

    print(f"## Files changed by both sides: {len(both)}\n")
    strong, weak, deleted = [], [], []
    for path in sorted(both):
        exists = args.worktree or subprocess.run(
            ["git", "cat-file", "-e", f"{result_ref}:{path}"], capture_output=True
        ).returncode == 0
        if not exists:
            deleted.append(path)
            continue
        findings = possible_reverts(base, args.ours, args.theirs, result, path)
        if any(back for back, _ in findings.values()):
            strong.append((path, findings))
        elif findings:
            weak.append((path, findings))

    print(f"### Removed lines that are back ({len(strong)} files)\n")
    print("One side deleted these lines and the result still has them. This is the usual "
          "shape of a resolution that kept the wrong side. Inspect every file.\n")
    for path, findings in strong:
        print(f"- {path}")
        for side, (back, dropped) in findings.items():
            if back:
                print(f"    {side} removed {len(back)} line(s) that the result still has:")
                for line in sorted(back)[: args.samples]:
                    print(f"      + {line[:150]}")
            if dropped:
                print(f"    {side} added {len(dropped)} line(s) that the result lacks:")
                for line in sorted(dropped)[: args.samples]:
                    print(f"      - {line[:150]}")

    print(f"\n### Added lines that are missing ({len(weak)} files)\n")
    print("One side added these lines and the result lacks them. A deliberate rewrite also "
          "looks like this; check the files whose change the result should keep.\n")
    for path, findings in weak:
        counts = ", ".join(f"{side} {len(dropped)}" for side, (_, dropped) in findings.items())
        print(f"- {path} ({counts})")
    if deleted:
        print(f"\n### Deleted in the result ({len(deleted)} files)\n")
        for path in deleted:
            print(f"- {path}")
    print()

    members = root_workspace_members(repo)
    groups = component_groups(repo)
    checks = collections.defaultdict(set)
    notes = collections.defaultdict(set)
    sdks_changed = set()
    for path in sorted(affected):
        classify(path, members, groups, checks, notes, sdks_changed)

    # An SDK change can break every test component written against that SDK.
    sdk_component_groups = {"rust": "rust", "ts": "ts", "effect": "effect",
                            "scala": "scala", "moonbit": "moonbit", "go": "go"}
    for sdk in sorted(sdks_changed):
        group = sdk_component_groups[sdk]
        checks[f"./test-components/build-components.sh {group}"].add(f"(sdks/{sdk} changed)")
        if sdk == "rust":
            checks["cargo make test-component-middleware-unit-tests"].add("(sdks/rust changed)")

    print(f"## Affected files: {len(affected)}\n")
    print("## Checks to run\n")
    for command in sorted(checks):
        sources = sorted(checks[command])
        print(f"- {command}")
        for source in sources[:3]:
            print(f"    {source}")
        if len(sources) > 3:
            print(f"    ... and {len(sources) - 3} more")
    if notes:
        print("\n## Needs a manual decision\n")
        for note in sorted(notes):
            sources = sorted(notes[note])
            print(f"- {note}")
            for source in sources[:3]:
                print(f"    {source}")
            if len(sources) > 3:
                print(f"    ... and {len(sources) - 3} more")
    return 0


if __name__ == "__main__":
    sys.exit(main())
