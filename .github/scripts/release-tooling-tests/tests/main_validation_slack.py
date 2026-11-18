"""Slack payload and receipt fixtures; no network or observer state access."""

import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch


ROOT = Path(__file__).resolve().parents[4]
HELPER = ROOT / ".github/main-validation/observer/post-slack.py"


class SlackTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / ".amp/in/orb-backup/slack-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.message = self.directory / "message.txt"
        self.text = "*Main validation failed (acceptance)*\n*Evidence*\n• `fixture::test`: `timeout`"
        self.message.write_text(self.text + "\n", encoding="utf-8")
        self.receipt = self.directory / "receipts/123-a2.json"
        self.env = {"OBSERVER_STATE_DIR": str(self.directory),
                    "CI_DASHBOARD_URL": "https://dashboard.example.invalid/",
                    "SLACK_BENCHMARK_WEBHOOK_URL": "https://slack.example.invalid/fixture"}

    def execute(self, status=200, body=b"ok", error=None):
        response = MagicMock()
        response.__enter__.return_value = response
        response.status = status
        response.read.return_value = body
        with patch.dict(os.environ, self.env, clear=True), \
                patch.object(sys, "argv", [str(HELPER), "123", "2", "failure", str(self.message)]), \
                patch("urllib.request.urlopen", return_value=response, side_effect=error) as post, \
                contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(SystemExit) as exited:
                runpy.run_path(str(HELPER), run_name="__main__")
        return exited.exception.code, post

    def test_mrkdwn_and_labeled_attempt_permalink_preserve_message(self):
        code, post = self.execute()
        self.assertEqual(code, 0)
        post.assert_called_once()
        request = post.call_args.args[0]
        payload = json.loads(request.data)
        expected = self.text + "\n\n<https://dashboard.example.invalid/#run-123-a2|Dashboard>"
        self.assertEqual(payload, {"text": expected, "mrkdwn": True})
        self.assertEqual(request.method, "POST")
        receipt = json.loads(self.receipt.read_text())
        self.assertEqual(receipt["message"], expected)
        self.assertEqual(receipt["delivery"], "confirmed")

    def test_existing_receipts_never_post_or_change(self):
        self.receipt.parent.mkdir()
        for delivery in ("confirmed", "pending", "unknown", "rejected"):
            with self.subTest(delivery=delivery):
                original = json.dumps({"delivery": delivery})
                self.receipt.write_text(original)
                code, post = self.execute()
                self.assertIn("not sending again", code)
                post.assert_not_called()
                self.assertEqual(self.receipt.read_text(), original)

    def test_only_exact_success_response_confirms_delivery(self):
        for status, body in ((200, b"ok\n"), (201, b"ok")):
            with self.subTest(status=status, body=body):
                code, post = self.execute(status=status, body=body)
                self.assertEqual(code, 1)
                post.assert_called_once()
                self.assertEqual(json.loads(self.receipt.read_text())["delivery"], "unknown")
                self.receipt.unlink()

    def test_connection_failure_keeps_receipt_and_blocks_retry(self):
        code, post = self.execute(error=TimeoutError("fixture timeout"))
        self.assertEqual(code, 1)
        post.assert_called_once()
        receipt = json.loads(self.receipt.read_text())
        self.assertEqual(receipt["delivery"], "unknown")
        self.assertEqual(receipt["errorType"], "TimeoutError")
        code, post = self.execute()
        self.assertIn("not sending again", code)
        post.assert_not_called()


if __name__ == "__main__":
    unittest.main()
