"""Source-based fixture reuse across languages, with real Git and shell hooks."""

import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[4]
HELPER = ROOT / ".github/main-validation/fixture-cache.py"
SPEC = importlib.util.spec_from_file_location("fixture_cache", HELPER)
CACHE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CACHE)


class FixtureCacheTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / ".amp/in/orb-backup/fixture-cache-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cache = self.root / "cache/completed"
        self.pending = self.cache.parent / "pending"
        self.pending.mkdir(parents=True)
        self.write("Cargo.toml", '[workspace]\n[workspace.dependencies]\n')
        self.write("cli/golem-cli/Cargo.toml", '[package]\nname="fixture-cli"\nversion="1.0.0"\n')
        self.write("cli/golem-cli/src/lib.rs", "cli")
        self.write("sdks/anything/runtime", "sdk")
        self.write("test-components/example/golem.yaml", "app: example\n")
        self.write("test-components/other/source", "unrelated")
        self.write("test-components/shared.yaml", "shared recipe")
        self.write("wit/interface.wit", "interface")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.track()

    def write(self, path, contents):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(contents.encode() if isinstance(contents, str) else contents)
        return target

    def track(self):
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)

    def key(self, app="example", variant="release"):
        return CACHE.fingerprint(self.root, app, variant)

    def bundle(self):
        key = self.key()
        before = CACHE.stamps(self.root)
        self.write("test-components/first.wasm", b"first compiled output")
        second = self.write("test-components/second.wasm", b"second compiled output")
        second.chmod(0o751)
        CACHE.stage(self.root, self.pending, key, before)
        return key

    def test_arbitrary_multifile_bundle_restores_only_exports_in_fresh_checkout(self):
        key = self.bundle()
        self.assertFalse(self.cache.exists())
        CACHE.publish(self.pending, self.cache)
        for path in (self.root / "test-components").glob("*.wasm"):
            path.unlink()
        CACHE.restore(self.root, self.cache / key, key)
        self.assertEqual((self.root / "test-components/first.wasm").read_bytes(), b"first compiled output")
        second = self.root / "test-components/second.wasm"
        self.assertEqual(second.read_bytes(), b"second compiled output")
        self.assertEqual(second.stat().st_mode & 0o777, 0o751)
        self.assertEqual(set(CACHE.verify(self.cache / key, key)), {"first.wasm", "second.wasm"})

    def test_eviction_keeps_current_restores_and_builds_in_an_ordinary_directory(self):
        restored = self.bundle()
        self.write("test-components/example/golem.yaml", "obsolete source")
        for output in (self.root / "test-components").glob("*.wasm"):
            output.unlink()
        obsolete = self.bundle()
        CACHE.publish(self.pending, self.cache)
        shutil.rmtree(self.pending)
        self.pending.mkdir()
        CACHE.restore(self.root, self.cache / restored, restored)
        (self.pending / "restored.json").write_text(json.dumps({"key": restored}))
        self.write("test-components/example/golem.yaml", "new source")
        for output in (self.root / "test-components").glob("*.wasm"):
            output.unlink()
        built = self.bundle()
        CACHE.publish(self.pending, self.cache)
        self.assertEqual({path.name for path in self.cache.iterdir()}, {restored, built})
        self.assertFalse((self.cache / obsolete).exists())
        CACHE.restore(self.root, self.cache / built, built)

        # A hit-only cycle has receipts but no staged bundles.
        shutil.rmtree(self.pending)
        self.pending.mkdir()
        (self.pending / "restored.json").write_text(json.dumps({"key": restored}))
        CACHE.publish(self.pending, self.cache)
        self.assertEqual({path.name for path in self.cache.iterdir()}, {restored})
        CACHE.restore(self.root, self.cache / restored, restored)

    def test_failed_publication_does_not_evict_previous_bundles(self):
        previous = self.bundle()
        CACHE.publish(self.pending, self.cache)
        shutil.rmtree(self.pending)
        self.pending.mkdir()
        self.write("test-components/example/golem.yaml", "new source")
        for output in (self.root / "test-components").glob("*.wasm"):
            output.unlink()
        broken = self.bundle()
        (self.pending / broken / "files/first.wasm").unlink()
        with self.assertRaises(ValueError):
            CACHE.publish(self.pending, self.cache)
        CACHE.restore(self.root, self.cache / previous, previous)
        self.assertEqual({path.name for path in self.cache.iterdir()}, {previous})

    def test_source_invalidation_is_per_application_and_language_independent(self):
        baseline = self.key()
        for path in ("sdks/anything/runtime", "cli/golem-cli/src/lib.rs", "wit/interface.wit",
                     "test-components/example/golem.yaml", "test-components/shared.yaml"):
            with self.subTest(path=path):
                file = self.root / path
                original = file.read_bytes()
                file.write_bytes(original + b"changed")
                self.assertNotEqual(baseline, self.key())
                file.write_bytes(original)
        self.write("test-components/other/source", "changed unrelated component")
        self.write("golem-worker-executor/src/lib.rs", "unrelated executor")
        self.write("integration-tests/test.rs", "unrelated tests")
        self.track()
        self.assertEqual(baseline, self.key())
        self.assertNotEqual(baseline, self.key(variant="quick"))
        self.assertNotEqual(self.key(variant="quick"), self.key(variant="optimized"))
        for language in ("rust", "ts", "effect", "scala", "moonbit", "go"):
            app = f"new-{language}-component"
            self.write(f"test-components/{app}/source", language)
            self.track()
            self.assertRegex(self.key(app), r"^[0-9a-f]{64}$")

    def test_added_deleted_and_mode_changed_source_invalidates(self):
        baseline = self.key()
        extra = self.write("test-components/example/new-file", "new")
        self.track()
        added = self.key()
        self.assertNotEqual(baseline, added)
        extra.chmod(0o755)
        self.assertNotEqual(added, self.key())
        extra.unlink()
        subprocess.run(["git", "-C", str(self.root), "rm", "--cached", str(extra)], check=True, capture_output=True)
        self.assertEqual(baseline, self.key())

    def test_cli_transitive_libraries_invalidate_but_independent_fixture_originals_do_not(self):
        self.write("cli/golem-cli/Cargo.toml", '[dependencies]\nshared={path="../../shared"}\n')
        self.write("shared/Cargo.toml", '[package]\nname="shared"\nversion="1.0.0"\n')
        shared = self.write("shared/src/lib.rs", "shared")
        self.write("test-components/example/golem.yaml", "dir: local-provider\n# copied from ../other\n")
        copied = self.write("test-components/example/local-provider/source", "local implementation")
        self.write("builtin-tools/example/source", "production implementation")
        self.track()
        baseline = self.key()
        shared.write_text("new shared CLI behavior")
        self.assertNotEqual(baseline, self.key())
        shared.write_text("shared")
        self.write("test-components/other/source", "changed sibling")
        self.write("builtin-tools/example/source", "changed production implementation")
        self.assertEqual(baseline, self.key())
        copied.write_text("changed local implementation")
        self.assertNotEqual(baseline, self.key())

    def test_stage_refuses_input_mutations_and_reports_each_path(self):
        helper = self.write(".github/main-validation/fixture-cache.py", HELPER.read_bytes())
        self.write("test-components/example/package-lock.json", "lockfile")
        self.track()
        environment = {**os.environ, "GOLEM_FIXTURE_CACHE": str(self.cache)}

        def run(operation):
            return subprocess.run([sys.executable, "-B", str(helper), operation, "example"],
                                  cwd=self.root, env=environment, capture_output=True, text=True, timeout=30)

        for paths in (("test-components/example/package-lock.json",),
                      ("sdks/anything/runtime", "wit/interface.wit", "cli/golem-cli/src/lib.rs")):
            with self.subTest(paths=paths):
                self.assertEqual(run("begin").returncode, 0)
                self.assertEqual(run("restore").returncode, 1)
                for path in paths:
                    self.write(path, "mutated input")
                self.write("test-components/export.wasm", "compiled output")
                result = run("stage")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("checked-in build inputs changed during compilation: " + ", ".join(sorted(paths)),
                              result.stdout)
                self.assertFalse(any(path.is_dir() for path in self.pending.iterdir()))

        # An unchanged input snapshot still stages the exported output.
        self.assertEqual(run("begin").returncode, 0)
        self.assertEqual(run("restore").returncode, 1)
        self.write("test-components/export.wasm", "new compiled output")
        result = run("stage")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("stage miss", result.stdout)
        CACHE.verify(self.pending / self.key(), self.key())

    def test_corrupt_incomplete_unsafe_and_symlink_bundles_are_misses_before_copy(self):
        key = self.bundle()
        bundle = self.pending / key
        original = (bundle / "COMPLETE.json").read_text()
        for defect in ("corrupt", "missing", "extra", "empty", "unsafe", "symlink", "malformed"):
            with self.subTest(defect=defect):
                shutil.rmtree(bundle)
                self.pending.mkdir(exist_ok=True)
                CACHE.stage(self.root, self.pending, key, {})
                marker, payload = bundle / "COMPLETE.json", bundle / "files"
                if defect == "corrupt":
                    (payload / "first.wasm").write_text("bad")
                elif defect == "missing":
                    (payload / "second.wasm").unlink()
                elif defect == "extra":
                    (payload / "extra.wasm").write_text("extra")
                elif defect == "empty":
                    marker.write_text(json.dumps({"schema": 1, "key": key, "files": {}}))
                elif defect == "unsafe":
                    manifest = json.loads(original)
                    manifest["files"] = {"../escape.wasm": {"size": 1}}
                    marker.write_text(json.dumps(manifest))
                elif defect == "symlink":
                    (payload / "first.wasm").unlink()
                    (payload / "first.wasm").symlink_to(self.root / "test-components/first.wasm")
                else:
                    marker.write_text("null")
                with self.assertRaises((ValueError, OSError)):
                    CACHE.restore(self.root, bundle, key)
                self.assertEqual((self.root / "test-components/first.wasm").read_bytes(), b"first compiled output")

    def test_empty_export_does_not_stage(self):
        key = self.key()
        with self.assertRaises(ValueError):
            CACHE.stage(self.root, self.pending, key, CACHE.stamps(self.root))
        self.write("test-components/empty.wasm", b"")
        with self.assertRaises(ValueError):
            CACHE.stage(self.root, self.pending, key, {})
        self.assertFalse((self.pending / key).exists())

    def test_source_links_hash_checked_in_targets_not_generated_build_trees(self):
        target = self.write("shared-source/implementation", "first")
        link = self.root / "test-components/example/source"
        link.symlink_to("../../shared-source/implementation")
        generated = self.root / "sdks/anything/target"
        generated.symlink_to("_build")
        self.track()
        baseline = self.key()
        target.write_text("second")
        self.assertNotEqual(baseline, self.key())
        target.write_text("first")
        self.write("sdks/anything/_build/generated", "compiler output")
        self.assertEqual(baseline, self.key())

    def test_full_shell_cold_warm_and_checks_for_every_language(self):
        script = (ROOT / "test-components/build-components.sh").read_text()
        applications = re.findall(r'^\w+_apps=\((.*)\)$', script, re.MULTILINE)
        for declaration in applications:
            for app in re.findall(r'"([^"]+)"', declaration):
                self.write(f"test-components/{app}/golem.yaml", f"app: {app}\n")
        for directory in ("tool-streaming/rust-provider", "tool-streaming/rust-caller",
                          "agent-rpc/golem-it-agent-rpc-rust", "size-analysis/fixtures"):
            self.write(f"test-components/{directory}/Cargo.toml", "[package]\n")
        self.write("test-components/build-components.sh", script).chmod(0o755)
        self.write(".github/main-validation/fixture-cache.py", HELPER.read_bytes())
        events = self.root / "events"
        cli = self.write("bin/golem", '''#!/bin/bash
set -eu
echo "${PWD##*/}:$*" >> "$EVENTS"
if [[ "$*" == *"--skip-check"* && "${FAIL_APP:-}" == "${PWD##*/}" ]]; then exit 7; fi
if [[ "$*" == *"--skip-check"* ]]; then
  mkdir -p "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release"
  for name in golem_it_tool_streaming_rust_provider golem_it_tool_streaming_rust_caller golem_it_agent_rpc_rust; do
    printf 'benchmark:%s' "$name" > "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release/$name.wasm"
  done
fi
if [[ "$*" == *"exec copy"* ]]; then
  printf 'compiled:%s' "${PWD##*/}" > "../${PWD##*/}.wasm"
  case "${PWD##*/}" in
    *scala*|*moonbit*) name="golem_it_${PWD##*/}"; printf compiled > "../${name//-/_}.wasm" ;;
  esac
fi
''')
        cli.chmod(0o755)
        self.write("bin/npm", "#!/bin/bash\nexit 0\n").chmod(0o755)
        self.write("bin/cargo", '''#!/bin/bash
set -eu
if [ "$1" = metadata ]; then printf '{"target_directory":"%s"}' "$CARGO_MAKE_CRATE_TARGET_DIRECTORY"; fi
if [ "$1" = build ]; then
  mkdir -p "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release"
  for name in retention_empty retention_non_reflective retention_reflection; do
    printf 'retention:%s' "$name" > "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release/$name.wasm"
  done
fi
''').chmod(0o755)
        self.track()
        environment = {**os.environ, "GOLEM_CLI": str(cli), "GOLEM_FIXTURE_CACHE": str(self.cache),
                       "CARGO_MAKE_CRATE_TARGET_DIRECTORY": str(self.root / "target"),
                       "CARGO_TARGET_DIR": "", "PATH": f"{self.root}/bin:{os.environ['PATH']}", "EVENTS": str(events)}

        def run(*args):
            result = subprocess.run(args, cwd=self.root, env=environment, capture_output=True, text=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result

        helper = [sys.executable, "-B", str(self.root / ".github/main-validation/fixture-cache.py")]
        run(*helper, "begin")
        run("bash", "test-components/build-components.sh", "rebuild")
        run("bash", "test-components/build-components.sh", "benchmarks", "rebuild")
        cold = {path.name: path.read_bytes() for path in (self.root / "test-components").glob("*.wasm")}
        self.assertGreater(len(cold), 30)
        for name in ("retention_empty", "retention_non_reflective", "retention_reflection"):
            self.assertEqual(cold[f"{name}.wasm"], f"retention:{name}".encode())
        run(*helper, "publish")
        for path in (self.root / "test-components").glob("*.wasm"):
            path.unlink()
        events.unlink()
        run(*helper, "begin")
        warm = run("bash", "test-components/build-components.sh", "rebuild")
        run("bash", "test-components/build-components.sh", "benchmarks", "rebuild")
        self.assertIn("Fixture cache hit:", warm.stdout)
        self.assertFalse(events.exists(), "a hit must invoke neither clean nor build")
        self.assertEqual(cold, {path.name: path.read_bytes() for path in (self.root / "test-components").glob("*.wasm")})
        run("bash", "test-components/build-components.sh", "check")
        run("bash", "test-components/build-components.sh", "benchmarks", "check")
        self.assertEqual(len(events.read_text().splitlines()), sum(len(re.findall(r'"([^"]+)"', line)) for line in applications) + 3)
        self.assertTrue(all("--step check" in line for line in events.read_text().splitlines()))
        # A changed checked-in component must rebuild; its failure cannot stage.
        self.write("test-components/agent-sdk-go/golem.yaml", "app: changed\n")
        environment["FAIL_APP"] = "agent-sdk-go"
        result = subprocess.run(["bash", "test-components/build-components.sh", "go", "rebuild"],
                                cwd=self.root, env=environment, capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 7, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
