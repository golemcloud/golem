#!/usr/bin/env python3

import argparse
import base64
import concurrent.futures
import datetime as dt
import html
import io
import json
import math
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path


ISSUE_TITLE = "Known flaky tests"
ISSUE_MARKER = "<!-- known-flaky-tests:v1 -->"
REPORT_NAME = re.compile(r"-report(?:-attempt(?P<attempt>[1-9][0-9]*))?$")


def utc_now():
    return dt.datetime.now(dt.timezone.utc)


def parse_time(value):
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def format_time(value):
    return value.astimezone(dt.timezone.utc).strftime("%Y-%m-%d")


def percentile(values, percentile_value):
    if not values:
        return 0
    ordered = sorted(values)
    rank = max(1, math.ceil(percentile_value * len(ordered)))
    return ordered[rank - 1]


def markdown_code(value):
    return "<code>{}</code>".format(
        html.escape(str(value)).replace("|", "&#124;").replace("\n", " ")
    )


def report_job_name(artifact_name):
    base = re.sub(r"-attempt[1-9][0-9]*$", "", artifact_name)
    base = re.sub(r"-report$", "", base)
    if base == "unit-tests":
        return "unit-tests-and-checks"
    if base.startswith("worker-executor-tests-"):
        return base.replace("worker-executor-tests-", "worker-tests-", 1)
    if base.startswith("integration-tests-group"):
        return base
    if base.startswith("cli-integration-tests-"):
        shard = base.removeprefix("cli-integration-tests-")
        if shard == "bridge":
            shard = "bridge_gen"
        return f"it-cli ({shard})"
    return base


def select_job_url(jobs, artifact_name, fallback):
    expected = report_job_name(artifact_name)
    for job in jobs:
        name = job.get("name", "")
        integration_match = expected.startswith("integration-tests-group") and name.startswith(
            f"it ({expected},"
        )
        if name == expected or integration_match:
            return job.get("html_url", fallback)
    return fallback


class GitHubApi:
    def __init__(self, repo, token):
        self.repo = repo
        self.token = token
        self.base_url = "https://api.github.com"

    def request(self, path, method="GET", data=None, absolute=False):
        url = path if absolute else f"{self.base_url}{path}"
        body = None if data is None else json.dumps(data).encode()
        request = urllib.request.Request(url, data=body, method=method)
        request.add_header("Accept", "application/vnd.github+json")
        request.add_unredirected_header("Authorization", f"Bearer {self.token}")
        request.add_header("X-GitHub-Api-Version", "2022-11-28")
        if body is not None:
            request.add_header("Content-Type", "application/json")

        for attempt in range(4):
            try:
                with urllib.request.urlopen(request, timeout=90) as response:
                    return response.read()
            except urllib.error.HTTPError as error:
                if error.code not in (429, 500, 502, 503, 504) or attempt == 3:
                    raise
                delay = int(error.headers.get("Retry-After", 2**attempt))
            except urllib.error.URLError:
                if attempt == 3:
                    raise
                delay = 2**attempt
            time.sleep(delay)
        raise RuntimeError("unreachable")

    def json(self, path, method="GET", data=None):
        return json.loads(self.request(path, method, data))

    def paginated(self, path, key):
        separator = "&" if "?" in path else "?"
        page = 1
        while True:
            response = self.json(f"{path}{separator}per_page=100&page={page}")
            values = response[key]
            yield from values
            if len(values) < 100:
                return
            page += 1


@dataclass
class Observation:
    name: str
    status: str
    duration: float
    retries: int
    run_id: int
    attempt: int
    branch: str
    seen_at: dt.datetime
    artifact_name: str
    run_url: str
    job_url: str = ""


@dataclass
class TestStats:
    name: str
    observations: int = 0
    runs: set = field(default_factory=set)
    failure_runs: set = field(default_factory=set)
    failures: int = 0
    retries: int = 0
    retried_runs: set = field(default_factory=set)
    main_failures: int = 0
    main_failure_runs: set = field(default_factory=set)
    durations: list = field(default_factory=list)
    last_seen: dt.datetime | None = None
    links: list = field(default_factory=list)
    flips: int = 0

    @property
    def fail_rate(self):
        return len(self.failure_runs) / len(self.runs) if self.runs else 0

    @property
    def score(self):
        return self.flips * 5 + len(self.retried_runs) * 3 + len(self.main_failure_runs) * 2


