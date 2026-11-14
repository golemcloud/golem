"""Bound run-specific publications without changing their immutable disk keys.

At more than 350 disks, remove closed-PR publications first, then the least
recently used completed PR publications, until 250 remain. Main, active runs,
unknown ownership and disks used within the last hour are protected. Failed-job
reruns can lose old snapshots under pressure and must rerun their producers.
Requires the official Blacksmith CLI and gh; defaults to a read-only dry run.
"""

import argparse
import json
import os
import re
import subprocess
from collections import Counter
from datetime import UTC, datetime, timedelta
from urllib.parse import urlencode

HIGH_WATER = 350
LOW_WATER = 250
GRACE = timedelta(hours=1)
PRODUCERS = (
    "native-binaries|executor-test-binaries|service-test-binaries|"
    "builtin-artifacts|cli-binary|scala-ivy|test-components"
)


def timestamp(value):
    return datetime.fromisoformat(value)


def run_id(repo, entry):
    match = re.fullmatch(
        rf"{re.escape(repo)}-(?:{PRODUCERS})-(\d+)-producer-attempt\d+",
        entry["key"],
    )
    return int(match[1]) if match and entry["type"] == "stickydisk" else None


def identity(entry):
    # CLI deletion scopes by key and architecture, not by region or disk type.
    return entry["key"], entry["arch"]


def plan_cleanup(repo, entries, runs, prs, now):
    if len(entries) <= HIGH_WATER:
        return []
    scopes = Counter(identity(e) for e in entries)
    candidates = []
    for entry in entries:
        run = runs.get(run_id(repo, entry))
        if (
            not run
            or run["head_branch"] == "main"
            or run["status"] != "completed"
            or not run["pull_requests"]
            or scopes[identity(entry)] != 1
            or now - timestamp(entry["last_used_at"]) < GRACE
        ):
            continue
        numbers = [pr["number"] for pr in run["pull_requests"]]
        if any(not prs.get(number) for number in numbers):
            continue
        closed = all(prs[number]["state"] == "closed" for number in numbers)
        candidates.append(
            (not closed, timestamp(entry["last_used_at"]), entry["key"], entry)
        )
    candidates.sort(key=lambda item: item[:3])
    return [item[3] for item in candidates[: len(entries) - LOW_WATER]]


def command(args):
    return subprocess.check_output(args, text=True, timeout=60)


def github(path):
    result = subprocess.run(
        ["gh", "api", path],
        check=False,
        text=True,
        capture_output=True,
        timeout=60,
    )
    if result.returncode:
        if "(HTTP 404)" in result.stderr:
            print(
                f"WARNING: cannot establish ownership from {path}; protecting its disks",
                flush=True,
            )
            return None
        raise RuntimeError(f"GitHub lookup failed for {path}: {result.stderr.strip()}")
    return json.loads(result.stdout)


def blacksmith(repo, *args):
    return command(
        ["blacksmith", "--org", repo.split("/")[0], "stickydisk", *args, "--repo", repo]
    )


def inventory(repo):
    entries = []
    for page in range(1, 101):
        data = json.loads(
            blacksmith(
                repo,
                "list",
                "--per-page",
                "100",
                "--page",
                str(page),
                "--sort-by",
                "key",
                "--sort-direction",
                "asc",
                "--format",
                "json",
            )
        )
        if (
            data["repo"] != repo
            or data["installation"]["installation_name"] != repo.split("/")[0]
        ):
            raise RuntimeError(
                "Blacksmith returned a different repository or organization"
            )
        entries.extend(data["entries"])
        if not data["pagination"]["has_more_pages"]:
            full_ids = {(e["key"], e["arch"], e["region"], e["type"]) for e in entries}
            if len(entries) != data["pagination"]["total"] or len(full_ids) != len(
                entries
            ):
                raise RuntimeError(
                    "Disk inventory changed during pagination; try again"
                )
            return entries
    raise RuntimeError("Disk inventory exceeds 100 pages")


