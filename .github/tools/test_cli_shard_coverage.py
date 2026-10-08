import json
from pathlib import Path
import tempfile
import unittest

from check_cli_shard_coverage import latest_reports, validate


class ShardCoverageTests(unittest.TestCase):
    inventory = "Runner output\nintegration::a\nintegration::b\nintegration::ignored\n\n3 tests\n"

    def test_complete_disjoint_partition_including_ignored_tests(self):
        self.assertEqual(
            validate(
                self.inventory,
                {
                    "one": ["integration::b"],
                    "two": ["integration::a", "integration::ignored"],
                },
            ),
            3,
        )

    def test_rejects_missing_duplicate_unknown_and_empty_selections(self):
        cases = [
            {"one": ["integration::a", "integration::b"]},
            {
                "one": ["integration::a", "integration::b", "integration::ignored"],
                "two": ["integration::b"],
            },
            {
                "one": [
                    "integration::a",
                    "integration::a",
                    "integration::b",
                    "integration::ignored",
                ]
            },
            {
                "one": [
                    "integration::a",
                    "integration::b",
                    "integration::ignored",
                    "integration::unknown",
                ]
            },
            {
                "one": ["integration::a", "integration::b", "integration::ignored"],
                "two": [],
            },
            {},
        ]
        for reports in cases:
            with self.subTest(reports=reports), self.assertRaises(ValueError):
                validate(self.inventory, reports)

    def test_rejects_truncated_or_duplicate_inventory(self):
        for inventory in [
            "",
            "integration::a\n2 tests\n",
            "integration::a\nintegration::a\n2 tests\n",
        ]:
            with self.subTest(inventory=inventory), self.assertRaises(ValueError):
                validate(inventory, {"one": ["integration::a"]})

    def test_latest_attempt_per_shard_keeps_unrerun_shards_and_skipped_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, tests in [
                ("cli-integration-tests-one-report-attempt1", ["old"]),
                ("cli-integration-tests-one-report-attempt2", ["new"]),
                ("cli-integration-tests-two-report-attempt1", ["ignored"]),
                ("unit-tests-report-attempt2", ["not-cli"]),
            ]:
                path = root / name / "ctrf-report.json"
                path.parent.mkdir()
                path.write_text(
                    json.dumps(
                        {
                            "results": {
                                "tests": [
                                    {"name": test, "status": "skipped"}
                                    for test in tests
                                ]
                            }
                        }
                    )
                )
            self.assertEqual(
                latest_reports(root),
                {
                    "cli-integration-tests-one": ["new"],
                    "cli-integration-tests-two": ["ignored"],
                },
            )


if __name__ == "__main__":
    unittest.main()
