#!/usr/bin/env python3
"""Offline Lima launch checks and native cargo-test-r handoff regression tests."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = Path(__file__).with_name("run-lima.sh")


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux XDG cache placement")
class LimaCacheTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / ".amp/in/orb-backup/lima-cache-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "checkout with spaces"
        self.script = self.root / "integration-tests/scripts/managed-filesystem/run-lima.sh"
        self.script.parent.mkdir(parents=True)
        shutil.copy2(SCRIPT, self.script)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "calls.jsonl"
        self.images = self.root / "effective-images.yaml"
        self.images.write_text('- location: https://example.invalid/base.img\n  digest: sha256:aaa\n')
        stub = self.bin / "limactl"
        stub.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['CALL_LOG'], 'a') as log:
    log.write(json.dumps({'args': args, 'cache': os.environ.get('XDG_CACHE_HOME'),
                          'home': os.environ.get('LIMA_HOME')}) + '\\n')
if args[:2] == ['template', 'yq']:
    assert args[-1] == '.images'
    if os.environ.get('FAIL_TEMPLATE'):
        sys.exit(7)
    print(pathlib.Path(os.environ['EFFECTIVE_IMAGES']).read_text(), end='')
elif args[0] == 'list':
    if os.environ.get('EXISTING_VM'):
        print('golem-managed-xfs-v1')
elif args[0] == 'shell' and args[-2:] == ['printenv', 'HOME']:
    print('/home/guest')
elif args[0] not in ('start', 'shell'):
    sys.exit('unexpected invocation')
""")
        stub.chmod(0o755)
        self.env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}",
                        CALL_LOG=str(self.log), EFFECTIVE_IMAGES=str(self.images),
                        LIMA_HOME=str(self.root / "ephemeral-vms"),
                        XDG_CACHE_HOME=str(self.root / "unrelated-cache"),
                        GOLEM_MANAGED_XFS_RUNS="1")

    def run_lima(self, **environment):
        self.log.unlink(missing_ok=True)
        result = subprocess.run(["bash", str(self.script), "--test-argument"],
                                env=dict(self.env, **environment), capture_output=True, text=True)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, calls

    def test_reuses_image_directory_but_not_vm_home(self):
        first, calls = self.run_lima()
        self.assertEqual(first.returncode, 0, first.stderr)
        cache = Path(calls[1]["cache"])
        self.assertEqual(cache.parent, self.root / ".validation-cache/lima-images-v1")
        self.assertEqual(cache.name, hashlib.sha256(self.images.read_text().rstrip("\n").encode()).hexdigest())
        marker = cache / "retained-download"
        marker.write_text("native Lima owns the downloads here")
        second, repeated = self.run_lima()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(repeated[1]["cache"], str(cache))
        self.assertTrue(marker.exists())
        self.assertTrue(all(call["home"] == self.env["LIMA_HOME"] for call in calls + repeated))
        start = next(call["args"] for call in calls if call["args"][0] == "start")
        self.assertEqual(start, ["start", "--name", "golem-managed-xfs-v1", "--mount-only",
                                 f"{self.root}:w", str(self.root / "integration-tests/lima/managed-filesystem.yaml")])
        self.assertEqual(calls[-1]["args"][-1], "--test-argument")
        self.assertIn(str(self.root / "integration-tests/scripts/managed-filesystem/run-loopback-xfs.sh"),
                      calls[-1]["args"])

    def test_effective_inherited_url_and_digest_each_invalidate(self):
        _, baseline = self.run_lima()
        original = self.images.read_text()
        keys = {baseline[1]["cache"]}
        for images in (original.replace("base.img", "new.img"), original.replace("aaa", "bbb")):
            self.images.write_text(images)
            result, calls = self.run_lima()
            self.assertEqual(result.returncode, 0, result.stderr)
            keys.add(calls[1]["cache"])
        self.assertEqual(len(keys), 3)

    def test_template_resolution_failure_stops_before_vm_launch(self):
        result, calls = self.run_lima(FAIL_TEMPLATE="1")
        self.assertEqual(result.returncode, 7)
        self.assertEqual(len(calls), 1)
        self.assertFalse((self.root / ".validation-cache").exists())

    def test_existing_vm_uses_the_checkout_mount(self):
        result, calls = self.run_lima(EXISTING_VM="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(["start", "--mount-only", f"{self.root}:w", "golem-managed-xfs-v1"],
                      [call["args"] for call in calls])

    def test_forwards_native_snapshot_contract_without_splitting_paths(self):
        (self.root / "report").mkdir()
        values = {
            "GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES": "1",
            "GOLEM_MANAGED_XFS_SELECTION": str(self.root / "host snapshot/selection.json"),
            "GOLEM_MANAGED_XFS_SELECTION_DIGEST": "sha256:" + "a" * 64,
            "GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS": str(self.root / "report/executable checksums"),
        }
        result, calls = self.run_lima(**values)
        self.assertEqual(result.returncode, 0, result.stderr)
        start = next(call["args"] for call in calls if call["args"][0] == "start")
        self.assertEqual(start.count("--mount-only"), 1)
        for name, value in values.items():
            self.assertIn(f"{name}={value}", calls[-1]["args"])
        self.assertFalse(any(arg.startswith("GOLEM_MANAGED_XFS_BINARIES_METADATA=")
                             for arg in calls[-1]["args"]))


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux glibc snapshot contract")
class NativeHandoffTests(unittest.TestCase):
    def setUp(self):
        self.cli = shutil.which(os.environ.get("GOLEM_MANAGED_XFS_CARGO_TEST_R", "cargo-test-r"))
        if not self.cli or not shutil.which("cargo") or not shutil.which("cc"):
            self.skipTest("requires cargo-test-r, cargo and cc")
        scratch = ROOT / ".amp/in/orb-backup/managed-xfs-native-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.work = self.root / "guest work"
        self.work.mkdir(mode=0o755)
        self.source = SCRIPT.with_name("run-loopback-xfs.sh").read_text()
        # Load production functions without mounting XFS or running the suite.
        self.functions = self.source.split('if [[ ${1:-} == --self-test-initial-file-wasm ]]; then')[0]
        self.functions += "\nbuild_test_binaries() {" + self.source.split("build_test_binaries() {", 1)[1].split(
            "run_filesystem_benchmark() {", 1)[0]
        self.env = dict(os.environ, CLI=self.cli, WORK=str(self.work), ROOT=str(self.root),
                        CARGO_TARGET_DIR=str(self.root / "target"))

    def command(self, args, **kwargs):
        return subprocess.run(args, cwd=self.root, env=self.env, capture_output=True,
                              text=True, timeout=120, **kwargs)

    def success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def shell(self, body, root=False):
        prefix = ["sudo", "-n", "env"] if root else []
        keys = ("CLI", "WORK", "ROOT", "GOLEM_MANAGED_XFS_SELECTION",
                "GOLEM_MANAGED_XFS_SELECTION_DIGEST", "GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS")
        assignments = [f"{key}={self.env[key]}" for key in keys] if root else []
        return self.command(prefix + assignments + ["bash", "-c", self.functions + '''
work_dir=$WORK
repo_root=$ROOT
cargo_test_r=$CLI
reuse_test_binaries=1
privileged_test_sequence=0
''' + body])

    def make_host_snapshot(self):
        target = self.root / "target"
        target.mkdir()
        library = self.root / "guest-library.so"
        c = self.root / "library.c"
        c.write_text("int fixture(void) { return 0; }\n")
        self.success(self.command(["cc", "-shared", "-fPIC", str(c), "-o", str(library)]))
        c = self.root / "fixture.c"
        c.write_text('#include <stdio.h>\n#include <stdlib.h>\n#include <unistd.h>\n'
                     'extern int fixture(void);\nint main(void) { '
                     'char *expected = getenv("EXPECT_UID"); '
                     'if (expected && getuid() != atoi(expected)) return 9; '
                     'puts("Running 1 tests"); puts("test result: ok; 1 passed; 0 failed"); '
                     'return fixture(); }\n')
        executable = target / "fixture"
        self.success(self.command(["cc", str(c), str(library), "-o", str(executable)]))
        metadata = self.root / "binaries.json"
        metadata.write_text(json.dumps({"rust-build-meta": {
            "target-directory": str(target), "base-output-directories": ["debug"],
            "non-test-binaries": {}, "linked-paths": []}, "rust-binaries": {
                "golem-worker-executor-tests::integration": {
                    "binary-id": "golem-worker-executor-tests::integration",
                    "binary-name": "integration", "kind": "test",
                    "package-id": f"path+{self.root.as_uri()}#golem-worker-executor-tests@0.1.0",
                    "binary-path": str(executable), "build-platform": "target"}}}))
        selection = self.root / "selection.json"
        selection.write_text(self.success(self.command([
            self.cli, "list", "--binaries-metadata", str(metadata)])))
        host = self.root / "host snapshot"
        self.success(self.command([self.cli, "snapshot", "--selection", str(selection),
                                   "--out", str(host), "--deadline", "10m", "--kill-grace", "10s"]))
        self.host_selection = host / "selection.json"
        self.checksums = self.root / "executables.sha256"
        self.checksums.write_text("".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n"
            for path in sorted((host / "bin").iterdir())))
        self.env.update(GOLEM_MANAGED_XFS_SELECTION=str(self.host_selection),
                        GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS=str(self.checksums))
        self.update_digest()
        return library

    def update_digest(self):
        self.env["GOLEM_MANAGED_XFS_SELECTION_DIGEST"] = (
            "sha256:" + hashlib.sha256(self.host_selection.read_bytes()).hexdigest())

    def test_external_reports_follow_production_mounts(self):
        if not all(shutil.which(tool) for tool in ("sudo", "unshare", "mount", "setpriv")):
            self.skipTest("requires mount namespaces and passwordless sudo")
        probe = self.command(["sudo", "-n", "unshare", "--mount", "true"])
        if probe.returncode:
            self.skipTest("mount namespaces unavailable: " + probe.stderr.strip())
        launch = LimaCacheTests()
        launch.setUp()
        self.addCleanup(launch.doCleanups)
        self.root = launch.root
        self.env.update(ROOT=str(self.root), CARGO_TARGET_DIR=str(self.root / "target"))
        self.make_host_snapshot()
        reports = self.root.parent / "runner temp/main-validation-reports"
        reports.mkdir(parents=True)
        metadata = reports / "executor-produced-binaries.json"
        shutil.move(self.root / "binaries.json", metadata)
        selection = self.root / "produced-selection.json"
        selection.write_text(self.success(self.command([
            self.cli, "list", "--binaries-metadata", str(metadata)])))
        host = self.root / "production snapshot"
        self.success(self.command([self.cli, "snapshot", "--selection", str(selection), "--out", str(host)]))
        self.host_selection = host / "selection.json"
        self.checksums = reports / "executor-executables.sha256"
        self.checksums.write_text("".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n"
            for path in sorted((host / "bin").iterdir())))
        self.env.update(GOLEM_MANAGED_XFS_SELECTION=str(self.host_selection),
                        GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS=str(self.checksums))
        self.update_digest()
        empty, backing = self.root.parent / "empty", self.root.parent / "report backing"
        empty.mkdir()
        backing.mkdir()
        boundary = self.root / "guest-boundary.sh"
        boundary.write_text('''#!/usr/bin/env bash
set -euo pipefail
mount --make-rprivate /
mount --bind "$REPORT" "$BACKING"
mount --bind "$EMPTY" "$REPORT"
for specification in "$@"; do
  if [[ ${specification%:w} == "$REPORT" ]]; then
    mount --bind "$BACKING" "$REPORT"
    if [[ $specification != *:w ]]; then
      mount -o remount,bind,ro "$REPORT"
    fi
  fi
done
exec setpriv --reuid "$TEST_UID" --regid "$TEST_GID" --clear-groups bash "$GUEST"
''')
        guest = self.root / "guest-preparation.sh"
        guest.write_text(self.functions + '\nwork_dir=$WORK\ncargo_test_r=$CLI\n'
                         'trap \'rm -rf "$WORK/guest-snapshot"\' EXIT\nprepare_guest_snapshot\n')
        self.env.update(REPORT=str(reports), BACKING=str(backing), EMPTY=str(empty), GUEST=str(guest),
                        TEST_UID=str(os.getuid()), TEST_GID=str(os.getgid()))

        def prepare(mounts, checksums=None):
            keys = ("CLI", "WORK", "REPORT", "BACKING", "EMPTY", "GUEST", "TEST_UID", "TEST_GID",
                    "GOLEM_MANAGED_XFS_SELECTION", "GOLEM_MANAGED_XFS_SELECTION_DIGEST",
                    "GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS")
            env = dict(self.env, GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS=str(checksums or self.checksums))
            return self.command(["sudo", "-n", "env", *[f"{key}={env[key]}" for key in keys],
                                 "unshare", "--mount", "bash", str(boundary), *mounts])

        missing = prepare([f"{self.root}:w"])
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("executor-executables.sha256", missing.stderr)
        self.assertIn("No such file", missing.stderr)
        staged_checksums = self.root / "staged-checksums"
        shutil.copy2(self.checksums, staged_checksums)
        missing = prepare([f"{self.root}:w"], staged_checksums)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("executor-produced-binaries.json: No such file", missing.stderr)
        readonly = prepare([f"{self.root}:w", str(reports)])
        self.assertNotEqual(readonly.returncode, 0)
        self.assertIn("Read-only file system", readonly.stderr)

        values = {key: value for key, value in self.env.items() if key.startswith("GOLEM_MANAGED_XFS_")}
        values["GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES"] = "1"
        alias = self.root / "report-link"
        alias.symlink_to(reports, target_is_directory=True)
        for checksums in (self.checksums, self.root / ".." / self.checksums.relative_to(self.root.parent),
                          alias / self.checksums.name):
            values["GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS"] = str(checksums)
            for existing in ("", "1"):
                result, calls = launch.run_lima(**values, EXISTING_VM=existing)
                self.success(result)
                start = next(call["args"] for call in calls if call["args"][0] == "start")
                mounts = [start[index + 1] for index, arg in enumerate(start) if arg == "--mount-only"]
                self.success(prepare(mounts, checksums))
        evidence = list(reports.glob("guest-snapshot-evidence.*"))
        self.assertEqual(len(evidence), 6)
        for path in evidence:
            self.assertEqual((path / "selection.digest").read_text().strip(),
                             "sha256:" + hashlib.sha256((path / "selection.json").read_bytes()).hexdigest())

    def test_guest_reattests_libraries_and_pins_run_and_evidence(self):
        library = self.make_host_snapshot()
        doc = json.loads(self.host_selection.read_text())
        for dependency in doc["binaries"][0]["dependencies"]:
            dependency["path"] = "/nonexistent-host-library/" + Path(dependency["path"]).name
            dependency["sha256"] = "0" * 64
        self.host_selection.write_text(json.dumps(doc))
        self.update_digest()
        self.assertNotEqual(self.command([self.cli, "verify", "--selection",
                                         str(self.host_selection)]).returncode, 0)
        self.success(self.shell('prepare_guest_snapshot\nrun_selected_test privileged integration "" fixture'))
        guest = self.work / "guest-snapshot/selection.json"
        guest_doc = json.loads(guest.read_text())
        dependencies = guest_doc["binaries"][0]["dependencies"]
        self.assertIn(str(library), [item["path"] for item in dependencies])
        for dependency in dependencies:
            self.assertEqual(dependency["sha256"], hashlib.sha256(Path(dependency["path"]).read_bytes()).hexdigest())
        evidence = list(self.root.glob("guest-snapshot-evidence.*"))
        self.assertEqual(len(evidence), 1)
        self.assertEqual((evidence[0] / "selection.json").read_bytes(), guest.read_bytes())
        digest = "sha256:" + hashlib.sha256(guest.read_bytes()).hexdigest()
        self.assertEqual((evidence[0] / "selection.digest").read_text().strip(), digest)
        result = self.shell('''
guest_selection="$WORK/guest-snapshot/selection.json"
guest_selection_digest=sha256:wrong
run_selected_test privileged integration "" fixture
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("digest mismatch", result.stdout + result.stderr)
        library.write_bytes(library.read_bytes() + b"changed guest library")
        result = self.command([self.cli, "verify", "--selection", str(guest), "--expect-digest", digest])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Library sha256 mismatch", result.stderr)

    def test_transfer_corruption_and_manifest_digest_fail_before_snapshot(self):
        self.make_host_snapshot()
        executable = next((self.host_selection.parent / "bin").iterdir())
        executable.write_bytes(executable.read_bytes() + b"transfer corruption")
        result = self.shell("prepare_guest_snapshot")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED", result.stdout)
        self.assertFalse((self.work / "guest-snapshot").exists())
        self.host_selection.write_bytes(self.host_selection.read_bytes() + b"\n")
        result = self.shell("prepare_guest_snapshot")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("selection.json: FAILED", result.stdout)

    def test_transfer_corruption_during_snapshot_is_rejected(self):
        self.make_host_snapshot()
        wrapper = self.root / "snapshot-wrapper"
        wrapper.write_text('''#!/usr/bin/env bash
set -euo pipefail
"$CLI" "$@"
if [[ $1 == snapshot ]]; then
  read -r digest executable < "$GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS"
  printf corruption >> "$executable"
fi
''')
        wrapper.chmod(0o755)
        self.env["WRAPPER"] = str(wrapper)
        result = self.shell('cargo_test_r=$WRAPPER\nprepare_guest_snapshot')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED", result.stdout)
        self.assertFalse(list(self.root.glob("guest-snapshot-evidence.*")))

    def test_reuse_requires_all_three_inputs(self):
        self.make_host_snapshot()
        for name in ("GOLEM_MANAGED_XFS_SELECTION", "GOLEM_MANAGED_XFS_SELECTION_DIGEST",
                     "GOLEM_MANAGED_XFS_EXECUTABLE_CHECKSUMS"):
            result = self.shell(f"unset {name}\nprepare_guest_snapshot")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("reuse requires " + name, result.stderr)

    def test_root_snapshot_is_runnable_after_privilege_drop(self):
        if os.getuid() == 0 or not shutil.which("sudo"):
            self.skipTest("requires a nonroot user with passwordless sudo")
        if self.command(["sudo", "-n", "true"]).returncode:
            self.skipTest("requires passwordless sudo")
        self.make_host_snapshot()
        self.success(self.shell('''
trap 'rm -rf "$work_dir/guest-snapshot" "$ROOT"/guest-snapshot-evidence.*' EXIT
prepare_guest_snapshot
SUDO_UID=$(stat -c %u "$ROOT")
SUDO_GID=$(stat -c %g "$ROOT")
export EXPECT_UID=$SUDO_UID
run_selected_test unprivileged integration "" fixture
''', root=True))

    def test_real_cargo_selection_rejects_stale_decoys_and_ambiguity(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["executor", "tests"]\nresolver="3"\n'
            '[profile.benchmarks]\ninherits="dev"\n')
        for directory, package in (("executor", "golem-worker-executor"),
                                   ("tests", "golem-worker-executor-tests")):
            path = self.root / directory
            (path / "src").mkdir(parents=True)
            (path / "tests").mkdir()
            (path / "Cargo.toml").write_text(
                f'[package]\nname="{package}"\nversion="0.1.0"\nedition="2024"\n'
                '[features]\nextra=[]\n')
            (path / "src/lib.rs").write_text("#[test] fn fixture() {}\n")
            (path / "tests/integration.rs").write_text("#[test] fn fixture() {}\n")
        for benchmark in ("false", "true"):
            result = self.shell(f'''
run_vm_cargo() {{ "$@"; }}
target_dir=$CARGO_TARGET_DIR
filesystem_benchmark={benchmark}
filesystem_workload_benchmark=false
filesystem_native_workload_baseline=false
reuse_test_binaries=0
build_test_binaries
test -x "$lib_test_binary"
test -x "$integration_test_binary"
''')
            self.success(result)
            self.assertFalse(list((self.root / "target").glob("**/*stale-decoy")))
        messages = self.work / "cargo-test-artifacts.jsonl"
        artifacts = [json.loads(line) for line in messages.read_text().splitlines()]
        artifact = next(item for item in artifacts if item.get("executable")
                        and item["target"]["name"] == "golem_worker_executor")
        original = messages.read_text()
        for key, value in (("features", ["extra"]), ("target", dict(artifact["target"],
                                                    src_path=str(self.root / "wrong.rs")))):
            changed = dict(artifact, **{key: value})
            messages.write_text(original + json.dumps(changed) + "\n")
            result = self.command([self.cli, "list", "--cargo-messages", str(messages),
                                   "--cargo-metadata", str(self.work / "cargo-metadata.json"),
                                   "--binary-id", "golem-worker-executor", "--expect-count", "1",
                                   "--output", "executable-path"])
            self.assertNotEqual(result.returncode, 0, result.stdout)


if __name__ == "__main__":
    unittest.main()
