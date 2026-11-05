import copy
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import yaml

SPEC = importlib.util.spec_from_file_location(
    "experiment", Path(__file__).with_name("ci-experiment.py")
)
experiment = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(experiment)
OVERLAP_SPEC = importlib.util.spec_from_file_location(
    "overlap", Path(__file__).with_name("ci-overlap-experiment.py")
)
overlap = importlib.util.module_from_spec(OVERLAP_SPEC)
OVERLAP_SPEC.loader.exec_module(overlap)


class ExperimentInventoryTests(unittest.TestCase):
    def setUp(self):
        self.workflow = yaml.safe_load(
            (experiment.ROOT / ".github/workflows/ci.yaml").read_text()
        )
        self.chunks = [
            {"name": "rust-1", "needs-node": False, "needs-moonbit": False},
            {
                "name": "scala",
                "needs-scala": True,
                "expected-artifact": "golem_it_tool_streaming_scala.wasm",
            },
        ]
        self.stages = experiment.inventory(self.workflow, self.chunks)

    def context(self, stage):
        return {
            "matrix": stage["matrix"],
            "github": {"workspace": "/workspace/golem"},
            "steps": {"cache": {"outputs": {"cache-hit": "false"}}},
        }

    def test_ordinary_coverage_and_dependencies(self):
        seen = set(experiment.EXCLUDED)
        for stage in self.stages:
            self.assertLessEqual(
                set(experiment.dependencies(stage["definition"])), seen
            )
            self.assertTrue(list(experiment.commands(stage, self.context(stage))))
            seen.add(stage["job"])
        self.assertEqual(seen, set(self.workflow["jobs"]))
        self.assertEqual(sum(s["job"] == "worker-tests" for s in self.stages), 7)
        self.assertEqual(sum(s["job"] == "it" for s in self.stages), 14)
        self.assertEqual(sum(s["job"] == "it-cli" for s in self.stages), 7)

    def test_cli_priming_retains_transitive_prerequisites_without_other_checks(self):
        selected = experiment.select_stages(self.stages, "cli-prime")
        seen = set(experiment.EXCLUDED)
        for stage in selected:
            self.assertLessEqual(
                set(experiment.dependencies(stage["definition"])), seen
            )
            seen.add(stage["job"])
        self.assertEqual(
            {s["job"] for s in selected},
            {
                "builtin-artifacts",
                "build-service-tests",
                "build-cli-test-bins",
                "build-golem-ts",
                "build-golem-effect",
                "build-golem-scala",
                "build-test-components",
                "merge-test-components",
                *experiment.CLI_JOBS,
            },
        )
        self.assertEqual(sum(s["job"] == "build-test-components" for s in selected), 2)
        self.assertEqual(len(experiment.select_stages(self.stages, "cli")), 9)

    def test_cli_baseline_is_independent_but_requires_complete_clean_cli_results(self):
        report = {
            "finished_at": 100,
            "cli_test_threads": 4,
            "success": False,
            "fully_green": False,
            "stages": [
                {"id": s["id"], "status": "passed"}
                for s in experiment.select_stages(self.stages, "cli")
            ]
            + [{"id": "it/integration-tests-group1", "status": "failed"}],
        }
        self.assertTrue(experiment.cli_baseline_eligible(report, self.stages))
        for status in ("failed", "blocked", "running", "passed_after_retry"):
            changed = copy.deepcopy(report)
            changed["stages"][0]["status"] = status
            self.assertFalse(experiment.cli_baseline_eligible(changed, self.stages))
        for field, value in (("finished_at", None), ("cli_test_threads", 6)):
            changed = {**report, field: value}
            self.assertFalse(experiment.cli_baseline_eligible(changed, self.stages))
        self.assertFalse(experiment.cli_baseline_eligible({}, self.stages))
        report["stages"].pop(0)
        self.assertFalse(experiment.cli_baseline_eligible(report, self.stages))

    def test_fixtures_are_built_not_only_compiler_identified(self):
        for stage in self.stages:
            if stage["job"] != "build-test-components":
                continue
            commands = list(experiment.commands(stage, self.context(stage)))
            names = [s["name"] for s in commands]
            self.assertIn("Build test components", names)
            self.assertIn("Collect built WASMs", names)
            expected = stage["matrix"]["chunk"]["name"]
            build = next(s for s in commands if s["name"] == "Build test components")
            self.assertIn(
                expected, experiment.render(build["run"], self.context(stage))
            )

    def test_conditions_preserve_asymmetric_cli_selection(self):
        condition = (
            "matrix.group.shard != 'core' && matrix.group.shard != 'bridge_gen_rust'"
        )
        self.assertFalse(
            experiment.condition(condition, {"matrix": {"group": {"shard": "core"}}})
        )
        self.assertTrue(
            experiment.condition(condition, {"matrix": {"group": {"shard": "deploy"}}})
        )
        self.assertFalse(
            experiment.condition(
                "matrix.chunk.needs-scala", {"matrix": {"chunk": self.chunks[0]}}
            )
        )
        self.assertTrue(
            experiment.condition(
                "matrix.chunk.needs-scala", {"matrix": {"chunk": self.chunks[1]}}
            )
        )

    def test_new_jobs_and_actions_require_classification(self):
        changed = copy.deepcopy(self.workflow)
        changed["jobs"]["new-validation"] = {"steps": []}
        with self.assertRaises(ValueError):
            experiment.inventory(changed, self.chunks)
        stage = copy.deepcopy(self.stages[0])
        stage["definition"]["steps"].insert(0, {"uses": "unknown/validation@v1"})
        with self.assertRaises(ValueError):
            list(experiment.commands(stage, self.context(stage)))

    def test_native_build_and_archive_keep_identical_selection(self):
        for stage in self.stages:
            for command in experiment.commands(stage, self.context(stage)):
                if command["name"] != "Build and archive native selection":
                    continue
                source = next(
                    s
                    for s in stage["definition"]["steps"]
                    if s.get("uses") == "./.github/actions/publish-native-binaries"
                )
                selection = source["with"]["cargo-args"]
                self.assertEqual(command["run"].count(selection), 2)

    def test_restored_binaries_do_not_inherit_feature_build_fingerprints(self):
        for stage in self.stages:
            for command in experiment.commands(stage, self.context(stage)):
                if command["name"] != "Select exact native archive":
                    continue
                script = command["run"]
                if "cp -t target/debug/" not in script:
                    continue
                self.assertLess(
                    script.index("rm -rf target/debug\n"),
                    script.index("cp -t target/debug/"),
                )
                self.assertNotIn("rm -rf target/dev-ci", script)

    def test_cli_tuning_changes_only_cli_concurrency(self):
        for threads in (6, 8):
            changed = 0
            for stage in self.stages:
                original = list(experiment.commands(stage, self.context(stage)))
                tuned = list(experiment.commands(stage, self.context(stage), threads))
                self.assertEqual(len(original), len(tuned))
                for before, after in zip(original, tuned):
                    if before == after:
                        continue
                    changed += 1
                    self.assertIn(stage["job"], experiment.CLI_JOBS)
                    self.assertIn("cargo make --profile ci ", before["run"])
                    self.assertTrue(after["run"].endswith(before["run"]))
                    prefix = after["run"][: -len(before["run"])]
                    self.assertIn(f'CLI_TEST_THREADS = "{threads}"', prefix)
                    self.assertEqual(
                        {k: v for k, v in before.items() if k != "run"},
                        {k: v for k, v in after.items() if k != "run"},
                    )
            self.assertEqual(changed, 9)
        with self.assertRaisesRegex(ValueError, "must not retry"):
            experiment.run(SimpleNamespace(cli_test_threads=6, test_retries=1))


