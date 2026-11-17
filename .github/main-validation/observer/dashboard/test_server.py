"""Contract tests for public observations, persistence and authenticated publishing."""

import copy
import contextlib
import http.client
import io
import json
from pathlib import Path
import runpy
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

from server import Store, ThreadingHTTPServer, handler


class DashboardTest(unittest.TestCase):
    def setUp(self):
        scratch = Path(__file__).resolve().parents[4] / ".amp/in/orb-backup/dashboard-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.directory.cleanup)
        self.store = Store(self.directory.name)
        self.token = 'test-only-token"with-backslash\\end'
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler(self.store, self.token))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop)
        self.run = {"run_id": 901, "attempt": 1, "created_at": "2026-11-17T08:00:00Z",
                    "updated_at": "2026-11-17T09:00:00Z", "source": "a" * 40,
                    "context": "Main validation", "status": "completed", "conclusion": "failure",
                    "selected": True, "classification": "failure",
                    "jobs": [{"name": "linux", "conclusion": "failure", "failed_steps": ["Validate"]}],
                    "tests": {"reports": [{"file": "unit.json"}],
                              "totals": {"tests": 17, "passed": 15, "failed": 2},
                              "failed": [{"name": "a_test", "message": "<script>untrusted</script>"}]},
                    "analysis": {"summary": "A hypothesis, not a confirmed cause.", "suspects": [
                        {"sha": "b" * 40, "confidence": "low", "reason": "Changed the affected crate"}]}}

    def stop(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, method, path, value=None, authorized=False):
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            headers = {"Content-Type": "application/json"}
            if authorized:
                headers["Authorization"] = "Bearer " + self.token
            connection.request(method, path, json.dumps(value) if value is not None else None, headers)
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    def publish(self, run):
        return self.request("POST", "/api/run", run, authorized=True)

    def test_read_is_public_but_writes_require_credentials(self):
        self.assertEqual(self.request("GET", "/data.json")[0], 200)
        self.assertEqual(self.request("POST", "/api/run", self.run)[0], 401)
        self.assertEqual(self.request("GET", "/server.py")[0], 404)
        self.assertEqual(self.publish(self.run)[0], 200)

    def test_persists_only_public_fields_and_preserves_observations_after_restart(self):
        self.run["raw_logs"] = "not public"
        self.run["environment"] = {"SECRET": "not public"}
        self.run["url"] = "javascript:alert(1)"
        self.assertEqual(self.publish(self.run)[0], 200)
        self.assertEqual(self.publish(self.run)[0], 200)
        saved = json.loads(Store(self.directory.name).snapshot())
        self.assertEqual(len(saved["runs"]), 1)
        record = saved["runs"][0]
        self.assertNotIn("raw_logs", record)
        self.assertNotIn("environment", record)
        self.assertEqual(record["url"], "https://github.com/golemcloud/golem/actions/runs/901/attempts/1")
        self.assertEqual(record["tests"]["totals"]["passed"], 15)
        self.assertEqual(record["analysis"]["suspects"][0]["sha"], "b" * 40)

    def test_reruns_have_distinct_records_and_old_activity_cannot_replace_completion(self):
        self.assertEqual(self.publish(self.run)[0], 200)
        older = copy.deepcopy(self.run)
        older.update(status="in_progress", classification="running", updated_at="2026-11-17T08:01:00Z")
        self.assertEqual(self.publish(older)[0], 400)
        rerun = copy.deepcopy(self.run)
        rerun.update(attempt=2, status="in_progress", conclusion=None, classification="running")
        self.assertEqual(self.publish(rerun)[0], 200)
        runs = json.loads(self.store.snapshot())["runs"]
        self.assertEqual([run["attempt"] for run in runs], [2, 1])
        self.assertEqual(runs[1]["classification"], "failure")

    def test_malformed_or_credential_bearing_payload_does_not_change_history(self):
        invalid = copy.deepcopy(self.run)
        invalid["source"] = "not-an-exact-sha"
        self.assertEqual(self.publish(invalid)[0], 400)
        invalid = copy.deepcopy(self.run)
        invalid["analysis"]["summary"] = self.token
        self.assertEqual(self.publish(invalid)[0], 400)
        self.assertEqual(json.loads(self.store.snapshot())["runs"], [])

    def test_publisher_rejects_escaped_credentials_before_transmission(self):
        facts, analysis = Path(self.directory.name) / "facts.json", Path(self.directory.name) / "analysis.json"
        facts.write_text(json.dumps(self.run))
        analysis.write_text(json.dumps({"classification": "failure", "analysis": {"summary": self.token}}))
        with patch.dict("os.environ", {"CI_DASHBOARD_URL": "https://ci.example.invalid", "CI_DASHBOARD_TOKEN": self.token}), \
                patch.object(sys, "argv", ["publish-dashboard.py", str(facts), str(analysis)]), \
                patch("urllib.request.urlopen") as network:
            with self.assertRaisesRegex(SystemExit, "refusing to publish a credential"):
                runpy.run_path(str(Path(__file__).resolve().parents[1] / "publish-dashboard.py"), run_name="__main__")
            network.assert_not_called()

    def test_malformed_report_cannot_hide_valid_report_or_contribute_partial_totals(self):
        directory = Path(self.directory.name)
        (directory / "a-bad.json").write_text(json.dumps({"results": {"tests": None}}))
        (directory / "b-partial.json").write_text(json.dumps({"results": {"tests": [
            {"name": "must_not_count", "status": "passed"}, {"status": "failed", "extra": 17}]}}))
        (directory / "c-valid.json").write_text(json.dumps({"results": {"tests": [
            {"name": "actual_failure", "status": "failed", "message": "assertion failed"}]}}))
        output = io.StringIO()
        with patch.object(sys, "argv", ["ctrf-summary.py", str(directory)]), contextlib.redirect_stdout(output):
            runpy.run_path(str(Path(__file__).resolve().parents[1] / "ctrf-summary.py"), run_name="__main__")
        summary = json.loads(output.getvalue())
        self.assertEqual(summary["totals"]["tests"], 1)
        self.assertEqual([test["name"] for test in summary["failed"]], ["actual_failure"])
        self.assertEqual([report["file"] for report in summary["unreadable"]], ["a-bad.json", "b-partial.json"])

    def test_heartbeat_does_not_fabricate_a_run(self):
        status, _ = self.request("POST", "/api/heartbeat", {}, authorized=True)
        self.assertEqual(status, 200)
        data = json.loads(self.store.snapshot())
        self.assertIsNotNone(data["updated_at"])
        self.assertEqual(data["runs"], [])


if __name__ == "__main__":
    unittest.main()
