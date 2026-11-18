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

from server import Store, ThreadingHTTPServer, handler, public_progress


class DashboardTest(unittest.TestCase):
    def setUp(self):
        scratch = Path(__file__).resolve().parents[4] / ".amp/in/orb-backup/dashboard-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.directory.cleanup)
        self.store = Store(self.directory.name)
        self.token = 'test-only-token"with-backslash\\end'
        self.progress_token = 'progress-only-token"with-backslash\\end'
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler(self.store, self.token, self.progress_token))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop)
        self.run = {"run_id": 901, "attempt": 1, "created_at": "2026-11-17T08:00:00Z",
                    "updated_at": "2026-11-17T09:00:00Z", "source": "a" * 40,
                    "context": "Main validation", "status": "completed", "conclusion": "failure",
                    "selected": True, "classification": "failure",
                    "changes": {"available": True, "baseline": "c" * 40, "note": "Oldest first.",
                                "commits": [{"sha": "b" * 40, "subject": "First <change>"},
                                            {"sha": "a" * 40, "subject": "Second change"}]},
                    "jobs": [{"name": "linux", "conclusion": "failure", "failed_steps": ["Validate"]}],
                    "tests": {"reports": [{"file": "unit.json"}],
                              "totals": {"tests": 17, "passed": 15, "failed": 2},
                              "failed": [{"name": "a_test", "message": "<script>untrusted</script>"}]},
                    "analysis": {"summary": "A hypothesis, not a confirmed cause.", "suspects": [
                        {"sha": "b" * 40, "subject": "Fix worker <replay> & delivery", "confidence": "low",
                         "reason": "Changed the affected crate"}]}}

    def stop(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, method, path, value=None, authorized=False, credential=None):
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=5)
        try:
            headers = {"Content-Type": "application/json"}
            if authorized:
                headers["Authorization"] = "Bearer " + self.token
            if credential is not None:
                headers["Authorization"] = "Bearer " + credential
            connection.request(method, path, json.dumps(value) if value is not None else None, headers)
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    def publish(self, run):
        return self.request("POST", "/api/run", run, authorized=True)

    def progress(self):
        return {"run_id": 901, "attempt": 1, "created_at": self.run["created_at"],
                "source": "a" * 40, "workflow_sha": "c" * 40, "branch": "main", "event": "push",
                "context": "Main validation", "progress": {
                    "updated_at": "2026-11-17T08:05:00Z", "mode": "parallel", "tasks": [
                        {"id": "build", "lane": "rust", "name": "Build Rust", "status": "running",
                         "started_at": "2026-11-17T08:01:00Z", "completed_at": None,
                         "pid": 123, "exit_code": None, "log": "tasks/build.log"},
                        {"id": "lint", "lane": "web", "name": "Lint web", "status": "pending",
                         "started_at": None, "completed_at": None, "pid": None,
                         "exit_code": None, "log": "tasks/lint.log"}]}}

    def publish_progress(self, value):
        return self.request("POST", "/api/progress", value, credential=self.progress_token)

    def record(self):
        return json.loads(self.store.snapshot())["runs"][0]

    def test_progress_credential_is_separate_and_missing_configuration_disables_it(self):
        value = self.progress()
        self.assertEqual(self.request("POST", "/api/progress", value)[0], 401)
        self.assertEqual(self.request("POST", "/api/progress", value, authorized=True)[0], 401)
        for path in ("/api/run", "/api/heartbeat"):
            self.assertEqual(self.request("POST", path, self.run, credential=self.progress_token)[0], 401)
        self.assertEqual(self.publish_progress(value)[0], 200)
        self.stop()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler(self.store, self.token))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.assertEqual(self.request("POST", "/api/progress", value, authorized=True)[0], 401)
        self.assertEqual(self.publish_progress(value)[0], 401)

    def test_progress_creates_only_selected_activity_and_cannot_publish_analysis(self):
        value = self.progress()
        value.update(status="completed", conclusion="success", selected=False,
                     analysis={"summary": "must not publish"}, tests={"totals": {"passed": 900}},
                     raw_logs="private", environment={"KEY": "private"}, argv=["private"])
        value["progress"]["tasks"][0].update(raw_logs="private", message="private", argv=["private"])
        self.assertEqual(self.publish_progress(value)[0], 200)
        run = self.record()
        self.assertEqual(run["status"], "in_progress")
        self.assertEqual(run["conclusion"], "")
        self.assertTrue(run["selected"])
        self.assertEqual(run["analysis"]["summary"], "")
        self.assertEqual(run["tests"]["totals"]["passed"], 0)
        self.assertEqual(run["progress"], self.progress()["progress"])
        self.assertNotIn("raw_logs", run)
        self.assertNotIn("_progress_only", run)
        self.assertNotIn("_progress_identity", run)
        self.assertEqual((run["source"], run["context"]), ("", ""))
        self.assertEqual(json.loads(Store(self.directory.name).snapshot())["runs"][0], run)

    def test_unknown_source_refreshes_preserve_live_progress_without_verifying_identity(self):
        self.run.update(status="in_progress", conclusion="", source="", context="")
        self.assertEqual(self.publish(self.run)[0], 200)
        analysis = self.record()["analysis"]
        progress = self.progress()
        self.assertEqual(self.publish_progress(progress)[0], 200)
        for minute in (6, 7, 8):
            self.assertEqual(self.publish(self.run)[0], 200)
            progress["progress"]["updated_at"] = f"2026-11-17T08:0{minute}:00Z"
            self.assertEqual(self.publish_progress(progress)[0], 200)
            self.assertEqual((self.record()["source"], self.record()["context"]), ("", ""))
            self.assertEqual(self.record()["progress"], progress["progress"])
            self.assertEqual(self.record()["analysis"], analysis)
        restarted = Store(self.directory.name)
        wrong = copy.deepcopy(progress)
        wrong["source"] = "d" * 40
        with self.assertRaisesRegex(ValueError, "progress identity changed"):
            restarted.update(public_progress(wrong), progress=True)

    def test_progress_first_unknown_observations_then_matching_final_keep_progress(self):
        progress = self.progress()
        self.assertEqual(self.publish_progress(progress)[0], 200)
        final = copy.deepcopy(self.run)
        self.run.update(status="in_progress", conclusion="", source="", context="")
        for _ in range(3):
            self.assertEqual(self.publish(self.run)[0], 200)
            self.assertEqual(self.record()["source"], "")
            self.assertEqual(self.record()["progress"], progress["progress"])
        self.assertEqual(self.publish(final)[0], 200)
        self.assertEqual(self.record()["source"], final["source"])
        self.assertEqual(self.record()["context"], final["context"])
        self.assertEqual(self.record()["progress"], progress["progress"])

    def test_verified_observation_corrects_progress_identity_and_drops_conflicting_tasks(self):
        progress = self.progress()
        self.assertEqual(self.publish_progress(progress)[0], 200)
        self.run.update(status="in_progress", conclusion="", source="")
        self.assertEqual(self.publish(self.run)[0], 200)
        self.assertEqual(self.record()["progress"], progress["progress"])
        self.run["source"] = "d" * 40
        self.assertEqual(self.publish(self.run)[0], 200)
        self.assertEqual(self.record()["source"], "d" * 40)
        self.assertNotIn("progress", self.record())
        before = self.store.snapshot()
        self.assertEqual(self.publish_progress(progress)[0], 400)
        self.assertEqual(self.store.snapshot(), before)
        progress["source"] = "d" * 40
        self.assertEqual(self.publish_progress(progress)[0], 200)
        self.assertEqual(self.record()["progress"], progress["progress"])

    def test_observation_first_rejects_wrong_source_progress(self):
        self.assertEqual(self.publish(self.run)[0], 200)
        progress = self.progress()
        progress["source"] = "d" * 40
        self.assertEqual(self.publish_progress(progress)[0], 400)
        self.assertNotIn("progress", self.record())
        progress["source"] = self.run["source"]
        self.assertEqual(self.publish_progress(progress)[0], 200)
        self.assertEqual(self.record()["progress"], progress["progress"])

    def test_observer_can_correct_each_progress_only_identity_field(self):
        for index, (field, corrected) in enumerate((
            ("source", "d" * 40), ("workflow_sha", "e" * 40),
            ("branch", "acceptance"), ("event", "workflow_dispatch"),
            ("context", "Main validation (acceptance)"),
        )):
            with self.subTest(field=field):
                progress = self.progress()
                progress["run_id"] += index
                self.assertEqual(self.publish_progress(progress)[0], 200)
                observation = {**self.run, **{name: progress[name] for name in (
                    "run_id", "source", "workflow_sha", "branch", "event", "context")}, field: corrected}
                self.assertEqual(self.publish(observation)[0], 200)
                self.assertEqual(self.record()[field], corrected)
                self.assertNotIn("progress", self.record())
                self.assertEqual(self.publish_progress(progress)[0], 400)

    def test_observer_and_progress_merge_with_independent_clocks_in_both_orders(self):
        value = self.progress()
        self.assertEqual(self.publish_progress(value)[0], 200)
        self.assertIsNone(json.loads(self.store.snapshot())["updated_at"])
        self.run.update(status="in_progress", conclusion="", updated_at="2026-11-17T08:02:00Z")
        self.assertEqual(self.publish(self.run)[0], 200)
        observer_seen = json.loads(self.store.snapshot())["updated_at"]
        self.assertEqual(self.record()["progress"], value["progress"])
        # The observer knows the Actions creation time; the runner can start later.
        value["created_at"] = "2026-11-17T08:00:30Z"
        value["progress"]["updated_at"] = "2026-11-17T08:06:00Z"
        self.assertEqual(self.publish_progress(value)[0], 200)
        self.assertEqual(json.loads(self.store.snapshot())["updated_at"], observer_seen)
        self.assertEqual(self.record()["created_at"], self.run["created_at"])
        self.assertEqual(self.record()["updated_at"], self.run["updated_at"])
        self.assertEqual(self.record()["analysis"]["summary"], self.run["analysis"]["summary"])
        self.assertEqual(self.record()["tests"]["totals"]["failed"], 2)
        self.assertEqual(self.publish(self.run)[0], 200)
        self.assertEqual(self.record()["progress"], value["progress"])
        self.run.update(status="completed", conclusion="failure", updated_at="2026-11-17T08:04:00Z")
        self.assertEqual(self.publish(self.run)[0], 200)
        restarted = Store(self.directory.name)
        self.assertEqual(json.loads(restarted.snapshot())["runs"][0]["progress"], value["progress"])

    def test_progress_does_not_revive_completed_or_cancelled_runs(self):
        for conclusion in ("success", "failure", "cancelled"):
            with self.subTest(conclusion=conclusion):
                self.run.update(conclusion=conclusion)
                self.assertEqual(self.publish(self.run)[0], 200)
                value = self.progress()
                value["progress"]["updated_at"] = "2026-11-17T09:01:00Z"
                self.assertEqual(self.publish_progress(value)[0], 200)
                stored = json.loads(self.store.path.read_text())["runs"][0]
                self.assertEqual((stored["status"], stored["conclusion"]), ("completed", conclusion))
                self.assertEqual(stored["analysis"]["summary"], self.run["analysis"]["summary"])
                self.assertEqual(len(json.loads(self.store.snapshot())["runs"]), 0 if conclusion == "cancelled" else 1)

    def test_progress_order_and_task_history_are_protected(self):
        value = self.progress()
        self.assertEqual(self.publish_progress(value)[0], 200)
        self.assertEqual(self.publish_progress(value)[0], 200)
        for change in ("older", "equal_conflict", "missing", "start", "identity", "mode"):
            invalid = copy.deepcopy(value)
            invalid["progress"]["updated_at"] = "2026-11-17T08:06:00Z"
            if change == "older":
                invalid["progress"]["updated_at"] = "2026-11-17T08:04:00Z"
            elif change == "equal_conflict":
                invalid["progress"]["updated_at"] = value["progress"]["updated_at"]
                invalid["progress"]["tasks"][0]["pid"] = 124
            elif change == "missing":
                invalid["progress"]["tasks"].pop()
            elif change == "start":
                invalid["progress"]["tasks"][0]["started_at"] = "2026-11-17T08:02:00Z"
            elif change == "identity":
                invalid["source"] = "d" * 40
            else:
                invalid["progress"]["mode"] = "serial"
            with self.subTest(change=change):
                self.assertEqual(self.publish_progress(invalid)[0], 400)
                self.assertEqual(self.record()["progress"], value["progress"])
        task = value["progress"]["tasks"][0]
        task.update(status="passed", completed_at="2026-11-17T08:06:00Z", exit_code=0)
        value["progress"]["updated_at"] = "2026-11-17T08:06:00Z"
        self.assertEqual(self.publish_progress(value)[0], 200)
        task.update(completed_at="2026-11-17T08:07:00Z")
        value["progress"]["updated_at"] = "2026-11-17T08:07:00Z"
        self.assertEqual(self.publish_progress(value)[0], 400)
        self.assertEqual(self.record()["progress"]["tasks"][0]["completed_at"], "2026-11-17T08:06:00Z")

    def test_blocked_tasks_have_no_invented_interval_and_compressed_names_can_update(self):
        value = self.progress()
        self.assertEqual(self.publish_progress(value)[0], 200)
        value["progress"]["updated_at"] = "2026-11-17T08:06:00Z"
        value["progress"]["tasks"][0].update(status="passed", completed_at="2026-11-17T08:06:00Z", exit_code=0)
        value["progress"]["tasks"][1].update(status="blocked", completed_at="2026-11-17T08:06:00Z")
        value["progress"]["tasks"][0]["log"] += ".gz"
        self.assertEqual(self.publish_progress(value)[0], 200)
        self.assertEqual(self.record()["progress"], value["progress"])
        self.assertIsNone(self.record()["progress"]["tasks"][1]["started_at"])
        value["progress"]["updated_at"] = "2026-11-17T08:07:00Z"
        value["progress"]["tasks"][1]["log"] += ".gz"
        self.assertEqual(self.publish_progress(value)[0], 400)  # Terminal records cannot change.

    def test_active_log_name_can_only_move_once_to_its_own_gzip_name(self):
        value = self.progress()
        self.assertEqual(self.publish_progress(value)[0], 200)
        value["progress"]["updated_at"] = "2026-11-17T08:06:00Z"
        for name in ("different.log", "tasks/build.gz", "tasks/build.log.gz.gz"):
            value["progress"]["tasks"][0]["log"] = name
            self.assertEqual(self.publish_progress(value)[0], 400)
        value["progress"]["tasks"][0]["log"] = "tasks/build.log.gz"
        self.assertEqual(self.publish_progress(value)[0], 200)
        value["progress"]["updated_at"] = "2026-11-17T08:07:00Z"
        value["progress"]["tasks"][0]["log"] = "tasks/build.log"
        self.assertEqual(self.publish_progress(value)[0], 400)

    def test_progress_rejects_invalid_intervals_paths_and_unbounded_fields(self):
        changes = [
            ("started_at", "2026-11-17T08:06:00Z"), ("started_at", "2026-11-17T07:59:00Z"),
            ("started_at", "2026-11-17T08:00:00"), ("started_at", "2026-02-30T08:00:00Z"),
            ("started_at", "2026-11-17X08:00:00Z"), ("started_at", "2026-11-17T08:00:00+99:00"),
            ("completed_at", "2026-11-17T08:00:00Z"), ("completed_at", "2026-11-17T08:04:00Z"),
            ("status", "passed"), ("status", "pending"), ("pid", True), ("exit_code", 0),
            ("log", "../private.log"), ("log", "/tmp/private.log"), ("log", "tasks/%2e%2e/x"),
            ("log", "https://example.com/log"), ("name", "task\nsecret"), ("name", "n" * 201)]
        for field, replacement in changes:
            value = self.progress()
            value["progress"]["tasks"][0][field] = replacement
            with self.subTest(field=field, replacement=replacement):
                self.assertEqual(self.publish_progress(value)[0], 400)
        for field, replacement in (("updated_at", "bad"), ("updated_at", "2026-11-17T07:00:00Z"),
                                   ("mode", "auto"), ("tasks", [self.progress()["progress"]["tasks"][0]] * 201)):
            value = self.progress()
            value["progress"][field] = replacement
            self.assertEqual(self.publish_progress(value)[0], 400)
        for secret in (self.token, self.progress_token):
            value = self.progress()
            value["diagnostic"] = secret  # JSON escapes quote and backslash before transmission.
            self.assertEqual(self.publish_progress(value)[0], 400)
        self.assertEqual(json.loads(self.store.snapshot())["runs"], [])

    def test_actual_overlapping_intervals_and_final_progress_survive_restart(self):
        value = self.progress()
        value["progress"]["tasks"][0].update(status="passed", completed_at="2026-11-17T08:04:00Z", exit_code=0)
        value["progress"]["tasks"][1].update(status="passed", started_at="2026-11-17T08:02:00Z",
                                                  completed_at="2026-11-17T08:03:00Z", pid=124, exit_code=0)
        self.assertEqual(self.publish_progress(value)[0], 200)
        self.assertEqual(self.record()["status"], "in_progress")  # Task results do not conclude the run.
        self.assertEqual(self.publish(self.run)[0], 200)
        saved = json.loads(Store(self.directory.name).snapshot())["runs"][0]
        self.assertEqual(saved["progress"], value["progress"])
        self.assertEqual(saved["status"], "completed")
        self.assertEqual(saved["tests"]["totals"]["passed"], 15)
        rerun = copy.deepcopy(value)
        rerun["attempt"] = 2
        self.assertEqual(self.publish_progress(rerun)[0], 200)
        self.assertEqual([r["attempt"] for r in json.loads(self.store.snapshot())["runs"]], [2, 1])

    def test_failed_atomic_progress_write_keeps_previous_snapshot(self):
        value = self.progress()
        self.assertEqual(self.publish_progress(value)[0], 200)
        before = self.store.snapshot()
        value["progress"]["updated_at"] = "2026-11-17T08:06:00Z"
        with patch("server.os.replace", side_effect=OSError("disk unavailable")):
            self.assertEqual(self.publish_progress(value)[0], 503)
        self.assertEqual(self.store.snapshot(), before)
        self.assertEqual(Store(self.directory.name).snapshot(), before)

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
        self.assertEqual(record["analysis"]["suspects"][0]["subject"], "Fix worker <replay> & delivery")
        self.assertEqual(record["changes"], self.run["changes"])

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

    def test_publisher_never_uses_public_progress_to_verify_source_comparison(self):
        progress = self.progress()
        self.assertEqual(self.publish_progress(progress)[0], 200)
        facts, analysis = Path(self.directory.name) / "facts.json", Path(self.directory.name) / "analysis.json"
        analysis.write_text(json.dumps({"classification": "running", "analysis": {"summary": "Observing"}}))

        def send(request, timeout):
            self.assertEqual(request.get_method(), "POST")
            self.assertEqual(request.full_url, "https://ci.example.invalid/api/run")
            payload = json.loads(request.data)
            self.assertEqual((payload["source"], payload["context"]), (self.run["source"], self.run["context"]))
            self.assertFalse(payload["changes"]["available"])
            self.assertEqual(payload["changes"]["baseline"], "")
            self.assertEqual(payload["changes"]["commits"], [])
            self.assertIn("not yet verified", payload["changes"]["note"])
            self.assertEqual(self.publish(payload)[0], 200)
            response = io.BytesIO(b'{"status":"stored"}')
            response.status = 200
            return response

        for source, context in (("", ""), ("", "Main validation"), ("a" * 40, "")):
            with self.subTest(source=source, context=context):
                self.run.update(status="in_progress", conclusion="", source=source, context=context)
                facts.write_text(json.dumps(self.run))
                with patch.dict("os.environ", {"CI_DASHBOARD_URL": "https://ci.example.invalid", "CI_DASHBOARD_TOKEN": self.token}), \
                        patch.object(sys, "argv", ["publish-dashboard.py", str(facts), str(analysis)]), \
                        patch("urllib.request.urlopen", side_effect=send) as network, \
                        patch("subprocess.run") as git, contextlib.redirect_stdout(io.StringIO()):
                    runpy.run_path(str(Path(__file__).resolve().parents[1] / "publish-dashboard.py"), run_name="__main__")
                    network.assert_called_once()
                    git.assert_not_called()
                self.assertEqual(self.record()["progress"], progress["progress"])
                self.assertEqual((self.record()["source"], self.record()["context"]), (source, context))
                self.assertFalse(self.record()["changes"]["available"])

    def test_malformed_report_cannot_hide_valid_report_or_contribute_partial_totals(self):
        directory = Path(self.directory.name)
        (directory / "progress.json").write_text(json.dumps({"tasks": []}))
        (directory / "executor-binaries.json").write_text("not a CTRF report")
        (directory / "ctrf-a-bad.json").write_text(json.dumps({"results": {"tests": None}}))
        (directory / "ctrf-b-partial.json").write_text(json.dumps({"results": {"tests": [
            {"name": "must_not_count", "status": "passed"}, {"status": "failed", "extra": 17}]}}))
        (directory / "ctrf.json").write_text(json.dumps({"results": {"tests": [
            {"name": "actual_failure", "status": "failed", "message": "assertion failed"}]}}))
        output = io.StringIO()
        with patch.object(sys, "argv", ["ctrf-summary.py", str(directory)]), contextlib.redirect_stdout(output):
            runpy.run_path(str(Path(__file__).resolve().parents[1] / "ctrf-summary.py"), run_name="__main__")
        summary = json.loads(output.getvalue())
        self.assertEqual(summary["totals"]["tests"], 1)
        self.assertEqual([test["name"] for test in summary["failed"]], ["actual_failure"])
        self.assertEqual([report["file"] for report in summary["unreadable"]], ["ctrf-a-bad.json", "ctrf-b-partial.json"])

    def test_dashboard_shows_only_running_and_non_cancelled_completed_attempts(self):
        states = [("in_progress", ""), ("completed", "success"), ("completed", "failure"),
                  ("completed", "cancelled"), ("queued", ""), ("pending", ""),
                  ("waiting", ""), ("requested", ""), ("completed", "timed_out")]
        for index, (status, conclusion) in enumerate(states):
            run = copy.deepcopy(self.run)
            run.update(run_id=901 + index, status=status, conclusion=conclusion)
            self.assertEqual(self.publish(run)[0], 200)

        status, payload = self.request("GET", "/data.json")
        self.assertEqual(status, 200)
        self.assertEqual([run["run_id"] for run in json.loads(payload)["runs"]], [909, 903, 902, 901])
        self.assertEqual(len(json.loads(self.store.path.read_text())["runs"]), len(states))

        pending = copy.deepcopy(self.run)
        pending.update(run_id=906, status="in_progress", conclusion="")
        self.assertEqual(self.publish(pending)[0], 200)
        self.assertIn(906, [run["run_id"] for run in json.loads(self.request("GET", "/data.json")[1])["runs"]])
        pending.update(status="completed", conclusion="cancelled")
        self.assertEqual(self.publish(pending)[0], 200)
        self.assertNotIn(906, [run["run_id"] for run in json.loads(self.request("GET", "/data.json")[1])["runs"]])

    def test_no_selection_wakeups_are_counted_without_hiding_unknown_failures(self):
        cases = [
            (False, "no-selection", "schedule", "skipped"),
            (False, "incomplete", "schedule", "success"),
            (None, "no-selection", "schedule", "success"),
            (False, "no-selection", "workflow_dispatch", "skipped"),
            (False, "no-selection", "schedule", "cancelled"),
            (None, "incomplete", "schedule", "failure"),
            (True, "green", "schedule", "success"),
        ]
        for index, (selected, classification, event, conclusion) in enumerate(cases):
            run = copy.deepcopy(self.run)
            run.update(run_id=901 + index, selected=selected, classification=classification,
                       event=event, conclusion=conclusion)
            self.assertEqual(self.publish(run)[0], 200)
        for store in (self.store, Store(self.directory.name)):
            snapshot = json.loads(store.snapshot())
            self.assertEqual(snapshot["skipped_wakeups"], 3)
            self.assertEqual([run["run_id"] for run in snapshot["runs"]], [907, 906])
        self.assertEqual(len(json.loads(self.store.path.read_text())["runs"]), len(cases))

    def test_heartbeat_does_not_fabricate_a_run(self):
        status, _ = self.request("POST", "/api/heartbeat", {}, authorized=True)
        self.assertEqual(status, 200)
        data = json.loads(self.store.snapshot())
        self.assertIsNotNone(data["updated_at"])
        self.assertEqual(data["runs"], [])


if __name__ == "__main__":
    unittest.main()
