#!/usr/bin/env python3

import importlib.util
import os
from pathlib import Path
import unittest


path = Path(__file__).with_name("select-cargo-test-executables.py")
spec = importlib.util.spec_from_file_location("selector", path)
selector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selector)


class SelectExecutables(unittest.TestCase):
    root = os.path.realpath(Path(__file__).parents[3])

    def artifact(self, label, executable):
        name, kind, source = selector.TARGETS[label]
        return {
            "reason": "compiler-artifact",
            "profile": {"test": True},
            "target": {
                "name": name,
                "kind": kind,
                "src_path": os.path.join(self.root, source),
            },
            "executable": executable,
        }

    def test_selects_only_the_requested_library(self):
        messages = (self.artifact("integration", "integration"), self.artifact("lib", "lib"))
        self.assertEqual(selector.select_executables(messages, self.root, ("lib",)), {"lib": "lib"})

    def test_selects_both_managed_xfs_targets(self):
        messages = (self.artifact("integration", "integration"), self.artifact("lib", "lib"))
        self.assertEqual(
            selector.select_executables(messages, self.root, ("lib", "integration")),
            {"lib": "lib", "integration": "integration"},
        )

    def test_rejects_missing_and_multiple_artifacts(self):
        cases = ((), (self.artifact("lib", "first"), self.artifact("lib", "second")))
        for messages in cases:
            with self.subTest(messages=messages), self.assertRaisesRegex(ValueError, "exactly one lib"):
                selector.select_executables(messages, self.root, ("lib",))

    def test_ignores_duplicate_messages_for_the_same_artifact(self):
        artifact = self.artifact("lib", "lib")
        self.assertEqual(selector.select_executables((artifact, artifact), self.root, ("lib",)), {"lib": "lib"})

    def test_rejects_a_target_from_another_package(self):
        artifact = self.artifact("lib", "lib")
        artifact["target"]["src_path"] = os.path.join(self.root, "other-package", "src", "lib.rs")
        with self.assertRaisesRegex(ValueError, "exactly one lib"):
            selector.select_executables((artifact,), self.root, ("lib",))

    def test_requires_a_test_executable(self):
        artifact = self.artifact("lib", "lib")
        for field, value in (("profile", {"test": False}), ("executable", None)):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "exactly one lib"):
                selector.select_executables((dict(artifact, **{field: value}),), self.root, ("lib",))

    def test_rejects_invalid_requests(self):
        for requested in ((), ("lib", "lib"), ("unknown",)):
            with self.subTest(requested=requested), self.assertRaises(ValueError):
                selector.select_executables((), self.root, requested)


if __name__ == "__main__":
    unittest.main()