def aggregate(observations):
    stats = {}
    statuses = defaultdict(lambda: defaultdict(lambda: defaultdict(set)))

    for observation in sorted(observations, key=lambda item: item.seen_at):
        test = stats.setdefault(observation.name, TestStats(observation.name))
        test.observations += 1
        test.runs.add(observation.run_id)
        test.durations.append(observation.duration)
        test.last_seen = max(test.last_seen or observation.seen_at, observation.seen_at)
        statuses[observation.name][observation.run_id][observation.attempt].add(observation.status)

        if observation.retries > 0:
            test.retries += observation.retries
            test.retried_runs.add(observation.run_id)
        if observation.status == "failed":
            test.failures += 1
            test.failure_runs.add(observation.run_id)
            if observation.branch == "main":
                test.main_failures += 1
                test.main_failure_runs.add(observation.run_id)
            link = observation.job_url or observation.run_url
            if link and link not in test.links:
                test.links.append(link)

    for name, runs in statuses.items():
        for attempts in runs.values():
            failed_attempts = [number for number, values in attempts.items() if "failed" in values]
            passed_attempts = [number for number, values in attempts.items() if "passed" in values]
            if any(failed < passed for failed in failed_attempts for passed in passed_attempts):
                stats[name].flips += 1

    candidates = [
        test
        for test in stats.values()
        if test.flips or test.retries or test.main_failures
    ]
    return sorted(
        candidates,
        key=lambda test: (test.score, test.fail_rate, test.last_seen, test.name),
        reverse=True,
    )


def checked_tests(issue_body):
    checked = set()
    pattern = re.compile(r"^- \[[xX]\].*<!-- flaky-test:([A-Za-z0-9_-]+) -->$", re.MULTILINE)
    for encoded in pattern.findall(issue_body or ""):
        padding = "=" * (-len(encoded) % 4)
        try:
            checked.add(base64.urlsafe_b64decode(encoded + padding).decode())
        except (ValueError, UnicodeDecodeError):
            continue
    return checked


def render_report(candidates, run_count, artifact_count, days, existing_body="", limit=100):
    shown = candidates[:limit]
    claimed = checked_tests(existing_body)
    generated = utc_now().strftime("%Y-%m-%d %H:%M UTC")
    lines = [
        ISSUE_MARKER,
        f"# {ISSUE_TITLE}",
        "",
        f"Generated {generated} from **{run_count}** CI runs and **{artifact_count}** test-report artifacts in the last **{days} days**.",
        "",
        "Score = 5 × cross-attempt flips + 3 × runs with in-run retries + 2 × runs failing on `main`. "
        "Failures seen only on a feature branch are not treated as flaky unless they later pass in another attempt of the same run.",
        "",
        "| Test | Score | Runs | Failures | Flips | Retries | Main failures | Fail rate | p50 | p95 | Last seen | Failing jobs |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |",
    ]

    if not shown:
        lines.append("| No known flaky tests in this window | 0 | 0 | 0 | 0 | 0 | 0 | 0% | — | — | — | — |")
    for test in shown:
        links = " ".join(
            f"[log {number}]({url})" for number, url in enumerate(test.links[-3:], start=1)
        ) or "—"
        lines.append(
            "| {name} | {score} | {runs} | {failures} | {flips} | {retries} | {main} | {rate:.1%} | {p50:.0f} ms | {p95:.0f} ms | {seen} | {links} |".format(
                name=markdown_code(test.name),
                score=test.score,
                runs=len(test.runs),
                failures=test.failures,
                flips=test.flips,
                retries=test.retries,
                main=test.main_failures,
                rate=test.fail_rate,
                p50=percentile(test.durations, 0.50),
                p95=percentile(test.durations, 0.95),
                seen=format_time(test.last_seen),
                links=links,
            )
        )

    lines.extend(["", "## Claim a test", ""])
    if not shown:
        lines.append("No tests to claim.")
    for test in shown:
        encoded = base64.urlsafe_b64encode(test.name.encode()).decode().rstrip("=")
        mark = "x" if test.name in claimed else " "
        lines.append(f"- [{mark}] <code>{html.escape(test.name)}</code> <!-- flaky-test:{encoded} -->")

    if len(candidates) > len(shown):
        lines.extend(["", f"Showing the top {len(shown)} of {len(candidates)} known flaky tests."])
    return "\n".join(lines) + "\n"


