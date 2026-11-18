"""Real Git histories for the dashboard's complete previous-run source range."""

import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch


ROOT = Path(__file__).resolve().parents[4]
HELPER = ROOT / ".github/main-validation/observer/publish-dashboard.py"


class ChangesTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / ".amp/in/orb-backup/changes-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.state = self.directory / "state"
        (self.state / "seen").mkdir(parents=True)
        self.env = patch.dict(os.environ, {
            "CI_DASHBOARD_URL": "https://dashboard.example.invalid", "CI_DASHBOARD_TOKEN": "fixture-token",
            "OBSERVER_STATE_DIR": str(self.state), "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
            "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.cwd = contextlib.chdir(self.directory)
        self.cwd.__enter__()
        self.addCleanup(self.cwd.__exit__, None, None, None)
        self.git("init", "-q")
        self.git("commit", "--allow-empty", "-qm", "Initial source")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.git("commit", "--allow-empty", "-qm", "First change <safe> & literal")
        self.first = self.git("rev-parse", "HEAD").strip()
        self.git("commit", "--allow-empty", "-qm", "Second change")
        self.second = self.git("rev-parse", "HEAD").strip()
        response = MagicMock()
        response.status = 200
        response.read.return_value = b'{"status":"stored"}'
        with patch.object(sys, "argv", [str(HELPER), "--heartbeat"]), \
                patch("urllib.request.urlopen") as network, contextlib.redirect_stdout(io.StringIO()):
            network.return_value.__enter__.return_value = response
            self.collect = runpy.run_path(str(HELPER))["changes_since_previous_run"]
        self.run = {"run_id": 50, "source": self.second, "context": "Main validation",
                    "created_at": "2026-11-18T07:00:00Z"}

    def git(self, *arguments):
        return subprocess.run(["git", *arguments], capture_output=True, text=True,
                              timeout=10, check=True).stdout

    def seen(self, run_id, source, time, **fields):
        record = {"run_id": run_id, "attempt": 1, "source": source, "selected": True,
                  "context": "Main validation", "classification": "green", "conclusion": "success",
                  "created_at": f"2026-11-18T{time}:00Z", **fields}
        (self.state / "seen" / f"{run_id}.json").write_text(json.dumps(record))

    def test_lists_every_new_commit_in_order_with_literal_subjects(self):
        self.seen(40, self.base, "06:00")
        result = self.collect(self.run)
        self.assertTrue(result["available"], result)
        self.assertEqual(result["baseline"], self.base)
        self.assertEqual(result["commits"], [
            {"sha": self.first, "subject": "First change <safe> & literal"},
            {"sha": self.second, "subject": "Second change"}])

    def test_uses_previous_failure_not_last_green_and_ignores_other_contexts(self):
        self.seen(30, self.base, "05:00")
        self.seen(40, self.first, "06:00", classification="failure", conclusion="failure")
        self.seen(41, self.second, "06:01", context="Main validation (acceptance)")
        self.seen(42, self.second, "06:02", selected=False)
        self.seen(43, self.second, "06:03", conclusion="cancelled")
        self.seen(50, self.second, "06:04")
        self.seen(51, self.second, "08:00")
        result = self.collect(self.run)
        self.assertEqual(result["baseline"], self.first)
        self.assertEqual([commit["sha"] for commit in result["commits"]], [self.second])

    def test_same_source_is_verified_empty_not_unavailable(self):
        self.seen(40, self.second, "06:00")
        result = self.collect(self.run)
        self.assertTrue(result["available"])
        self.assertEqual(result["commits"], [])

    def test_running_publication_compares_only_observer_verified_source_and_context(self):
        self.seen(40, self.base, "06:00")
        facts = self.directory / "facts.json"
        analysis = self.directory / "analysis.json"
        analysis.write_text(json.dumps({"classification": "running", "analysis": {"summary": "In progress"}}))
        for fields, expected in (({}, True), ({"source": ""}, False), ({"context": ""}, False),
                                 ({"source": "", "context": ""}, False)):
            with self.subTest(fields=fields):
                observation = {**self.run, "status": "in_progress", "attempt": 1, **fields}
                facts.write_text(json.dumps(observation))
                response = io.BytesIO(b'{"status":"stored"}')
                response.status = 200
                with patch.object(sys, "argv", [str(HELPER), str(facts), str(analysis)]), \
                        patch("urllib.request.urlopen", return_value=response) as network, \
                        contextlib.redirect_stdout(io.StringIO()):
                    runpy.run_path(str(HELPER))
                network.assert_called_once()
                request = network.call_args.args[0]
                self.assertEqual(request.get_method(), "POST")
                self.assertEqual(request.full_url, "https://dashboard.example.invalid/api/run")
                published = json.loads(request.data)
                self.assertEqual(published["source"], observation["source"])
                self.assertEqual(published["context"], observation["context"])
                self.assertEqual(published["changes"]["available"], expected)
                if expected:
                    self.assertEqual(published["changes"]["baseline"], self.base)
                    self.assertEqual(published["changes"]["commits"], [
                        {"sha": self.first, "subject": "First change <safe> & literal"},
                        {"sha": self.second, "subject": "Second change"}])
                else:
                    self.assertEqual(published["changes"]["baseline"], "")
                    self.assertEqual(published["changes"]["commits"], [])
                    self.assertIn("not yet verified", published["changes"]["note"])

    def test_missing_baseline_or_unknown_source_is_not_an_empty_range(self):
        self.assertFalse(self.collect(self.run)["available"])
        self.seen(40, self.base, "06:00")
        self.assertFalse(self.collect({**self.run, "source": ""})["available"])
        self.seen(40, "a" * 40, "06:00")
        result = self.collect(self.run)
        self.assertFalse(result["available"])
        self.assertEqual(result["commits"], [])

    def test_divergent_histories_show_only_newly_reachable_commits_and_explain(self):
        self.git("checkout", "-q", "--detach", self.base)
        self.git("commit", "--allow-empty", "-qm", "Other branch")
        other = self.git("rev-parse", "HEAD").strip()
        self.seen(40, other, "06:00")
        result = self.collect(self.run)
        self.assertTrue(result["available"])
        self.assertIn("diverge", result["note"])
        self.assertEqual([commit["sha"] for commit in result["commits"]], [self.first, self.second])


if __name__ == "__main__":
    unittest.main()
