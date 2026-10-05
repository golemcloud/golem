"""Tests for the Cargo dependency feature audit."""

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("cargo-dependency-feature-audit.py")
SPEC = importlib.util.spec_from_file_location("cargo_dependency_feature_audit", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)


class FeatureAuditTests(unittest.TestCase):
    def test_reverse_dependent_count_counts_distinct_packages_with_same_name(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = self.metadata(root)
            configurable_id = metadata["packages"][1]["id"]
            for version in ("1.0.0", "2.0.0"):
                package_id = f"registry+https://example.invalid#index/consumer@{version}"
                metadata["packages"].append(
                    {
                        "id": package_id,
                        "name": "consumer",
                        "version": version,
                        "manifest_path": f"/registry/consumer-{version}/Cargo.toml",
                        "source": "registry+https://example.invalid/index",
                        "features": {},
                    }
                )
                metadata["resolve"]["nodes"].append(
                    {
                        "id": package_id,
                        "features": [],
                        "deps": [{"pkg": configurable_id}],
                    }
                )

            records, _ = AUDIT.build_feature_records(metadata)

            self.assertEqual(records[0]["reverse_dependent_count"], 3)

    def test_lists_only_third_party_packages_with_non_default_features(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = self.metadata(root)

            records, third_party_count = AUDIT.build_feature_records(metadata)

            self.assertEqual(third_party_count, 3)
            self.assertEqual([record["name"] for record in records], ["configurable"])
            self.assertEqual(records[0]["enabled"], ["serde"])
            self.assertEqual(records[0]["disabled"], ["logging"])
            self.assertTrue(records[0]["default_enabled"])
            self.assertEqual(records[0]["workspace_consumers"], ["app"])
            self.assertEqual(records[0]["reverse_dependent_count"], 1)

    def test_report_states_workspace_wide_scope_and_escapes_feature_names(self) -> None:
        report = AUDIT.render_report(
            [
                {
                    "name": "example",
                    "version": "1.0.0",
                    "source": "registry",
                    "default_enabled": False,
                    "enabled": ["a|b"],
                    "disabled": [],
                    "feature_count": 1,
                    "reverse_dependent_count": 2,
                    "workspace_consumers": [],
                }
            ],
            4,
            None,
        )

        self.assertIn("workspace-wide unified resolution", report)
        self.assertIn("Packages with at least one non-`default` feature: **1**", report)
        self.assertIn("`a&#124;b`", report)

    @staticmethod
    def metadata(root: Path) -> dict:
        app_id = "path+file:///workspace/app#0.1.0"
        configurable_id = "registry+https://example.invalid#index/configurable@1.0.0"
        default_only_id = "registry+https://example.invalid#index/default-only@1.0.0"
        featureless_id = "registry+https://example.invalid#index/featureless@1.0.0"
        return {
            "workspace_root": str(root),
            "packages": [
                {
                    "id": app_id,
                    "name": "app",
                    "version": "0.1.0",
                    "manifest_path": str(root / "app/Cargo.toml"),
                    "source": None,
                    "features": {"local": []},
                },
                {
                    "id": configurable_id,
                    "name": "configurable",
                    "version": "1.0.0",
                    "manifest_path": "/registry/configurable/Cargo.toml",
                    "source": "registry+https://example.invalid/index",
                    "features": {
                        "default": ["serde"],
                        "serde": ["dep:serde"],
                        "logging": [],
                    },
                },
                {
                    "id": default_only_id,
                    "name": "default-only",
                    "version": "1.0.0",
                    "manifest_path": "/registry/default-only/Cargo.toml",
                    "source": "registry+https://example.invalid/index",
                    "features": {"default": []},
                },
                {
                    "id": featureless_id,
                    "name": "featureless",
                    "version": "1.0.0",
                    "manifest_path": "/registry/featureless/Cargo.toml",
                    "source": "registry+https://example.invalid/index",
                    "features": {},
                },
            ],
            "resolve": {
                "nodes": [
                    {
                        "id": app_id,
                        "features": ["local"],
                        "deps": [{"pkg": configurable_id}],
                    },
                    {
                        "id": configurable_id,
                        "features": ["default", "serde"],
                        "deps": [],
                    },
                    {"id": default_only_id, "features": ["default"], "deps": []},
                    {"id": featureless_id, "features": [], "deps": []},
                ]
            },
        }


if __name__ == "__main__":
    unittest.main()
