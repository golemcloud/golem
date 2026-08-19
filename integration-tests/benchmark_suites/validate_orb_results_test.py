import copy
import unittest

from validate_orb_results import ValidationError, validate_results


COMMIT = "4358cac70a1dd11f186cb0f22f855a7a96e05cfc"
REF = "refs/heads/main"
RUNNER = "amp-orb-a1.xxlarge"


def valid_collection():
    benchmark = {
        "name": "latency-small",
        "runs": [{"clusterSize": 1, "size": 1, "length": 1}],
        "results": [
            {
                "run_config": {"clusterSize": 1, "size": 1, "length": 1},
                "duration_results": {
                    "invocation": {"avg": 1.0, "min": 0.9, "max": 1.1}
                },
            }
        ],
    }
    return {
        "runs": [
            {
                "suite": "CI",
                "runner": {"id": RUNNER},
                "source": {
                    "repository": "golemcloud/golem",
                    "commitSha": COMMIT,
                    "ref": REF,
                },
                "results": [copy.deepcopy(benchmark) for _ in range(11)],
            }
        ]
    }


class ValidateResultsTests(unittest.TestCase):
    def setUp(self):
        self.collection = valid_collection()
        for index, benchmark in enumerate(self.collection["runs"][0]["results"]):
            benchmark["name"] = f"benchmark-{index}"

    def validate(self):
        return validate_results(
            self.collection,
            expected_commit=COMMIT,
            expected_ref=REF,
            expected_runner=RUNNER,
            expected_benchmarks=11,
        )

    def test_accepts_a_complete_run(self):
        self.assertEqual(len(self.validate()["results"]), 11)

    def test_rejects_wrong_source(self):
        self.collection["runs"][0]["source"]["commitSha"] = "0" * 40
        with self.assertRaisesRegex(ValidationError, "commitSha"):
            self.validate()

    def test_rejects_missing_benchmarks(self):
        self.collection["runs"][0]["results"].pop()
        with self.assertRaisesRegex(ValidationError, "11 primary benchmarks"):
            self.validate()

    def test_rejects_partial_measurements(self):
        self.collection["runs"][0]["results"][0]["results"] = []
        with self.assertRaisesRegex(ValidationError, "non-empty array"):
            self.validate()

    def test_rejects_duplicate_benchmark_names(self):
        self.collection["runs"][0]["results"][1]["name"] = "benchmark-0"
        with self.assertRaisesRegex(ValidationError, "duplicate benchmark"):
            self.validate()


if __name__ == "__main__":
    unittest.main()
