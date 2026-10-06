"""Tests for the Cargo dependency visualizer."""

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("cargo-dependency-visualizer.py")
SPEC = importlib.util.spec_from_file_location("cargo_dependency_visualizer", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
VISUALIZER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VISUALIZER)


class SourceSizeTests(unittest.TestCase):
    def test_empty_rust_source_has_zero_lines(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "empty"\nversion = "0.1.0"\n'
            )
            (root / "src/lib.rs").write_bytes(b"")

            source_bytes, source_lines = VISUALIZER.source_size(
                {"manifest_path": str(root / "Cargo.toml")}
            )

            self.assertEqual(source_bytes, 0)
            self.assertEqual(source_lines, 0)

    def test_parent_package_does_not_claim_nested_package_source(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "parent"\nversion = "0.1.0"\n'
            )
            (root / "src/lib.rs").write_text("pub fn parent() {}\n")
            nested = root / "nested"
            (nested / "src").mkdir(parents=True)
            (nested / "Cargo.toml").write_text(
                '[package]\nname = "nested"\nversion = "0.1.0"\n'
            )
            (nested / "src/lib.rs").write_text("pub fn nested() {}\n")

            parent_bytes, parent_lines = VISUALIZER.source_size(
                {"manifest_path": str(root / "Cargo.toml")}
            )

            self.assertEqual(parent_bytes, len(b"pub fn parent() {}\n"))
            self.assertEqual(parent_lines, 1)


if __name__ == "__main__":
    unittest.main()
