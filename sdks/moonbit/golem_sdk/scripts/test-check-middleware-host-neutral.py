#!/usr/bin/env python3
"""Tests for the pure-middleware dependency-closure checker."""

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-middleware-host-neutral.py")
SPEC = importlib.util.spec_from_file_location("host_neutral", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
HOST_NEUTRAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HOST_NEUTRAL)


class DependencyClosureTest(unittest.TestCase):
    def test_rejects_transitive_tool_host_import_with_chain(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_package(
                root,
                "tool-core",
                'import { "golemcloud/golem_sdk/helper" }\n',
            )
            self.write_package(
                root,
                "helper",
                'import { "golemcloud/golem_sdk/interface/golem/tool/host" }\n',
            )
            self.write_package(root, "interface/golem/tool/host", "")

            with self.assertRaisesRegex(
                SystemExit,
                "(?s)tool-core.*helper.*interface/golem/tool/host",
            ):
                HOST_NEUTRAL.check_dependency_closure(
                    root, ("golemcloud/golem_sdk/tool-core",)
                )

    def test_rejects_unknown_external_import_with_chain(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_package(
                root,
                "tool-core",
                'import { "golemcloud/golem_sdk/helper" }\n',
            )
            self.write_package(
                root,
                "helper",
                'import { "third-party/ambient-tool-client" }\n',
            )

            with self.assertRaisesRegex(
                SystemExit,
                "(?s)unapproved external package.*tool-core.*helper.*third-party/ambient-tool-client",
            ):
                HOST_NEUTRAL.check_dependency_closure(
                    root, ("golemcloud/golem_sdk/tool-core",)
                )

    def test_accepts_an_explicitly_audited_external_import(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_package(
                root,
                "tool-core",
                'import { "audited/pure-helper" }\n',
            )

            closure = HOST_NEUTRAL.check_dependency_closure(
                root,
                ("golemcloud/golem_sdk/tool-core",),
                frozenset({"audited/pure-helper"}),
            )

            self.assertEqual(
                closure,
                {"golemcloud/golem_sdk/tool-core": None},
            )

    @staticmethod
    def write_package(root: Path, relative: str, descriptor: str) -> None:
        package = root / relative
        package.mkdir(parents=True)
        (package / "moon.pkg").write_text(descriptor)


if __name__ == "__main__":
    unittest.main()
