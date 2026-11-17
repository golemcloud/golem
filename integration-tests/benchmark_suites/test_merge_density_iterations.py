"""Adversarial evidence-preservation checks for schema-2 density pooling."""

import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("merge_density_iterations.py")
SPEC = importlib.util.spec_from_file_location("density_pooling", SCRIPT)
POOLING = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POOLING)
SCRATCH = Path(__file__).resolve().parents[2] / ".amp/in/orb-backup"


def collection(index, failed=False):
    messages = ["timeout", "timeout"] if failed else []
    evidence = {
        "index": 0,
        "duration_samples": {} if failed else {"latency": [37.0, 11.0]},
        "count_samples": {"agents": [index]},
        "failures": {"invocation": messages} if failed else {},
    }
    failures = (
        {"invocation": {"count": 2, "samples": ["timeout"], "perIteration": [messages]}}
        if failed else {}
    )
    return {"runs": [{
        "schema_version": 2,
        "harness_version": 1,
        "suite": "density",
        "environment": "deployment",
        "version": "test-build",
        "timestamp": "2026-08-19T00:00:00Z",
        "results": [{
            "name": "density-agent-cell",
            "entry_version": 1,
            "iterations": 1,
            "series_key": "same-series",
            "description": "cell",
            "measurement_environment": {
                "runner_id": "runner", "hardware_fingerprint": "hardware",
                "storage_profile": "postgres", "sockets_enabled": False,
            },
            "runs": [{"clusterSize": 1, "size": 0, "length": 0,
                      "disableCompilationCache": False}],
            "results": [{
                "run_config": {"clusterSize": 1, "size": 0, "length": 0,
                               "disableCompilationCache": False},
                "iteration_results": [evidence],
                "count_results": {"agents": {
                    "avg": index, "min": index, "max": index,
                    "all": [index], "perIteration": [[index]],
                }},
                "duration_results": {} if failed else {"latency": {
                    "avg": 24.0, "min": 11.0, "max": 37.0,
                    "median": 24.0, "p90": 34.4, "p95": 35.7, "p99": 36.74,
                    "all": [37.0, 11.0], "perIteration": [[37.0, 11.0]],
                }},
                "failures": failures,
            }],
        }],
    }]}


class DensityPoolingAdversarialTests(unittest.TestCase):
    def pool(self, collections):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            root = Path(directory)
            inputs = []
            for index, value in enumerate(collections):
                path = root / f"iteration-{index}.json"
                path.write_text(json.dumps(value))
                inputs.append(str(path))
            output = root / "output.json"
            with patch("sys.argv", [str(SCRIPT), str(output), *inputs]):
                with contextlib.redirect_stderr(io.StringIO()):
                    POOLING.main()
            return json.loads(output.read_text())

    def test_failed_middle_iteration_retains_all_evidence_and_sample_order(self):
        inputs = [collection(7), collection(0, failed=True), collection(19)]
        result = self.pool(inputs)["runs"][0]["results"][0]
        self.assertEqual(result["iterations"], 3)
        run = result["results"][0]
        for index, original in enumerate(inputs):
            expected = copy.deepcopy(original["runs"][0]["results"][0]["results"][0]["iteration_results"][0])
            expected["index"] = index
            self.assertEqual(run["iteration_results"][index], expected)
        self.assertEqual(run["failures"]["invocation"]["count"], 2)
        self.assertEqual(run["failures"]["invocation"]["perIteration"], [["timeout", "timeout"]])
        self.assertEqual(run["count_results"]["agents"]["all"], [7, 0, 19])
        self.assertEqual(run["duration_results"]["latency"]["all"], [37.0, 11.0, 37.0, 11.0])

    def test_rejects_mismatched_entry_identity(self):
        for field, value in [("name", "other"), ("entry_version", 2), ("series_key", "different")]:
            with self.subTest(field=field):
                second = collection(2)
                second["runs"][0]["results"][0][field] = value
                with self.assertRaises(ValueError):
                    self.pool([collection(1), second])

    def test_rejects_mismatched_harness_or_schema(self):
        for field in ["schema_version", "harness_version"]:
            with self.subTest(field=field):
                second = collection(2)
                second["runs"][0][field] = 3
                with self.assertRaises(ValueError):
                    self.pool([collection(1), second])

    def test_rejects_different_cell_configuration(self):
        second = collection(2)
        second["runs"][0]["results"][0]["results"][0]["run_config"]["size"] = 100
        with self.assertRaises(ValueError):
            self.pool([collection(1), second])

    def test_failure_summary_is_distinct_and_capped_without_losing_evidence(self):
        inputs = []
        for index in range(8):
            item = collection(0, failed=True)
            result = item["runs"][0]["results"][0]["results"][0]
            message = f"failure-{index // 2}"
            if index > 3:
                message = f"failure-{index}"
            result["iteration_results"][0]["failures"]["invocation"] = [message, message]
            result["failures"]["invocation"]["samples"] = [message]
            result["failures"]["invocation"]["perIteration"] = [[message, message]]
            inputs.append(item)
        pooled = self.pool(inputs)["runs"][0]["results"][0]["results"][0]
        failure = pooled["failures"]["invocation"]
        self.assertEqual(failure["count"], 16)
        self.assertEqual(sum(map(len, failure["perIteration"])), 16)
        self.assertEqual(len(pooled["iteration_results"]), 8)
        self.assertEqual(failure["samples"], ["failure-0", "failure-1", "failure-4", "failure-5", "failure-6"])


if __name__ == "__main__":
    unittest.main()
