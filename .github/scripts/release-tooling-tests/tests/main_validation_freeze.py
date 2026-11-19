"""Real ELF fixtures for immutable cargo-test-r local-metadata snapshots."""

import hashlib
import json
import os
from pathlib import Path
import runpy
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[4]
HELPER = ROOT / ".github/main-validation/freeze-binaries.py"
# Include library harnesses and auxiliary test executables, not just integration.
IDENTITIES = (
    "golem-common", "golem-common::agent_extraction",
    "golem-common::component_introspection_exports", "golem-common::component_introspection_mems",
    "golem-common::component_introspection_metadata", "golem-common::poem_telemetry",
    "golem-worker-executor", "golem-worker-executor::file_creation_mask",
    "golem-worker-executor-tests::integration",
)


@unittest.skipUnless(sys.platform.startswith("linux"), "ldd/ELF fixtures require Linux")
class FreezeTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / ".amp/in/orb-backup/freeze-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.target = self.directory / "target"
        self.target.mkdir()
        self.destination = self.target / "main-validation/frozen-cycle"
        self.input = self.directory / "input.json"
        self.output = self.directory / "executor-binaries.json"
        self.manifest = self.output.with_name(self.output.name + ".manifest.json")
        self.env = {key: value for key, value in os.environ.items()
                    if key not in ("CARGO_TARGET_DIR", "LD_LIBRARY_PATH", "LD_PRELOAD")}

    def execute(self, *arguments):
        return subprocess.run([sys.executable, str(HELPER), *map(str, arguments)],
                              env=self.env, capture_output=True, text=True, timeout=60)

    def test_general_metadata_keeps_declared_tests_without_forcing_disabled_binary_tests(self):
        workspace = self.directory / "native"
        workspace.mkdir()
        (workspace / "Cargo.toml").write_text('''[package]
name = "native-selection"
version = "0.1.0"
edition = "2024"
[workspace]
[profile.dev-ci]
inherits = "dev"
incremental = false
[[bin]]
name = "server"
path = "server.rs"
test = false
[[bin]]
name = "binary-tests"
path = "binary.rs"
''')
        (workspace / "src").mkdir()
        (workspace / "tests").mkdir()
        (workspace / "src/lib.rs").write_text("#[test] fn library_test() {}")
        (workspace / "tests/declared.rs").write_text("#[test] fn integration_test() {}")
        (workspace / "server.rs").write_text(
            '#[cfg(test)] compile_error!("server has test = false");\nfn main() {}')
        (workspace / "binary.rs").write_text("fn main() {}\n#[test] fn binary_test() {}")
        script = tomllib.loads((ROOT / "Makefile.main-validation.toml").read_text())["tasks"][
            "main-validation-general-native"]["script"]
        commands = [shlex.split(line.split("exec --fail-on-error ", 1)[1])
                    for line in script.splitlines() if "exec --fail-on-error cargo " in line]
        environment = {**self.env, "CARGO_TARGET_DIR": str(workspace / "target")}
        for command in commands:
            result = subprocess.run(command, cwd=workspace, env=environment,
                                    capture_output=True, text=True, timeout=60)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        metadata = json.loads(result.stdout)
        binaries = list(metadata["rust-binaries"].values())
        self.assertEqual({entry["binary-name"] for entry in binaries},
                         {"native_selection", "declared", "binary-tests"})
        self.assertTrue((workspace / "target/dev-ci/server").is_file())
        self.assertTrue(all(Path(entry["binary-path"]).is_file() for entry in binaries))

    def freeze(self):
        return self.execute("--metadata", self.input, "--destination", self.destination,
                            "--output", self.output)

    def metadata(self, paths):
        result = {"rust-build-meta": {"target-directory": str(self.target),
                                     "build-directory": str(self.target),
                                     "linked-paths": ["dev-ci/build/native/out"],
                                     "non-test-binaries": {"package": [{"path": "dev-ci/worker-executor"}]},
                                     "platforms": {"host": {"triple": "x86_64-unknown-linux-gnu"}}},
                  "rust-binaries": {}, "extra": {"retained": [1, True, None]}}
        for identity, path in zip(IDENTITIES, paths):
            package, _, name = identity.partition("::")
            result["rust-binaries"][identity] = {
                "binary-id": identity, "binary-name": name or package.replace("-", "_"),
                "package-id": f"path+file://{ROOT / package}#0.0.0",
                "kind": "test" if name else "lib", "binary-path": str(path),
                "build-platform": "target", "extra": [identity, {"untouched": True}]}
        self.input.write_text(json.dumps(result))
        return result

    def system_binary(self, name="source/program"):
        path = self.target / name
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(shutil.which("true"), path)
        path.chmod(0o751)
        return path

    def assert_failed(self, result, message=None):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        if message:
            self.assertIn(message, result.stderr)
        self.assertFalse(self.output.exists())
        self.assertFalse(self.manifest.exists())

    def compile(self, output, source, *flags):
        output.parent.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(["cc", "-x", "c", "-", "-o", str(output), *map(str, flags)],
                                input=source, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        return output

    def library(self, directory):
        return self.compile(directory / "libfreeze_fixture.so", "int fixture(void) { return 0; }",
                            "-shared", "-fPIC")

    def linked_binary(self, library_directory, rpath):
        return self.compile(self.directory / "runtime/source/program",
                            "extern int fixture(void); int main(void) { return fixture(); }",
                            f"-L{library_directory}", "-lfreeze_fixture", f"-Wl,-rpath,{rpath}")

    def test_completed_cycles_can_reuse_the_report_directory(self):
        self.metadata([self.system_binary()])
        flow = tomllib.loads((ROOT / "Makefile.main-validation.toml").read_text())
        fixture = self.directory / "Makefile.toml"
        fixture.write_text('[config]\ndefault_to_workspace = false\nskip_core_tasks = true\n' +
                           '\n'.join('[tasks.' + task + ']\nscript_runner = "@duckscript"\nscript = \'\'\'\n' +
                                     flow['tasks'][task]['script'] + '\n\'\'\'\n' for task in
                                     ('main-validation-executor-native', 'main-validation-result')))
        tools = self.directory / "bin"
        tools.mkdir()
        cargo = tools / "cargo"
        cargo.write_text('#!/bin/sh\ncase "$1" in\nbuild) exit 0;;\nnextest) cat "' +
                         str(self.input) + '";;\n*) exit 91;;\nesac\n')
        cargo.chmod(0o755)
        report = self.directory / "report"
        report.mkdir()
        env = {key: value for key, value in self.env.items()
               if not key.startswith("CARGO_MAKE_")}
        env.update(PATH=str(tools) + os.pathsep + env["PATH"],
                   MAIN_VALIDATION_REPORT_DIR=str(report), CARGO_TARGET_DIR=str(self.target),
                   CARGO_MAKE_CRATE_TARGET_DIRECTORY=str(self.target), CI="false")
        snapshots = []
        for cycle in range(2):
            temporary = self.directory / f"cycle-{cycle}"
            temporary.mkdir()
            (report / "temporary-root.txt").write_text(str(temporary))
            for branch in ("worker", "service", "recovery", "cli", "exclusive", "checks"):
                (report / (branch + ".status")).write_text("0")
            for task in ("main-validation-executor-native", "main-validation-result"):
                result = subprocess.run([shutil.which("cargo-make"), "make", "--disable-check-for-updates",
                                         "--makefile", str(fixture), "--cwd", str(ROOT), task],
                                        env=env, capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                if task == "main-validation-executor-native":
                    metadata = json.loads((report / "executor-binaries.json").read_text())
                    copied = Path(next(iter(metadata["rust-binaries"].values()))["binary-path"])
                    self.assertEqual(copied.read_bytes(), self.system_binary().read_bytes())
                    snapshots.append(copied)
            self.assertFalse(snapshots[-1].exists())
        self.assertNotEqual(*snapshots)

    def test_all_nine_entries_collision_free_independent_bytes_and_permissions(self):
        sources = [self.system_binary(f"source-{index}/same-name") for index in range(9)]
        original = self.metadata(sources)
        result = self.freeze()
        self.assertEqual(result.returncode, 0, result.stderr)
        frozen = json.loads(self.output.read_text())
        manifest = json.loads(self.manifest.read_text())
        self.assertEqual(manifest["snapshot-directory"], str(self.destination))
        self.assertEqual(set(frozen["rust-binaries"]), set(IDENTITIES))
        copies = [Path(entry["binary-path"]) for entry in frozen["rust-binaries"].values()]
        self.assertEqual(len(set(copies)), 9)
        for identity, source, copied in zip(IDENTITIES, sources, copies):
            self.assertFalse(source.samefile(copied))
            self.assertFalse(copied.is_symlink())
            self.assertEqual(copied.read_bytes(), source.read_bytes())
            self.assertEqual(stat.S_IMODE(copied.stat().st_mode), 0o751)
            self.assertEqual(manifest["binaries"][identity]["sha256"],
                             hashlib.sha256(source.read_bytes()).hexdigest())
            self.assertIn(manifest["binaries"][identity]["sha256"], result.stdout)
            frozen["rust-binaries"][identity]["binary-path"] = str(source)
        self.assertEqual(frozen, original)
        # Exercise both in-place compiler writes and atomic link-output replacement.
        sources[0].write_bytes(b"overwritten during service build")
        replacement = self.directory / "replacement"
        replacement.write_bytes(b"replacement executable")
        replacement.replace(sources[1])
        sources[2].unlink()
        verified = self.execute("--verify", self.output)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertEqual(len([line for line in verified.stdout.splitlines() if '"verify"' in line]), 9)
        self.assertNotIn('"ldd"', verified.stdout)

    def test_missing_later_binary_does_not_publish_partial_metadata(self):
        self.metadata([self.system_binary(), self.target / "missing"])
        self.assert_failed(self.freeze(), "missing")
        self.assertTrue(list(self.destination.glob("0/*")))

    def test_empty_selection_and_malformed_binary_fields_fail_closed(self):
        self.metadata([])
        self.assert_failed(self.freeze(), "nonempty rust-binaries")
        source = self.system_binary()
        for field, value in (("binary-path", "relative"), ("binary-id", "different"),
                             ("package-id", None), ("kind", [])):
            with self.subTest(field=field):
                metadata = self.metadata([source])
                metadata["rust-binaries"][IDENTITIES[0]][field] = value
                self.input.write_text(json.dumps(metadata))
                self.assert_failed(self.freeze())

    def test_invalid_metadata_and_binaries_fail_closed(self):
        for text in ("", "{", "null", "[]", "{}", '{"rust-binaries": {}}',
                     '{"rust-binaries": {}, "rust-binaries": {}}'):
            with self.subTest(text=text):
                self.input.write_text(text)
                self.assert_failed(self.freeze())
        self.input.unlink()
        self.assert_failed(self.freeze())
        source = self.system_binary()
        truncated_elf = source.read_bytes()[:64]
        for contents, mode in ((b"", 0o755), (b"#!/bin/sh\nexit 0\n", 0o755),
                               (truncated_elf, 0o755), (b"text", 0o644)):
            with self.subTest(contents=contents, mode=mode):
                shutil.rmtree(self.destination, ignore_errors=True)
                source.write_bytes(contents)
                source.chmod(mode)
                self.metadata([source])
                self.assert_failed(self.freeze())

    def test_requires_fresh_destination_and_output(self):
        self.metadata([self.system_binary()])
        self.destination.mkdir(parents=True)
        self.assert_failed(self.freeze(), "destination already exists")
        self.destination.rmdir()
        self.output.write_text("existing success must not be overwritten")
        result = self.freeze()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.output.read_text(), "existing success must not be overwritten")

    def test_real_static_elf_is_accepted(self):
        source = self.compile(self.target / "static", "int main(void) { return 0; }", "-static")
        self.metadata([source])
        result = self.freeze()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue("not a dynamic executable" in result.stdout or "statically linked" in result.stdout)
        self.assertEqual(self.execute("--verify", self.output).returncode, 0)

    def test_unresolved_dependency_is_rejected_before_copy(self):
        library = self.library(self.directory / "runtime/lib")
        source = self.linked_binary(library.parent, str(library.parent))
        library.unlink()
        self.metadata([source])
        self.assert_failed(self.freeze(), "unresolved dependencies")

    def test_cargo_managed_dependency_is_rejected(self):
        library = self.library(self.target / "dev-ci/build/native/out")
        source = self.linked_binary(library.parent, str(library.parent))
        self.metadata([source])
        self.assert_failed(self.freeze(), "dependency under Cargo-managed target")

    def test_symlink_to_cargo_managed_library_is_rejected(self):
        library = self.library(self.target / "dev-ci/build/native/out")
        external = self.directory / "runtime/lib"
        external.mkdir(parents=True)
        (external / library.name).symlink_to(library)
        source = self.linked_binary(external, str(external))
        self.metadata([source])
        self.assert_failed(self.freeze(), "dependency under Cargo-managed target")

    def test_unchanged_external_shared_dependency_is_accepted(self):
        library = self.library(self.directory / "runtime/lib")
        source = self.linked_binary(library.parent, str(library.parent))
        self.metadata([source])
        result = self.freeze()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.count(str(library)), 4)
        self.assertEqual(self.execute("--verify", self.output).returncode, 0)

    def test_origin_dependency_missing_after_relocation_is_rejected(self):
        library = self.library(self.directory / "runtime/source")
        source = self.linked_binary(library.parent, "$ORIGIN")
        self.metadata([source])
        result = self.freeze()
        self.assert_failed(result, "unresolved dependencies")
        self.assertEqual(result.stdout.count('"ldd_exit"'), 2)

    def test_origin_resolution_to_different_library_is_rejected(self):
        library = self.library(self.directory / "runtime/source")
        fallback = self.library(self.directory / "runtime/fallback")
        source = self.linked_binary(library.parent, f"$ORIGIN:{fallback.parent}")
        self.metadata([source])
        self.assert_failed(self.freeze(), "dependency resolution changed")

    def test_publish_metadata_last_and_atomically(self):
        self.metadata([self.system_binary()])
        module = runpy.run_path(str(HELPER))
        replace = os.replace
        published = []

        def inspect_replace(source, destination):
            self.assertFalse(self.output.exists())
            self.assertEqual(Path(source).parent, Path(destination).parent)
            if Path(destination) == self.output:
                self.assertTrue(self.manifest.is_file())
                pending = json.loads(Path(source).read_text())
                module["check_snapshots"](pending, json.loads(self.manifest.read_text()))
            published.append(Path(destination))
            replace(source, destination)

        with patch.dict(os.environ, self.env, clear=True), patch("os.replace", inspect_replace):
            module["freeze"](self.input, self.destination, self.output)
        self.assertEqual(published, [self.manifest, self.output])

    def test_verify_detects_content_mode_metadata_and_manifest_tampering_without_ldd(self):
        self.metadata([self.system_binary()])
        result = self.freeze()
        self.assertEqual(result.returncode, 0, result.stderr)
        metadata_bytes = self.output.read_bytes()
        manifest_bytes = self.manifest.read_bytes()
        copied = Path(json.loads(metadata_bytes)["rust-binaries"][IDENTITIES[0]]["binary-path"])
        original_bytes = copied.read_bytes()
        # No ldd or compiler is available in verify mode. It must only read the snapshot.
        self.env["PATH"] = str(self.directory / "no-tools")
        self.assertEqual(self.execute("--verify", self.output).returncode, 0)
        for change in ("bytes", "mode", "metadata", "manifest", "missing", "symlink"):
            with self.subTest(change=change):
                if change == "bytes":
                    copied.write_bytes(b"tampered")
                elif change == "mode":
                    copied.chmod(0o700)
                elif change == "metadata":
                    self.output.write_bytes(metadata_bytes + b" ")
                elif change == "manifest":
                    manifest = json.loads(manifest_bytes)
                    manifest["binaries"] = {}
                    self.manifest.write_text(json.dumps(manifest))
                elif change == "missing":
                    copied.unlink()
                else:
                    copied.unlink()
                    copied.symlink_to(self.target / "source/program")
                verification = self.execute("--verify", self.output)
                self.assertNotEqual(verification.returncode, 0, verification.stdout)
                self.assertIn("freeze-binaries:", verification.stderr)
                if copied.is_symlink():
                    copied.unlink()
                copied.write_bytes(original_bytes)
                copied.chmod(0o751)
                self.output.write_bytes(metadata_bytes)
                self.manifest.write_bytes(manifest_bytes)
        self.manifest.unlink()
        self.assertNotEqual(self.execute("--verify", self.output).returncode, 0)


if __name__ == "__main__":
    unittest.main()