class Collector:
    def __init__(self, api, workflow, days, run_ids=None):
        self.api = api
        self.workflow = workflow
        self.days = days
        self.run_ids = run_ids or []
        self.jobs = {}

    def runs(self):
        if self.run_ids:
            return [self.api.json(f"/repos/{self.api.repo}/actions/runs/{run_id}") for run_id in self.run_ids]

        now = utc_now()
        since = now - dt.timedelta(days=self.days)
        runs = {}
        day = since.replace(hour=0, minute=0, second=0, microsecond=0)
        while day <= now:
            end = min(day + dt.timedelta(days=1) - dt.timedelta(seconds=1), now)
            created = urllib.parse.quote(f"{day.isoformat()}..{end.isoformat()}")
            path = f"/repos/{self.api.repo}/actions/workflows/{self.workflow}/runs?created={created}"
            for run in self.api.paginated(path, "workflow_runs"):
                if parse_time(run["created_at"]) >= since:
                    runs[run["id"]] = run
            day += dt.timedelta(days=1)
        return list(runs.values())

    def attempts(self, run):
        count = int(run.get("run_attempt", 1))
        if count == 1:
            return {1: run}
        return {
            number: self.api.json(
                f"/repos/{self.api.repo}/actions/runs/{run['id']}/attempts/{number}"
            )
            for number in range(1, count + 1)
        }

    def artifacts(self, runs):
        result = []
        for run in runs:
            path = f"/repos/{self.api.repo}/actions/runs/{run['id']}/artifacts"
            for artifact in self.api.paginated(path, "artifacts"):
                if REPORT_NAME.search(artifact["name"]) and not artifact.get("expired", False):
                    result.append((run, artifact))
        return result

    def infer_attempt(self, artifact, attempts):
        match = REPORT_NAME.search(artifact["name"])
        if match and match.group("attempt"):
            return int(match.group("attempt"))
        created = parse_time(artifact["created_at"])
        started = [
            (number, parse_time(attempt["run_started_at"]))
            for number, attempt in attempts.items()
            if attempt.get("run_started_at")
        ]
        eligible = [item for item in started if item[1] <= created]
        return max(eligible, key=lambda item: item[1])[0] if eligible else 1

    def download_report(self, run, artifact, attempts):
        archive = self.api.request(artifact["archive_download_url"], absolute=True)
        reports = []
        with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
            for name in zipped.namelist():
                if name.endswith(".json"):
                    report = json.loads(zipped.read(name))
                    if isinstance(report.get("results", {}).get("tests"), list):
                        reports.append(report)
        if not reports:
            raise ValueError(f"artifact {artifact['id']} contains no CTRF report")

        attempt = self.infer_attempt(artifact, attempts)
        seen_at = parse_time(artifact["created_at"])
        observations = []
        for report in reports:
            for test in report["results"]["tests"]:
                retries = test.get("retries", 0)
                if not isinstance(retries, int) or retries < 0:
                    retries = 0
                if retries == 0 and test.get("flaky") is True:
                    retries = 1
                duration = test.get("duration", 0)
                if not isinstance(duration, (int, float)) or duration < 0:
                    duration = 0
                observations.append(
                    Observation(
                        name=str(test.get("name", "<unnamed test>")),
                        status=str(test.get("status", "unknown")).lower(),
                        duration=duration,
                        retries=retries,
                        run_id=run["id"],
                        attempt=attempt,
                        branch=run.get("head_branch") or "",
                        seen_at=seen_at,
                        artifact_name=artifact["name"],
                        run_url=run["html_url"],
                    )
                )
        return observations

    def job_url(self, observation):
        key = (observation.run_id, observation.attempt)
        if key not in self.jobs:
            path = f"/repos/{self.api.repo}/actions/runs/{observation.run_id}/attempts/{observation.attempt}/jobs"
            self.jobs[key] = list(self.api.paginated(path, "jobs"))
        fallback = f"{observation.run_url}/attempts/{observation.attempt}"
        return select_job_url(self.jobs[key], observation.artifact_name, fallback)

    def collect(self):
        runs = self.runs()
        attempts_by_run = {run["id"]: self.attempts(run) for run in runs}
        artifacts = self.artifacts(runs)
        observations = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
            futures = [
                executor.submit(self.download_report, run, artifact, attempts_by_run[run["id"]])
                for run, artifact in artifacts
            ]
            for future in concurrent.futures.as_completed(futures):
                observations.extend(future.result())

        for observation in observations:
            if observation.status == "failed":
                observation.job_url = self.job_url(observation)
        return observations, len(runs), len(artifacts)