class ExperimentRetryTests(unittest.TestCase):
    def simulate(self, outcomes, retries, elapsed_per_attempt=0, stage_timeout=360):
        stages = [
            {
                "id": job,
                "job": job,
                "matrix": {},
                "definition": {
                    "needs": needs,
                    "timeout-minutes": stage_timeout,
                    "steps": [{"name": "Run tests", "run": "test-command"}],
                },
            }
            for job, needs in (
                ("build-golem-ts", []),
                ("worker-tests", ["build-golem-ts"]),
            )
        ]
        observations = iter(enumerate(outcomes))
        clock = [0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github/workflows/ci.yaml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text("jobs: {}")

            def execute(command, env, cwd, log, timeout):
                index, (code, output) = next(observations)
                clock[0] += elapsed_per_attempt
                log.write_text(output)
                report = root / "target/ctrf.json"
                report.parent.mkdir(exist_ok=True)
                report.write_text(json.dumps({"attempt_marker": index}))
                return {"exit_code": code, "seconds": 1, "resources": []}

            args = SimpleNamespace(
                cycle="cold",
                phase="all",
                workers=1,
                shard=0,
                cli_test_threads=4,
                test_retries=retries,
                inventory=False,
                check_cli_baseline=None,
            )
            with (
                patch.object(experiment, "ROOT", root),
                patch.object(experiment, "inventory", return_value=stages),
                patch.object(experiment, "execute", side_effect=execute),
                patch.object(
                    experiment.subprocess,
                    "check_output",
                    side_effect=[b"[]", "source-sha"],
                ),
                patch.object(experiment.subprocess, "run"),
                patch.object(experiment.time, "sleep"),
                patch.object(
                    experiment.time, "monotonic", side_effect=lambda: clock[0]
                ),
                patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}),
            ):
                code = experiment.run(args)
            evidence = root / "tmp/experiment-results/cold"
            return (
                code,
                json.loads((evidence / "all-0.json").read_text()),
                {
                    p.name: p.read_text()
                    for p in (evidence / "build-golem-ts").iterdir()
                    if p.is_file()
                },
            )

    def test_recovery_preserves_attempts_without_marking_fully_green(self):
        code, report, files = self.simulate(
            [(1, "ℹ fail 1"), (0, "ℹ fail 0"), (0, "passed")], 2
        )
        self.assertEqual(code, 0)
        self.assertTrue(report["success"])
        self.assertFalse(report["fully_green"])
        self.assertEqual(
            [s["status"] for s in report["stages"]], ["passed_after_retry", "passed"]
        )
        self.assertEqual([s["exit_code"] for s in report["stages"][0]["steps"]], [1, 0])
        self.assertEqual(files["00.log"], "ℹ fail 1")
        self.assertEqual(files["00-retry1.log"], "ℹ fail 0")
        self.assertEqual(json.loads(files["ctrf-0-0-0.json"])["attempt_marker"], 0)
        self.assertEqual(json.loads(files["ctrf-0-1-0.json"])["attempt_marker"], 1)
        code, report, _ = self.simulate([(0, "passed"), (0, "passed")], 2)
        self.assertEqual(code, 0)
        self.assertTrue(report["fully_green"])

    def test_only_confirmed_test_failures_retry_within_the_bound(self):
        for output, exit_code, retries, attempts in (
            ("error[E0308]: mismatched types", 1, 2, 1),
            ("error[E0308]: mismatched types", 105, 2, 1),
            ("test result: FAILED; 1 failed", 124, 2, 1),
            ("ℹ fail 1", 1, 0, 1),
            ("ℹ fail 1", 1, 2, 3),
            ("test result: FAILED; 1 failed", 101, 1, 2),
            (
                (
                    "test result: FAILED; 4 passed; 1 failed;\n"
                    "Error while executing command, exit code: 101"
                ),
                105,
                2,
                3,
            ),
        ):
            with self.subTest(output=output, exit_code=exit_code, retries=retries):
                code, report, _ = self.simulate(
                    [(exit_code, output)] * attempts, retries
                )
                self.assertEqual(code, 1)
                self.assertFalse(report["success"])
                self.assertFalse(report["fully_green"])
                self.assertEqual(
                    [s["status"] for s in report["stages"]], ["failed", "blocked"]
                )
                self.assertEqual(len(report["stages"][0]["steps"]), attempts)

    def test_retry_uses_remaining_stage_budget_not_a_fresh_timeout(self):
        code, report, _ = self.simulate(
            [(1, "test result: FAILED; 1 failed"), (124, "timed out")],
            retries=2,
            elapsed_per_attempt=40,
            stage_timeout=1,
        )
        self.assertEqual(code, 1)
        self.assertEqual(
            [s["timeout_seconds"] for s in report["stages"][0]["steps"]], [60, 20]
        )
        self.assertEqual([s["status"] for s in report["stages"]], ["failed", "blocked"])


