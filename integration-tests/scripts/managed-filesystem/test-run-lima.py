#!/usr/bin/env python3
"""Offline Linux launch checks; the limactl stub never boots a VM."""

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

    def test_existing_vm_launch_contract_is_unchanged(self):
        result, calls = self.run_lima(EXISTING_VM="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(["start", "golem-managed-xfs-v1"], [call["args"] for call in calls])


if __name__ == "__main__":
    unittest.main()