def metadata(repo, entries):
    runs = {}
    prs = {}
    branches = {}
    for rid in sorted({run_id(repo, e) for e in entries} - {None}):
        run = github(f"repos/{repo}/actions/runs/{rid}")
        runs[rid] = run
        if not run or run["head_branch"] == "main" or run["status"] != "completed":
            continue
        # Older runs can omit pull_requests. Resolve the exact source repository
        # and branch, not a branch with the same name in another fork.
        if (
            not run["pull_requests"]
            and run["event"] == "pull_request"
            and run["head_repository"]
        ):
            head = run["head_repository"]
            branch = (head["id"], run["head_branch"])
            if branch not in branches:
                query = urlencode(
                    {
                        "state": "all",
                        "head": f"{head['owner']['login']}:{run['head_branch']}",
                        "per_page": 100,
                    }
                )
                matches = github(f"repos/{repo}/pulls?{query}") or []
                if len(matches) == 100:
                    raise RuntimeError(f"PR lookup for run {rid} exceeds one page")
                branches[branch] = [
                    p
                    for p in matches
                    if p["head"]["repo"]
                    and p["head"]["repo"]["id"] == head["id"]
                    and p["head"]["ref"] == run["head_branch"]
                ]
                prs.update({p["number"]: p for p in branches[branch]})
            run["pull_requests"] = [{"number": p["number"]} for p in branches[branch]]
        for pr in run["pull_requests"]:
            number = pr["number"]
            if number not in prs:
                prs[number] = github(f"repos/{repo}/pulls/{number}")
    return runs, prs


def cleanup(repo, apply=False):
    entries = inventory(repo)
    print(
        f"Inventory: {len(entries)} disks; trigger >{HIGH_WATER}; target {LOW_WATER}",
        flush=True,
    )
    if len(entries) <= HIGH_WATER:
        return
    runs, prs = metadata(repo, entries)
    selected = plan_cleanup(repo, entries, runs, prs, datetime.now(UTC))
    print(
        f"Selected {len(selected)} disks; protected/ineligible {len(entries) - len(selected)}",
        flush=True,
    )
    for entry in selected:
        print(
            f"{'DELETE' if apply else 'DRY RUN'}: {entry['key']} ({entry['arch']}); last used {entry['last_used_at']}",
            flush=True,
        )
    if not apply:
        if len(entries) - len(selected) > LOW_WATER:
            print("WARNING: protected disks prevent reaching the target")
        return

    # Refresh inventory after metadata lookups. Never delete a changed, recently
    # used, or ambiguous scope; the next workflow run can reconsider it.
    current_entries = inventory(repo)
    current_scopes = Counter(identity(e) for e in current_entries)
    current = {identity(e): e for e in current_entries}
    deleted = []
    for entry in selected:
        fresh = current.get(identity(entry))
        if current_scopes[identity(entry)] != 1 or fresh != entry:
            print(f"SKIP changed disk: {entry['key']}", flush=True)
            continue
        rid = run_id(repo, entry)
        run = github(f"repos/{repo}/actions/runs/{rid}")
        if (
            not run
            or run["head_branch"] == "main"
            or run["status"] != "completed"
            or run["run_attempt"] != runs[rid]["run_attempt"]
        ):
            print(f"SKIP protected or rerun run: {rid}", flush=True)
            continue
        # Do not retry a failed deletion: its outcome may be unknown. Let the
        # next invocation obtain a fresh inventory instead.
        print(
            blacksmith(
                repo, "delete", "--key", entry["key"], "--arch", entry["arch"], "--yes"
            ),
            flush=True,
        )
        deleted.append(identity(entry))
    after = inventory(repo)
    remaining = {identity(e) for e in after}
    if remaining.intersection(deleted):
        raise RuntimeError("Deleted disks still appear in the final inventory")
    print(f"Verified {len(deleted)} deletions; {len(after)} disks remain", flush=True)
    if len(after) > HIGH_WATER:
        raise RuntimeError(
            "Inventory remains above the high-water mark; protected disks or concurrent growth require attention"
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument(
        "--apply", action="store_true", help="Delete selected disks (default: dry run)"
    )
    args = parser.parse_args()
    os.environ["BLACKSMITH_DISABLE_AUTO_UPDATE"] = "1"
    cleanup(args.repo, args.apply)
