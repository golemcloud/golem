import copy
import importlib.util
import unittest
from pathlib import Path

import yaml

SPEC = importlib.util.spec_from_file_location(
    "experiment", Path(__file__).with_name("ci-experiment.py")
)
experiment = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(experiment)


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


if __name__ == "__main__":
    unittest.main()
