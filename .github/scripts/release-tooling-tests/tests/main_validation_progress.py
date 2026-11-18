"""Small process fixtures for validation logging; no Golem workload is executed."""

import contextlib
import gzip
import http.client
import io
import json
import os
from pathlib import Path
import runpy
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
from unittest.mock import patch
import urllib.error


ROOT = Path(__file__).resolve().parents[4]
HELPER = ROOT / ".github/main-validation/progress.py"
FLOW = ROOT / "Makefile.main-validation.toml"


class ProgressTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / ".amp/in/orb-backup/progress-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.report = self.directory / "evidence"
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("CARGO_MAKE_", "MAIN_VALIDATION_", "CI_DASHBOARD_", "TEST_R_"))}
        self.env.update(MAIN_VALIDATION_REPORT_DIR=str(self.report), CI="false",
                        CARGO_TARGET_DIR=str(self.directory / "target"),
                        GITHUB_STEP_SUMMARY=str(self.directory / "summary.md"))
        shutil.copy(FLOW, self.directory / FLOW.name)

    def execute(self, arguments, **kwargs):
        return subprocess.run([sys.executable, str(HELPER), *arguments], cwd=self.directory,
                              env=self.env, capture_output=True, timeout=180, **kwargs)

    def test_full_output_and_nonzero_exit_are_preserved(self):
        script = "import os,sys; os.write(1,b'first\\n'+b'x'*200000+b'\\n'); os.write(2,b'last\\n'); assert sys.argv[1]=='one argument'; sys.exit(7)"
        result = self.execute(["run", "worker", "misc", "--", sys.executable, "-c", script, "one argument"])
        self.assertEqual(result.returncode, 7, result.stderr)
        state = json.loads((self.report / "progress/worker--misc.json").read_text())
        self.assertEqual(state["status"], "failed")
        self.assertEqual(state["exit_code"], 7)
        self.assertEqual(gzip.decompress((self.report / state["log"]).read_bytes()),
                         b"first\n" + b"x" * 200000 + b"\nlast\n")
        self.assertLess(len(result.stdout), 2000)

    def test_dashboard_outage_does_not_change_a_successful_command(self):
        module = runpy.run_path(str(HELPER))
        identity = {"CI_DASHBOARD_URL": "https://example.invalid", "CI_DASHBOARD_PROGRESS_TOKEN": "fixture-token",
                    "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "GITHUB_SHA": "a" * 40,
                    "GITHUB_REF_NAME": "main", "GITHUB_EVENT_NAME": "workflow_dispatch",
                    "MAIN_VALIDATION_SOURCE": "b" * 40, "MAIN_VALIDATION_CONTEXT": "Main validation (acceptance)"}
        class TruncatedSocket:
            def makefile(self, *args):
                return io.BytesIO(b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n20\r\n{"status":')

        def truncated_reply(*args, **kwargs):
            response = http.client.HTTPResponse(TruncatedSocket())
            response.begin()
            return response

        for reply in (None, "truncated", b"[]", b"null", b'"not a confirmation"', b"invalid JSON"):
            with self.subTest(reply=reply), patch.dict(os.environ, {**self.env, **identity}), \
                    contextlib.chdir(self.directory), patch("urllib.request.build_opener") as opener:
                if reply is None:
                    opener.return_value.open.side_effect = urllib.error.URLError("fixture outage")
                elif reply == "truncated":
                    opener.return_value.open.side_effect = truncated_reply
                else:
                    response = opener.return_value.open.return_value.__enter__.return_value
                    response.status = 200
                    response.read.return_value = reply
                code = module["observe"](self.report, [sys.executable, "-c", "print('retained controller output')"])
                self.assertEqual(code, 0)
                self.assertGreater(opener.return_value.open.call_count, 0)
                self.assertIn(b"retained controller output", gzip.decompress((self.report / "logs/controller.log.gz").read_bytes()))

    def test_observer_can_read_ansi_job_logs_and_identify_selected_source(self):
        gh = self.directory / "gh"
        gh.write_text('''#!/usr/bin/env python3
import json, sys
args = sys.argv[1:]
path = next(value for value in args if value.startswith('repos/'))
if path.endswith('/jobs?per_page=100'):
    print(json.dumps([{'jobs': [{'id': index, 'name': name, 'conclusion': 'failure'} for index, name in enumerate(('select', 'result', 'linux'), 1)]}]))
elif path.endswith('/artifacts?per_page=100'):
    print('[{"artifacts": []}]')
elif path.endswith('/logs'):
    if '--allow-escape-sequences' not in args:
        sys.exit('the response contains terminal escape sequences')
    print('2026-11-17T08:00:00Z   SOURCE: ' + 'c'*40)
    print('2026-11-17T08:00:00Z   CONTEXT: Main validation (acceptance)')
    print('2026-11-17T08:00:00Z \\x1b[31mtest diagnostic\\x1b[0m')
else:
    print(json.dumps({'id': 123, 'run_attempt': 1, 'status': 'completed', 'conclusion': 'failure',
                      'head_sha': 'a'*40, 'head_branch': 'main', 'event': 'workflow_dispatch',
                      'created_at': '2026-11-17T08:00:00Z', 'updated_at': '2026-11-17T09:00:00Z',
                      'html_url': 'https://github.com/golemcloud/golem/actions/runs/123'}))
''')
        gh.chmod(0o755)
        environment = {**self.env, "OBSERVER_STATE_DIR": str(self.directory / "observer"),
                       "PATH": str(self.directory) + os.pathsep + self.env["PATH"]}
        result = subprocess.run(["bash", str(ROOT / ".github/main-validation/observer/run-facts.sh"), "123", "1"],
                                env=environment, capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        facts = json.loads(result.stdout)
        self.assertEqual(facts["source"], "c" * 40)
        self.assertEqual(facts["context"], "Main validation (acceptance)")
        self.assertTrue(facts["selected"])
        self.assertIn(b"\x1b[31mtest diagnostic", (self.directory / "observer/runs/123-a1/logs/result.log").read_bytes())

    def test_cancellation_forwards_to_the_workload_and_retains_partial_output(self):
        command = [sys.executable, str(HELPER), "observe", "--", sys.executable, str(HELPER),
                   "run", "worker", "misc", "--", sys.executable, "-u", "-c",
                   "import time; print('before interruption'); time.sleep(120)"]
        process = subprocess.Popen(command, cwd=self.directory, env=self.env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        deadline = time.monotonic() + 15
        partial = self.report / "logs/worker--misc.log"
        while not partial.exists() or b"before interruption" not in partial.read_bytes():
            self.assertLess(time.monotonic(), deadline, "workload failed to start")
            time.sleep(.05)
        state = json.loads((self.report / "progress/worker--misc.json").read_text())
        pid = state["pid"]
        process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=20)
        self.assertEqual(process.returncode, 143, (stdout, stderr))
        state = json.loads((self.report / "progress/worker--misc.json").read_text())
        self.assertEqual(state["status"], "interrupted")
        path = self.report / state["log"]
        retained = gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()
        self.assertIn(b"before interruption", retained)
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        tasks = json.loads((self.report / "progress.json").read_text())["tasks"]
        self.assertTrue(all(task["status"] in ("blocked", "interrupted") for task in tasks))
        self.assertTrue(all(task["completed_at"] for task in tasks))

    def prepare_cargo_fixture(self):
        helper_dir = self.directory / ".github/main-validation"
        helper_dir.mkdir(parents=True)
        shutil.copy(HELPER, helper_dir / HELPER.name)
        config = tomllib.loads(FLOW.read_text())
        lanes = runpy.run_path(str(HELPER))["LANES"]
        names = []
        for lane, owner in lanes.items():
            for task in config["tasks"][owner]["env"]["MAIN_VALIDATION_TASKS"].split():
                prefix = {"worker": "worker-executor-tests-", "service": "integration-tests-group",
                          "cli": "cli-integration-tests-"}.get(lane, "")
                names.append(prefix + task)
        makefile = '[config]\ndefault_to_workspace = false\nskip_core_tasks = true\nextend = []\n'
        makefile = 'extend = "Makefile.main-validation.toml"\n' + makefile
        for name in names:
            makefile += f'\n[tasks.{name}]\nclear = true\ncommand = "python3"\nargs = ["workload.py", "{name}"]\n'
        (self.directory / "Makefile.toml").write_text(makefile)
        (self.directory / "Cargo.toml").write_text('[workspace]\nmembers = []\n')
        bin_dir = self.directory / "bin"
        bin_dir.mkdir()
        runner = bin_dir / "cargo-test-r"
        runner.write_text('#!/bin/sh\necho --binaries-metadata\n')
        runner.chmod(0o755)
        self.env["PATH"] = str(bin_dir) + os.pathsep + self.env["PATH"]
        self.env["CI_DASHBOARD_PROGRESS_TOKEN"] = "fixture-credential-never-forwarded"
        (self.directory / "workload.py").write_text('''import json, os, subprocess, sys, tempfile, time
from pathlib import Path
assert 'CI_DASHBOARD_PROGRESS_TOKEN' not in os.environ
name = sys.argv[1]
assert '--retry-failed 2' in os.environ['TEST_R_REPORT_OPTS']
if name.startswith('worker-executor-tests-'):
    assert os.environ['GOLEM_TEST_PORT_OFFSET'] == '0'
    assert os.environ['TMPDIR'].endswith('/worker')
elif name.startswith('integration-tests-group'):
    assert os.environ['GOLEM_TEST_PORT_OFFSET'] == '10000'
    assert os.environ['TMPDIR'].endswith('/service')
elif name.startswith('cli-integration-tests-'):
    assert os.environ['GOLEM_TEST_PORT_OFFSET'] == '20000'
    assert os.environ['TMPDIR'].endswith('/cli')
    if name == 'cli-integration-tests-agents':
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            (project / 'Cargo.toml').write_text('[package]\\nname = "generated-client"\\nversion = "0.1.0"\\n[lib]\\npath = "lib.rs"\\n')
            (project / 'lib.rs').write_text('')
            metadata = subprocess.run(['cargo', 'metadata', '--offline', '--no-deps', '--format-version=1'],
                                      cwd=project, capture_output=True, text=True)
            assert metadata.returncode == 0, metadata.stderr
print('full fixture output for ' + name, flush=True)
if os.environ.get('MAIN_VALIDATION_PARALLEL') == 'true' and name in ('worker-executor-tests-misc', 'main-validation-service-native'):
    Path(name + '.started').touch()
    other = 'worker-executor-tests-misc' if name == 'main-validation-service-native' else 'main-validation-service-native'
    deadline = time.monotonic() + 15
    while not Path(other + '.started').exists():
        assert time.monotonic() < deadline, 'expected cargo-make overlap did not start'
        time.sleep(.05)
sys.exit(9 if name == os.environ.get('FAIL_TASK') else 0)
''')

    def check_graph(self, parallel, failed_task):
        self.prepare_cargo_fixture()
        self.env["FAIL_TASK"] = failed_task
        if not parallel:
            self.env["MAIN_VALIDATION_PARALLEL"] = "false"
        result = self.execute(["observe", "--", "cargo", "make", "--disable-check-for-updates", "--no-workspace",
                               "--profile", "ci", "main-validation"])
        self.assertNotEqual(result.returncode, 0, result.stdout.decode())
        progress = json.loads((self.report / "progress.json").read_text())
        self.assertEqual(progress["mode"], "parallel" if parallel else "serial")
        tasks = {task["id"]: task for task in progress["tasks"]}
        self.assertEqual(len(tasks), 58)
        self.assertEqual(tasks["worker--tool-streaming"]["status"], "passed")
        self.assertEqual(tasks["exclusive--main-validation-docker"]["status"], "passed")
        self.assertEqual(tasks["prepare--unit-tests-and-checks"]["status"], "passed")
        self.assertEqual(tasks["service-build--main-validation-service-native"]["status"], "passed" if parallel else "failed")
        self.assertEqual(tasks["cli--agents"]["status"], "passed" if parallel else "blocked")
        self.assertEqual(tasks["cli--core"]["status"], "passed" if parallel else "blocked")
        self.assertEqual(tasks["service--13"]["status"], "passed" if parallel else "blocked")
        self.assertEqual(tasks["worker--group1"]["status"], "failed" if parallel else "passed")
        self.assertEqual(tasks["worker--group1"]["exit_code"] == 0, not parallel)
        worker = tasks["worker--misc"]
        service = tasks["service-build--main-validation-service-native"]
        self.assertEqual(worker["started_at"] < service["completed_at"] and service["started_at"] < worker["completed_at"], parallel)
        exclusive_start = tasks["exclusive--dev-tools-tests"]["started_at"]
        self.assertTrue(all(task["completed_at"] <= exclusive_start for task in tasks.values()
                            if task["lane"] in ("worker", "service-build", "service", "cli") and task["started_at"]))
        for task in tasks.values():
            if task["status"] in ("passed", "failed"):
                self.assertIn(b"full fixture output", gzip.decompress((self.report / task["log"]).read_bytes()))
        self.assertNotIn(b"full fixture output for wit", result.stdout)
        self.assertLess(len(result.stdout), 50000)
        self.assertIn(b"log:", result.stdout)
        self.assertTrue((self.directory / "summary.md").exists())
        self.assertFalse(Path((self.report / "temporary-root.txt").read_text()).exists())
        samples = [json.loads(line) for line in (self.report / "resources.jsonl").read_text().splitlines()]
        self.assertGreaterEqual(len(samples), 2)
        self.assertTrue(all(sample["scope"] == "runner-host" for sample in samples))
        self.assertGreater(samples[-1]["monotonic_seconds"], samples[0]["monotonic_seconds"])
        self.assertGreater(sum(samples[-1]["cpu_ticks"].values()), sum(samples[0]["cpu_ticks"].values()))
        self.assertGreater(samples[0]["memory_bytes"]["MemTotal"], 0)
        self.assertGreater(samples[0]["disks"]["worktree"]["total_bytes"], 0)
        # Feed a real helper snapshot through the public API's schema, not a hand-made copy.
        public = runpy.run_path(str(ROOT / ".github/main-validation/observer/dashboard/server.py"))["public_progress"]
        created = min(task["started_at"] for task in tasks.values() if task["started_at"])
        public({"run_id": 123, "attempt": 1, "created_at": created, "source": "a" * 40,
                "workflow_sha": "b" * 40, "branch": "main", "event": "workflow_dispatch",
                "context": "Main validation (acceptance)", "progress": progress})

    def test_default_parallel_graph_retains_failed_group_and_joins_siblings(self):
        self.check_graph(True, "worker-executor-tests-group1")

    def test_serial_graph_blocks_consumers_after_failed_producer(self):
        self.check_graph(False, "main-validation-service-native")

    def test_apfs_wrapper_requires_executed_tests_with_console_or_ctrf_output(self):
        binaries = self.directory / "bin"
        binaries.mkdir()
        for name, text in (("uname", "Darwin"), ("hdiutil", "")):
            path = binaries / name
            path.write_text(f'#!/bin/sh\nprintf "%s\\n" "{text}"\n')
            path.chmod(0o755)
        environment = {**self.env, "PATH": str(binaries) + os.pathsep + self.env["PATH"],
                       "TMPDIR": str(self.directory)}
        for output, code, expected in (
            ("test result: ok; 31 passed; 0 failed;", 0, 0),
            ("[1/31] Finished test: native_apfs::clone [PASSED]", 0, 0),
            ("Running 31 tests", 0, 1),
            ("test result: ok; 0 passed; 0 failed;", 0, 1),
            ("[1/31] Finished test: native_apfs::clone [IGNORED]", 0, 1),
            ("[1/31] Finished test: native_apfs::clone [PASSED]", 7, 7),
        ):
            with self.subTest(output=output, code=code):
                result = subprocess.run(
                    ["bash", str(ROOT / "integration-tests/scripts/apfs/run-native-tests.sh"),
                     sys.executable, "-c", f"print({output!r}); raise SystemExit({code})"],
                    env=environment, capture_output=True, timeout=15)
                self.assertEqual(result.returncode, expected, result.stderr)
        self.assertFalse(list(self.directory.glob("golem-apfs-tests.*")))

    def test_unwritable_resource_evidence_does_not_change_workload_result(self):
        self.report.mkdir()
        (self.report / "resources.jsonl").mkdir()
        for code in (0, 7):
            with self.subTest(code=code):
                result = self.execute(["observe", "--", sys.executable, "-c",
                                       f"print('controller evidence'); raise SystemExit({code})"])
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertIn(b"Resource sampling unavailable", result.stdout)
                self.assertIn(b"controller evidence", gzip.decompress((self.report / "logs/controller.log.gz").read_bytes()))


if __name__ == "__main__":
    unittest.main()
