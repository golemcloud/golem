#!/usr/bin/env python3

import datetime as dt
import io
import json
import unittest
import zipfile

from flaky_tests import Collector, Observation, aggregate, checked_tests, render_report, select_job_url


NOW = dt.datetime(2026, 9, 3, tzinfo=dt.timezone.utc)


def observation(name, status, run_id, attempt, branch="feature", retries=0, duration=10):
    return Observation(
        name=name,
        status=status,
        duration=duration,
        retries=retries,
        run_id=run_id,
        attempt=attempt,
        branch=branch,
        seen_at=NOW + dt.timedelta(minutes=run_id + attempt),
        artifact_name="unit-tests-report-attempt1",
        run_url=f"https://example.test/runs/{run_id}",
        job_url=f"https://example.test/jobs/{run_id}-{attempt}" if status == "failed" else "",
    )


def report_archive(tests):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        archive.writestr("ctrf-report.json", json.dumps({"results": {"tests": tests}}))
    return output.getvalue()


class FakeApi:
    repo = "golemcloud/golem"

    def __init__(self, archives):
        self.archives = archives

    def request(self, url, absolute=False):
        return self.archives[url]


class AggregateTests(unittest.TestCase):
    def test_aggregates_synthetic_ctrf_reports_across_attempts(self):
        first = report_archive(
            [
                {"name": "suite::flip", "status": "failed", "duration": 10},
                {"name": "suite::retry", "status": "passed", "duration": 20, "retries": 2},
            ]
        )
        second = report_archive(
            [
                {"name": "suite::flip", "status": "passed", "duration": 30},
                {"name": "suite::retry", "status": "passed", "duration": 40},
            ]
        )
        run = {"id": 123, "head_branch": "feature", "html_url": "https://example.test/runs/123"}
        attempts = {
            1: {"run_started_at": "2026-09-03T10:00:00Z"},
            2: {"run_started_at": "2026-09-03T11:00:00Z"},
        }
        artifacts = [
            {
                "id": 1,
                "name": "unit-tests-report-attempt1",
                "created_at": "2026-09-03T10:30:00Z",
                "archive_download_url": "first",
            },
            {
                "id": 2,
                "name": "unit-tests-report-attempt2",
                "created_at": "2026-09-03T11:30:00Z",
                "archive_download_url": "second",
            },
        ]
        collector = Collector(FakeApi({"first": first, "second": second}), "ci.yaml", 30)
        observations = [
            item
            for artifact in artifacts
            for item in collector.download_report(run, artifact, attempts)
        ]

        result = {test.name: test for test in aggregate(observations)}

        self.assertEqual(result["suite::flip"].flips, 1)
        self.assertEqual(result["suite::retry"].retries, 2)

    def test_aggregates_flip_retries_main_failures_and_durations(self):
        observations = [
            observation("flip", "failed", 1, 1, duration=10),
            observation("flip", "passed", 1, 2, duration=20),
            observation("flip", "passed", 2, 1, duration=30),
            observation("retry", "passed", 3, 1, retries=2, duration=100),
            observation("main failure", "failed", 4, 1, branch="main", duration=200),
            observation("ordinary failure", "failed", 5, 1),
        ]

        result = {test.name: test for test in aggregate(observations)}

        self.assertEqual(set(result), {"flip", "retry", "main failure"})
        self.assertEqual(result["flip"].flips, 1)
        self.assertEqual(len(result["flip"].runs), 2)
        self.assertEqual(result["flip"].fail_rate, 0.5)
        self.assertEqual(result["flip"].score, 5)
        self.assertEqual(result["retry"].retries, 2)
        self.assertEqual(result["retry"].score, 3)
        self.assertEqual(result["main failure"].main_failures, 1)
        self.assertEqual(result["main failure"].score, 2)

    def test_report_preserves_claimed_checkboxes(self):
        test = aggregate([observation("suite::flaky", "passed", 1, 1, retries=1)])[0]
        first = render_report([test], 1, 1, 30)
        claimed = first.replace("- [ ] <code>suite::flaky</code>", "- [x] <code>suite::flaky</code>")

        second = render_report([test], 2, 2, 30, existing_body=claimed)

        self.assertEqual(checked_tests(second), {"suite::flaky"})
        self.assertIn("- [x] <code>suite::flaky</code>", second)

    def test_selects_the_job_that_produced_each_report(self):
        jobs = [
            {"name": "worker-tests-group2", "html_url": "worker-url"},
            {"name": "it (integration-tests-group4, IT #4)", "html_url": "it-url"},
            {"name": "it-cli (bridge_gen)", "html_url": "cli-url"},
        ]

        self.assertEqual(
            select_job_url(jobs, "worker-executor-tests-group2-report-attempt2", "fallback"),
            "worker-url",
        )
        self.assertEqual(
            select_job_url(jobs, "integration-tests-group4-report-attempt1", "fallback"),
            "it-url",
        )
        self.assertEqual(
            select_job_url(jobs, "cli-integration-tests-bridge-report-attempt1", "fallback"),
            "cli-url",
        )

    def test_selects_exact_integration_group_when_group_name_is_prefix_of_another(self):
        jobs = [
            {"name": "it (integration-tests-group10, IT #10)", "html_url": "group10-url"},
            {"name": "it (integration-tests-group1, IT #1)", "html_url": "group1-url"},
        ]

        self.assertEqual(
            select_job_url(jobs, "integration-tests-group1-report-attempt1", "fallback"),
            "group1-url",
        )


if __name__ == "__main__":
    unittest.main()