class OverlapGateTests(unittest.TestCase):
    def reports(self):
        stages = [
            {
                "id": name,
                "status": "passed",
                "steps": [{"log": "00.log", "attempt": 0, "seconds": seconds}],
            }
            for name, seconds in (
                (overlap.CLIPPY, 120),
                (overlap.SERVICES[0], 110),
                (overlap.SERVICES[1], 40),
            )
        ]
        report = {
            "source_sha": "same-source",
            "finished_at": 10,
            "success": True,
            "fully_green": True,
            "test_retries": 0,
            "cli_test_threads": 4,
            "stages": stages,
        }
        return copy.deepcopy(report), report

    def test_only_complete_clean_matched_warm_baseline_is_eligible(self):
        cold, warm = self.reports()
        expected = [s["id"] for s in warm["stages"]]
        cold["fully_green"] = False
        cold["stages"][0]["status"] = "passed_after_retry"
        self.assertTrue(overlap.complete_baseline(cold, warm, expected))
        for field, value in (
            ("fully_green", False),
            ("success", False),
            ("finished_at", None),
            ("test_retries", 1),
            ("cli_test_threads", 6),
            ("source_sha", "other"),
        ):
            with self.subTest(field=field):
                self.assertFalse(
                    overlap.complete_baseline(cold, {**warm, field: value}, expected)
                )
        for status in ("failed", "blocked", "running", "passed_after_retry"):
            changed = copy.deepcopy(warm)
            changed["stages"][1]["status"] = status
            self.assertFalse(overlap.complete_baseline(cold, changed, expected))
        for stages in (warm["stages"][:-1], [warm["stages"][0]] * 3):
            self.assertFalse(
                overlap.complete_baseline(cold, {**warm, "stages": stages}, expected)
            )

    def test_value_boundary_inventory_reconciliation_and_retry_evidence(self):
        cold, warm = self.reports()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "inventory.json").write_text(
                json.dumps({"stages": [{"id": s["id"]} for s in warm["stages"]]})
            )
            for cycle, report in (("cold", cold), ("warm", warm)):
                for stage in report["stages"]:
                    directory = root / cycle / stage["id"].replace("/", "--")
                    directory.mkdir(parents=True)
                    if stage["id"] == overlap.CLIPPY:
                        continue
                    (directory / "ctrf-0-0-0.json").write_text(
                        json.dumps(
                            {
                                "results": {
                                    "summary": {"tests": 1},
                                    "tests": [
                                        {"name": stage["id"], "status": "passed"}
                                    ],
                                }
                            }
                        )
                    )
                (root / cycle / "all-0.json").write_text(json.dumps(report))
            # An earlier failed attempt must not contaminate the recovered final inventory.
            stage = cold["stages"][1]
            stage["steps"].append({"log": "00-retry1.log", "attempt": 1, "seconds": 2})
            directory = root / "cold" / stage["id"].replace("/", "--")
            (directory / "ctrf-0-1-0.json").write_text(
                (directory / "ctrf-0-0-0.json").read_text()
            )
            (directory / "ctrf-0-0-0.json").write_text(
                json.dumps(
                    {
                        "results": {
                            "summary": {"tests": 1},
                            "tests": [{"name": "initial failure", "status": "failed"}],
                        }
                    }
                )
            )
            (root / "cold/all-0.json").write_text(json.dumps(cold))
            with (
                patch.object(overlap, "RESULTS", root),
                patch.dict(os.environ, {"GITHUB_OUTPUT": str(root / "output")}),
            ):
                overlap.gate()
                self.assertTrue(
                    json.loads((root / "overlap-gate.json").read_text())["eligible"]
                )
                warm["stages"][0]["steps"][0]["seconds"] = 119.99
                (root / "warm/all-0.json").write_text(json.dumps(warm))
                overlap.gate()
                self.assertFalse(
                    json.loads((root / "overlap-gate.json").read_text())["eligible"]
                )
                changed = (
                    root
                    / "warm"
                    / overlap.SERVICES[0].replace("/", "--")
                    / "ctrf-0-0-0.json"
                )
                changed.unlink()
                with self.assertRaisesRegex(ValueError, "inventory differs"):
                    overlap.gate()

    def test_two_equally_reduced_replays_do_not_validate_the_baseline(self):
        _, warm = self.reports()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "warm").mkdir()
            (root / "warm/all-0.json").write_text(json.dumps(warm))
            (root / "overlap-gate.json").write_text('{"eligible":true}')
            (root / "allocation-start").write_text(str(int(overlap.time.time())))
            for cycle in ("warm", "serial-slice", "overlap"):
                for stage in warm["stages"]:
                    directory = root / cycle
                    if stage["id"] == overlap.CLIPPY:
                        if cycle == "warm":
                            continue
                        directory /= "clippy-results"
                        stages = [dict(stage, cache_before={})]
                    else:
                        stages = warm["stages"][1:]
                    directory.mkdir(parents=True, exist_ok=True)
                    (directory / "all-0.json").write_text(
                        json.dumps({**warm, "stages": stages})
                    )
                    reports = directory / stage["id"].replace("/", "--")
                    reports.mkdir()
                    tests = [{"name": "shared-test", "status": "passed"}]
                    if cycle == "warm":
                        tests.append(
                            {"name": "missing-from-both-replays", "status": "passed"}
                        )
                    (reports / "ctrf-0-0-0.json").write_text(
                        json.dumps(
                            {
                                "results": {
                                    "summary": {"tests": len(tests)},
                                    "tests": tests,
                                }
                            }
                        )
                    )
            with (
                patch.object(overlap, "RESULTS", root),
                patch.object(
                    overlap.subprocess, "check_output", return_value="same-source"
                ),
                patch.object(overlap, "run_slice"),
                self.assertRaisesRegex(ValueError, "admitted warm test inventory"),
            ):
                overlap.slices(root / "clippy")


if __name__ == "__main__":
    unittest.main()