def find_issue(api):
    query = urllib.parse.quote(f'repo:{api.repo} is:issue in:title "{ISSUE_TITLE}"')
    exact_match = None
    for issue in api.json(f"/search/issues?q={query}&per_page=100")["items"]:
        if issue["title"] != ISSUE_TITLE:
            continue
        if ISSUE_MARKER in (issue.get("body") or ""):
            return issue
        exact_match = exact_match or issue
    return exact_match


def ensure_label(api):
    path = f"/repos/{api.repo}/labels/flaky-test"
    try:
        api.json(path)
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        api.json(
            f"/repos/{api.repo}/labels",
            "POST",
            {"name": "flaky-test", "color": "d73a4a", "description": "Tests with evidence of intermittent failure"},
        )


def pin_issue(api, issue):
    query = "query($id:ID!){node(id:$id){... on Issue{isPinned}}}"
    result = api.json("/graphql", "POST", {"query": query, "variables": {"id": issue["node_id"]}})
    if result.get("errors"):
        raise RuntimeError(f"could not query issue pin: {result['errors']}")
    if result.get("data", {}).get("node", {}).get("isPinned"):
        return
    mutation = "mutation($id:ID!){pinIssue(input:{issueId:$id}){issue{id}}}"
    result = api.json("/graphql", "POST", {"query": mutation, "variables": {"id": issue["node_id"]}})
    if result.get("errors"):
        raise RuntimeError(f"could not pin issue: {result['errors']}")


def upsert_issue(api, body):
    ensure_label(api)
    issue = find_issue(api)
    if issue:
        labels = {label["name"] for label in issue.get("labels", [])}
        labels.add("flaky-test")
        data = {"title": ISSUE_TITLE, "body": body, "labels": sorted(labels), "state": "open"}
        issue = api.json(f"/repos/{api.repo}/issues/{issue['number']}", "PATCH", data)
    else:
        data = {"title": ISSUE_TITLE, "body": body, "labels": ["flaky-test"]}
        issue = api.json(f"/repos/{api.repo}/issues", "POST", data)
    pin_issue(api, issue)
    return issue["html_url"]


def parse_args():
    parser = argparse.ArgumentParser(description="Aggregate CTRF artifacts into a known-flaky-tests report")
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--workflow", default="ci.yaml")
    parser.add_argument("--days", type=int, default=30)
    parser.add_argument("--run-id", type=int, action="append", default=[])
    parser.add_argument("--limit", type=int, default=100)
    parser.add_argument("--output")
    parser.add_argument("--summary")
    parser.add_argument("--update-issue", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not args.repo:
        raise SystemExit("--repo or GITHUB_REPOSITORY is required")
    if not token:
        raise SystemExit("GH_TOKEN or GITHUB_TOKEN is required")
    if args.days < 1:
        raise SystemExit("--days must be at least 1")

    api = GitHubApi(args.repo, token)
    collector = Collector(api, args.workflow, args.days, args.run_id)
    observations, run_count, artifact_count = collector.collect()
    candidates = aggregate(observations)

    issue = find_issue(api) if args.update_issue else None
    body = render_report(
        candidates,
        run_count,
        artifact_count,
        args.days,
        existing_body=(issue or {}).get("body", ""),
        limit=args.limit,
    )
    if args.output:
        Path(args.output).write_text(body)
    else:
        print(body, end="")
    if args.summary:
        Path(args.summary).write_text(body)
    if args.update_issue:
        print(f"Updated {upsert_issue(api, body)}", file=sys.stderr)


if __name__ == "__main__":
    main()
