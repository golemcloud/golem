import unittest
from contextlib import redirect_stdout
from datetime import UTC, datetime, timedelta
from io import StringIO
from unittest.mock import patch

import cleanup_sticky_disks as cleanup

REPO = "golemcloud/golem"
NOW = datetime(2026, 11, 13, 12, tzinfo=UTC)


def disk(rid, hours=2, **fields):
    return {
        "key": f"{REPO}-native-binaries-{rid}-producer-attempt1",
        "type": "stickydisk",
        "arch": "amd64",
        "region": "eu-central",
        "last_used_at": (NOW - timedelta(hours=hours)).isoformat(),
        **fields,
    }


def run(rid, **fields):
    return {
        "head_branch": f"pr-{rid}",
        "status": "completed",
        "run_attempt": 1,
        "pull_requests": [{"number": rid}],
        **fields,
    }


class CleanupTests(unittest.TestCase):
    def test_high_water_is_strict_and_cleanup_stops_at_low_water(self):
        entries = [disk(i, hours=i + 2) for i in range(351)]
        runs = {i: run(i) for i in range(351)}
        prs = {i: {"state": "open"} for i in range(351)}
        self.assertEqual(cleanup.plan_cleanup(REPO, entries[:350], runs, prs, NOW), [])
        selected = cleanup.plan_cleanup(REPO, entries, runs, prs, NOW)
        self.assertEqual(
            [cleanup.run_id(REPO, e) for e in selected], list(range(350, 249, -1))
        )
        self.assertEqual(len(entries) - len(selected), 250)

    def test_closed_prs_precede_older_open_prs_and_last_use_not_creation_orders_them(
        self,
    ):
        entries = [disk(i) for i in range(347)] + [
            disk(1000, hours=20, created_at="2026-11-01T00:00:00Z"),
            disk(1001, hours=3, created_at="2026-11-12T00:00:00Z"),
            disk(1002, hours=5, created_at="2026-11-13T00:00:00Z"),
            disk(1003, hours=100),
        ]
        runs = {i: run(i) for i in range(1000, 1004)}
        prs = {
            1000: {"state": "closed"},
            1001: {"state": "closed"},
            1002: {"state": "closed"},
            1003: {"state": "open"},
        }
        selected = cleanup.plan_cleanup(REPO, entries, runs, prs, NOW)
        self.assertEqual(
            [cleanup.run_id(REPO, e) for e in selected], [1000, 1002, 1001, 1003]
        )

    def test_main_active_recent_unknown_and_ambiguous_delete_scopes_are_protected(self):
        entries = [disk(i) for i in range(340)] + [
            disk(1000, hours=100),  # main
            disk(1001, hours=100),  # active
            disk(1002, hours=0.99),  # grace period
            disk(1003, hours=1),  # exactly at the grace boundary is eligible
            disk(1004),  # missing run
            disk(1005),  # missing PR
            disk(1006),
            disk(1006, region="us-east-1"),  # CLI cannot select region
            disk(1007, key=f"{REPO}-docker"),
            disk(1008, key="other/repo-native-binaries-1008-producer-attempt1"),
            disk(1009),  # no PR association
        ]
        runs = {i: run(i) for i in range(1000, 1010) if i != 1004}
        runs[1000]["head_branch"] = "main"
        runs[1001]["status"] = "in_progress"
        runs[1009]["pull_requests"] = []
        prs = {i: {"state": "closed"} for i in range(1000, 1010) if i != 1005}
        selected = cleanup.plan_cleanup(REPO, entries, runs, prs, NOW)
        self.assertEqual(selected, [disk(1003, hours=1)])

    def test_apply_skips_rerunning_or_changed_disks_and_verifies_the_other_deletions(
        self,
    ):
        entries = [disk(i, hours=10000 + i) for i in range(351)]
        runs = {i: run(i) for i in range(351)}
        prs = {i: {"state": "open"} for i in range(351)}
        state = list(entries)
        state[350] = disk(350, hours=0)  # used after the plan was collected

        snapshots = iter([entries])

        def inventory(_repo):
            return next(snapshots, list(state))

        def github(path):
            rid = int(path.rsplit("/", 1)[1])
            return run(
                rid,
                status="in_progress" if rid == 349 else "completed",
                run_attempt=2 if rid == 348 else 1,
            )

        def blacksmith(_repo, *args):
            key = args[args.index("--key") + 1]
            state[:] = [entry for entry in state if entry["key"] != key]
            return "purged 1 sticky disks"

        with (
            patch.object(cleanup, "inventory", side_effect=inventory),
            patch.object(cleanup, "metadata", return_value=(runs, prs)),
            patch.object(cleanup, "github", side_effect=github),
            patch.object(cleanup, "blacksmith", side_effect=blacksmith),
            redirect_stdout(StringIO()) as output,
        ):
            cleanup.cleanup(REPO, apply=True)
        self.assertEqual(
            {cleanup.run_id(REPO, e) for e in state}, {348, 349, 350, *range(250)}
        )
        self.assertIn("Verified 98 deletions; 253 disks remain", output.getvalue())

    def test_missing_run_is_protected_and_branch_lookup_does_not_match_another_fork(
        self,
    ):
        def github(path):
            if path.endswith("/actions/runs/1"):
                return None
            if path.endswith("/actions/runs/2"):
                return run(
                    2,
                    event="pull_request",
                    pull_requests=[],
                    head_branch="feature",
                    head_repository={"id": 10, "owner": {"login": "golemcloud"}},
                )
            if "/pulls?" in path:
                return [
                    {
                        "number": 20,
                        "state": "closed",
                        "head": {"ref": "feature", "repo": {"id": 99}},
                    },
                    {
                        "number": 21,
                        "state": "open",
                        "head": {"ref": "feature", "repo": {"id": 10}},
                    },
                    {
                        "number": 22,
                        "state": "closed",
                        "head": {"ref": "other", "repo": {"id": 10}},
                    },
                ]
            self.fail(f"Unexpected GitHub lookup: {path}")

        entries = [disk(i, key=f"unowned-{i}") for i in range(349)] + [disk(1), disk(2)]
        with patch.object(cleanup, "github", side_effect=github):
            runs, prs = cleanup.metadata(REPO, entries)
        self.assertEqual(runs[2]["pull_requests"], [{"number": 21}])
        self.assertEqual(set(prs), {21})
        self.assertEqual(cleanup.plan_cleanup(REPO, entries, runs, prs, NOW), [disk(2)])

    def test_main_protection_wins_over_capacity_and_reports_failure(self):
        entries = [disk(i, hours=10000) for i in range(351)]
        runs = {i: run(i, head_branch="main") for i in range(351)}
        with (
            patch.object(cleanup, "inventory", return_value=entries),
            patch.object(cleanup, "metadata", return_value=(runs, {})),
            redirect_stdout(StringIO()),
            self.assertRaisesRegex(RuntimeError, "above the high-water mark"),
        ):
            cleanup.cleanup(REPO, apply=True)
        self.assertEqual(len(entries), 351)

    def test_dry_run_leaves_the_inventory_unchanged(self):
        entries = [disk(i) for i in range(351)]
        runs = {i: run(i) for i in range(351)}
        prs = {i: {"state": "open"} for i in range(351)}
        with (
            patch.object(cleanup, "inventory", return_value=entries),
            patch.object(cleanup, "metadata", return_value=(runs, prs)),
            redirect_stdout(StringIO()) as output,
        ):
            cleanup.cleanup(REPO)
        self.assertIn("DRY RUN:", output.getvalue())
        self.assertEqual(len(entries), 351)


if __name__ == "__main__":
    unittest.main()
