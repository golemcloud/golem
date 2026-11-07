"""Focused pure/mocked checks for the temporary source campaign."""

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import yaml

SPEC = importlib.util.spec_from_file_location(
    "campaign", Path(__file__).with_name("ci-campaign.py")
)
campaign = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(campaign)
A = "a" * 40
B = "b" * 40


class CampaignTests(unittest.TestCase):
    def test_hashes_are_required_exact_and_different(self):
        campaign.validate_hashes(A, B)
        for a, b in ((A, A.upper()), ("main", B), (A, B[:39]), ("-" * 40, B)):
            with self.subTest(a=a, b=b), self.assertRaises(ValueError):
                campaign.validate_hashes(a, b)

    def test_same_source_inventory_matches_and_changed_source_additions_are_reported(
        self,
    ):
        prime = {"unit": [["suite", "test", 2], ["suite", "removed", 1]]}
        self.assertEqual(campaign.reconcile_inventories("warm", prime, prime), {})
        changed = {"unit": [["suite", "test", 1], ["suite", "added", 1]]}
        with self.assertRaisesRegex(ValueError, "test identities differ"):
            campaign.reconcile_inventories("warm", changed, prime)
        self.assertEqual(
            campaign.reconcile_inventories("changed", changed, prime),
            {
                "added": [["unit", "suite", "added", 1]],
                "removed": [
                    ["unit", "suite", "removed", 1],
                    ["unit", "suite", "test", 1],
                ],
            },
        )

    def test_stage_caps_and_absolute_budget_include_setup_and_finalization(self):
        self.assertEqual(campaign.phase_seconds(0, 720, 330, 60 * 10), 330 * 60)
        self.assertEqual(campaign.phase_seconds(0, 720, 210, 60 * 600), 105 * 60)
        self.assertEqual(campaign.phase_seconds(0, 720, 210, 60 * 706), 0)
        self.assertEqual(campaign.phase_seconds(0, 120, 330, 60 * 100), 5 * 60)
        self.assertEqual(campaign.minimum_seconds("linux", 300), 300 * 60)
        self.assertEqual(campaign.minimum_seconds("linux", 180), 180 * 60)
        self.assertEqual(campaign.minimum_seconds("macos", 300), 15 * 60)

    def report(self):
        return {
            "success": True,
            "fully_green": False,
            "finished_at": 10,
            "source_sha": A,
            "test_retries": 2,
            "cli_test_threads": 4,
            "prerequisite_mode": "serial",
            "phase": "all",
            "workers": 1,
            "shard": 0,
            "stages": [
                {
                    "id": "unit-tests-and-checks",
                    "status": "passed_after_retry",
                    "finished_at": 9,
                    "steps": [
                        {"attempt": 0, "log": "00.log", "exit_code": 101},
                        {"attempt": 1, "log": "00-retry1.log", "exit_code": 0},
                    ],
                }
            ],
        }

    def write_initial(self, root, tests=None):
        directory = root / "unit-tests-and-checks"
        directory.mkdir(exist_ok=True)
        tests = (
            [{"name": "initial failure", "status": "failed"}]
            if tests is None
            else tests
        )
        (directory / "ctrf-1-0-0.json").write_text(
            json.dumps({"results": {"tests": tests, "summary": {"tests": len(tests)}}})
        )

    def test_recovered_success_accepts_initial_failures_not_fully_green(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_initial(root)
            (root / "unit-tests-and-checks/ctrf-1-0-1.json").write_text(
                json.dumps({"results": {"tests": [], "summary": {"tests": 0}}})
            )
            # Focused retry may omit passing tests; do not compare final inventory.
            campaign.admit(
                self.report(),
                ["unit-tests-and-checks"],
                A,
                root,
                {"unit-tests-and-checks"},
            )

    def test_admission_rejects_missing_initial_reports_and_zero_tests(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, "Missing initial"):
                campaign.admit(
                    self.report(),
                    ["unit-tests-and-checks"],
                    A,
                    root,
                    {"unit-tests-and-checks"},
                )
            self.write_initial(root, [])
            with self.assertRaisesRegex(ValueError, "actual initial"):
                campaign.admit(
                    self.report(),
                    ["unit-tests-and-checks"],
                    A,
                    root,
                    {"unit-tests-and-checks"},
                )

    def test_admission_fails_closed_for_incomplete_and_mismatched_reports(self):
        mutations = [
            {"success": False},
            {"finished_at": None},
            {"source_sha": B},
            {"test_retries": 0},
            {"test_retries": 3},
            {"cli_test_threads": 8},
            {"prerequisite_mode": "overlap"},
            {"phase": "checks"},
            {"workers": 2},
            {"stages": []},
            {"stages": self.report()["stages"] * 2},
        ]
        for mutation in mutations:
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                campaign.admit(
                    {**self.report(), **mutation},
                    ["unit-tests-and-checks"],
                    A,
                    Path("unused"),
                    set(),
                )
        for status in ("running", "blocked", "failed"):
            report = self.report()
            report["stages"][0]["status"] = status
            with self.subTest(status=status), self.assertRaises(ValueError):
                campaign.admit(
                    report, ["unit-tests-and-checks"], A, Path("unused"), set()
                )
        report = self.report()
        report["stages"][0]["steps"][1]["attempt"] = 3
        with self.assertRaisesRegex(ValueError, "Unbounded"):
            campaign.admit(report, ["unit-tests-and-checks"], A, Path("unused"), set())
        for mutation in (
            {"steps": []},
            {"validation_error": "bad inventory"},
            {
                "steps": [
                    {
                        "attempt": 0,
                        "log": "00.log",
                        "exit_code": 0,
                        "validation_error": "bad retry",
                    }
                ]
            },
            {"steps": [{"attempt": 0, "log": "00.log", "exit_code": 101}]},
        ):
            report = self.report()
            report["stages"][0].update(mutation)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                campaign.admit(
                    report, ["unit-tests-and-checks"], A, Path("unused"), set()
                )

    def test_preflight_exact_objects_and_meaningful_diff(self):
        def fake_git(*args):
            if args[0] == "rev-parse":
                value = args[1]
                if value == "--is-shallow-repository":
                    return "true"
                if value.endswith("^{commit}"):
                    return value[:40]
                return "same-contract"
            if args[:2] == ("diff", "--name-only"):
                return "golem-common/src/lib.rs"
            if args[0] == "diff":
                return "captured diff"
            self.fail(args)

        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(campaign, "git", side_effect=fake_git),
            patch.object(campaign.subprocess, "run") as run,
        ):
            root = Path(temporary)
            campaign.preflight(A, B, root)
            self.assertEqual((root / "source.diff").read_text(), "captured diff")
            self.assertEqual(
                json.loads((root / "preflight.json").read_text())[
                    "native_rust_changes"
                ],
                ["golem-common/src/lib.rs"],
            )
            calls = [call.args[0] for call in run.call_args_list]
            self.assertIn(["git", "fetch", "--no-tags", "origin", A], calls)
            self.assertIn(
                ["git", "fetch", "--no-tags", "--unshallow", "origin", A, B], calls
            )
            self.assertIn(["git", "merge-base", "--is-ancestor", A, B], calls)

    def mac_scenario(self, results):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        commands = []

        def execute(command, env, cwd, log, timeout):
            commands.append(command)
            code, text = results[len(commands) - 1]
            log.write_text(text)
            if log.name == "select.log":
                (root / "executable.tsv").write_text("lib\t/path/test-bin\n")
            return {"exit_code": code, "seconds": 1, "log": log.name}

        with patch.object(campaign, "mac_execute", side_effect=execute):
            accepted = campaign.mac_phase(root, root, {"GITHUB_SHA": A}, 10000)
        return accepted, commands, json.loads((root / "attempts.json").read_text())

    def test_macos_retries_only_failed_tests_and_keeps_compile_once(self):
        accepted, commands, attempts = self.mac_scenario(
            [
                (0, ""),
                (0, ""),
                (101, "test result: FAILED; 1 passed; 1 failed;"),
                (101, "test result: FAILED; 1 passed; 1 failed;"),
                (0, "test result: ok; 2 passed;"),
            ]
        )
        self.assertTrue(accepted)
        self.assertEqual(sum("cargo test --locked" in c for c in commands), 1)
        self.assertEqual(len(attempts), 5)
        self.assertTrue(all(a["source_sha"] == A for a in attempts))

    def test_macos_rejects_compile_failure_timeout_zero_and_persistent_failure(self):
        for results in (
            [(101, "compile error")],
            [(124, "timeout")],
            [(0, ""), (0, ""), (0, "test result: ok; 0 passed;")],
            [(0, ""), (0, ""), (124, "test result: FAILED;")],
            [(0, ""), (0, ""), (101, "test result: FAILED; 0 passed; 0 failed;")],
            [(0, ""), (0, "")]
            + [(101, "test result: FAILED; 0 passed; 1 failed;")] * 3,
        ):
            with self.subTest(results=results):
                accepted, commands, _ = self.mac_scenario(results)
                self.assertFalse(accepted)
                self.assertEqual(len(commands), len(results))

    def test_macos_timeout_cleans_only_owned_process_group(self):
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(campaign.subprocess, "Popen") as popen,
            patch.object(campaign.os, "killpg") as killpg,
        ):
            process = popen.return_value
            process.pid = 123
            process.poll.return_value = None
            process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 1), 0, 0]
            observation = campaign.mac_execute(
                "fixture", {}, Path(temporary), Path(temporary) / "log", 1
            )
            self.assertEqual(observation["exit_code"], 124)
            self.assertEqual(
                [call.args for call in killpg.call_args_list],
                [(123, campaign.signal.SIGTERM), (123, campaign.signal.SIGKILL)],
            )
            self.assertTrue(popen.call_args.kwargs["start_new_session"])

    def test_campaign_stops_after_persistent_phase_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "driver"
            (root / ".github/workflows").mkdir(parents=True)
            (root / ".github/workflows/ci.yaml").write_text("jobs: {}")
            start = root / "start"
            start.write_text("0")
            harness = Mock()
            harness.yaml = yaml
            harness.inventory.return_value = []
            harness.execute.return_value = {"exit_code": 1, "seconds": 1}
            args = SimpleNamespace(
                source_a=A, source_b=B, allocation_start=start, platform="linux"
            )

            def fake_git(*args):
                return A if args[0] == "-C" else "same"

            with (
                patch.object(campaign, "ROOT", root),
                patch.object(campaign, "load_harness", return_value=harness),
                patch.object(campaign, "git", side_effect=fake_git),
                patch.object(campaign.subprocess, "run"),
                patch.object(campaign.subprocess, "check_output", return_value="[]"),
                patch.object(campaign.time, "time", return_value=10),
            ):
                self.assertEqual(campaign.campaign(args), 1)
            self.assertEqual(harness.execute.call_count, 1)
            report = json.loads(
                (root / "tmp/experiment-results/campaign-linux.json").read_text()
            )
            self.assertFalse(report["success"])
            self.assertEqual([p["phase"] for p in report["phases"]], ["prime"])
            call = harness.execute.call_args
            self.assertIn(str(campaign.HARNESS), call.args[0])
            self.assertEqual(call.args[1]["GITHUB_SHA"], A)
            self.assertTrue(
                call.args[1]["GITHUB_WORKSPACE"].endswith("-campaign-main-suite")
            )
            harness.execute.reset_mock()
            with (
                patch.object(campaign, "ROOT", root),
                patch.object(campaign, "load_harness", return_value=harness),
                patch.object(campaign, "git", side_effect=fake_git),
                patch.object(campaign.subprocess, "run"),
                patch.object(campaign.subprocess, "check_output", return_value="[]"),
                patch.object(campaign.time, "time", return_value=704 * 60),
            ):
                self.assertEqual(campaign.campaign(args), 1)
            harness.execute.assert_not_called()
            report = json.loads(
                (root / "tmp/experiment-results/campaign-linux.json").read_text()
            )
            self.assertEqual(report["phases"][0]["status"], "incomplete_budget")

    def test_all_three_worktrees_transition_together_and_workloads_use_exact_shas(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "driver"
            (root / ".github/workflows").mkdir(parents=True)
            (root / ".github/workflows/ci.yaml").write_text("jobs: {}")
            start = root / "start"
            start.write_text("0")
            harness = Mock()
            harness.yaml = yaml
            harness.inventory.return_value = []
            args = SimpleNamespace(
                source_a=A, source_b=B, allocation_start=start, platform="linux"
            )
            transitions = []
            current = {}

            def run(command, cwd, **_):
                if command[1] == "checkout":
                    current[cwd] = command[-1]
                    transitions.append((cwd, command[-1]))

            def git(*args):
                return current[Path(args[1])] if args[0] == "-C" else "same"

            def execute(command, env, cwd, log, timeout):
                self.assertEqual(set(current.values()), {env["GITHUB_SHA"]})
                phase = log.stem
                actual = cwd / "tmp/experiment-results" / phase
                actual.mkdir(parents=True)
                (actual / "all-0.json").write_text('{"stages": []}')
                self.assertGreater(timeout, 0)
                return {"exit_code": 0, "seconds": 1}

            harness.execute.side_effect = execute
            with (
                patch.object(campaign, "ROOT", root),
                patch.object(campaign, "load_harness", return_value=harness),
                patch.object(campaign, "git", side_effect=git),
                patch.object(campaign.subprocess, "run", side_effect=run),
                patch.object(campaign.subprocess, "check_output", return_value="[]"),
                patch.object(campaign.time, "time", return_value=10),
                patch.object(campaign, "admit"),
            ):
                self.assertEqual(campaign.campaign(args), 0)
            self.assertEqual([sha for _, sha in transitions], [A] * 6 + [B] * 3)
            self.assertEqual(len({path for path, _ in transitions}), 3)
            self.assertEqual(
                [call.args[1]["GITHUB_SHA"] for call in harness.execute.call_args_list],
                [A, A, B],
            )

    def test_workflow_dispatch_only_and_both_required_workers(self):
        workflow = yaml.load(
            (campaign.ROOT / ".github/workflows/ci-experiment.yaml").read_text(),
            Loader=yaml.BaseLoader,
        )
        self.assertEqual(set(workflow["on"]), {"workflow_dispatch"})
        self.assertEqual(
            set(workflow["on"]["workflow_dispatch"]["inputs"]), {"source_a", "source_b"}
        )
        for name, minutes in (("linux", "720"), ("macos", "120")):
            job = workflow["jobs"][name]
            self.assertEqual(job["timeout-minutes"], minutes)
            self.assertEqual(job["needs"], "validate-sources")
            self.assertEqual(job["steps"][-1]["if"], "always()")


if __name__ == "__main__":
    unittest.main()
