"""Exercise the checked-in selection and release shells with offline API fixtures."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from main_validation_cache_lifecycle import ROOT, script, step


MAIN = "a" * 40
FROZEN = "b" * 40


def status(state="success", context="Main validation", creator="github-actions[bot]"):
    return {"state": state, "context": context, "creator": {"login": creator}}


class CutoverShellTests(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/main-validation.yaml").read_text()
        self.release = (ROOT / ".github/workflows/prepare-golem-release.yaml").read_text()
        scratch = ROOT / ".amp/in/orb-backup/cutover-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        tools = self.directory / "bin"
        tools.mkdir()
        gh = tools / "gh"
        gh.write_text(f"#!{sys.executable}\n" + '''
import json, os, sys
from pathlib import Path
with open(os.environ["CALLS"], "a") as calls:
    calls.write(json.dumps(sys.argv[1:]) + "\\n")
responses = json.loads(Path(os.environ["RESPONSES"]).read_text())
args = sys.argv[1:]
if args[:3] == ["api", "--method", "POST"] or args[:2] == ["issue", "close"]:
    sys.exit(0)
for key, response in responses.items():
    if any(key in arg for arg in args):
        if isinstance(response, dict) and "exit" in response:
            print(response.get("body", "API failure"))
            sys.exit(response["exit"])
        print(response if isinstance(response, str) else json.dumps(response))
        sys.exit(0)
raise SystemExit("Unexpected gh call: " + repr(args))
''')
        gh.chmod(0o755)
        git = tools / "git"
        git.write_text('#!/bin/sh\n[ "$*" = "rev-parse HEAD" ] || exit 1\nprintf "%s\\n" "$TAG_SOURCE"\n')
        git.chmod(0o755)
        self.env = {
            **os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
            "GITHUB_REPOSITORY": "fixture/repository", "GITHUB_REF": "refs/heads/main",
            "GITHUB_SHA": FROZEN, "GITHUB_SERVER_URL": "https://github.invalid",
            "GITHUB_RUN_ID": "123", "RUNNER_TEMP": str(self.directory),
            "GITHUB_OUTPUT": str(self.directory / "output"),
            "GITHUB_STEP_SUMMARY": str(self.directory / "summary"),
            "CALLS": str(self.directory / "calls"),
            "RESPONSES": str(self.directory / "responses.json"),
            "ACCEPTANCE": "false", "REQUESTED_SOURCE": "", "TAG_SOURCE": FROZEN,
        }

    def run_shell(self, body, responses, **env):
        Path(self.env["RESPONSES"]).write_text(json.dumps(responses))
        Path(self.env["CALLS"]).write_text("")
        Path(self.env["GITHUB_OUTPUT"]).write_text("")
        return subprocess.run(["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", body],
                              env={**self.env, **env}, cwd=self.directory,
                              capture_output=True, text=True, timeout=10)

    def select(self, responses=None, **env):
        defaults = {"git/ref/heads/main": MAIN, "issues?": [[]], "statuses?": [[]]}
        select_job = self.workflow.split("\n  linux:\n", 1)[0]
        return self.run_shell(script(step(select_job, "Select an exact request or the latest main")),
                              {**defaults, **(responses or {})}, **env)

    def calls(self):
        return Path(self.env["CALLS"]).read_text()

    def output(self):
        return Path(self.env["GITHUB_OUTPUT"]).read_text()

    def test_main_sampling_needs_no_activation_variable(self):
        self.assertNotIn("MAIN_VALIDATION_ENABLED", self.workflow)
        self.assertIn("- cron: '*/10 * * * *'", self.workflow)
        result = self.select()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"source={MAIN}\nrequest=\ncontext=Main validation\nselected=true", self.output())
        self.assertIn("state=pending", self.calls())

    def test_completed_main_skips_but_pending_or_acceptance_does_not(self):
        for state in ("success", "failure", "error", "pending"):
            with self.subTest(state=state):
                result = self.select({"statuses?": [[status(state)]]})
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual("selected=true" in self.output(), state == "pending")
        result = self.select({"statuses?": [[status(context="Main validation (acceptance)")]]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("selected=true", self.output())

    def test_acceptance_is_separate_and_production_rejects_branch_or_override(self):
        for source in ("", MAIN):
            result = self.select({"/commits/": ""}, ACCEPTANCE="true",
                                 GITHUB_REF="refs/heads/feature", REQUESTED_SOURCE=source)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"source={source or FROZEN}", self.output())
            self.assertIn("context=Main validation (acceptance)", self.output())
            self.assertNotIn("issues?", self.calls())
        for env in ({"GITHUB_REF": "refs/heads/feature"}, {"REQUESTED_SOURCE": MAIN}):
            result = self.select(**env)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.calls(), "")

    def test_frozen_tag_request_takes_priority_without_ancestry_requirement(self):
        issue = {"number": 42, "body": json.dumps({"source": FROZEN, "release_tag": "v1.6.0"})}
        for object_type in ("commit", "tag"):
            responses = {"issues?": [[issue]], "git/matching-refs/":
                         {"type": object_type, "sha": FROZEN},
                         "git/tags/": {"type": "commit", "sha": FROZEN}}
            result = self.select(responses)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"source={FROZEN}\nrequest=42", self.output())
            self.assertNotIn("compare/", self.calls())
            self.assertNotIn("issue\", \"close", self.calls())

    def test_invalid_request_closes_then_samples_and_api_failure_stops(self):
        issue = {"number": 42, "body": "not JSON"}
        result = self.select({"issues?": [[issue]]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('"issue", "close", "42"', self.calls())
        self.assertIn(f"source={MAIN}", self.output())
        issue["body"] = json.dumps({"source": FROZEN})
        result = self.select({"issues?": [[issue]], "compare/": {"exit": 1, "body": '{"status":500}'}})
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('"issue", "close"', self.calls())
        self.assertNotIn("selected=true", self.output())

    def test_release_gate_requires_latest_exact_source_bot_production_success(self):
        validate_job = self.release.split("\n  release-prerequisites:\n", 1)[0]
        gate = step(validate_job, "Require completed validation of this exact release source")
        self.assertNotIn("        if:", gate)
        self.assertNotIn("MAIN_VALIDATION_RELEASE_GATE_ENABLED", self.release)
        cases = [([[status()]], True), ([[]], False),
                 ([[status("pending"), status()]], False),
                 ([[status("failure"), status()]], False),
                 ([[status("error")]], False),
                 ([[status(context="Main validation (acceptance)")]], False),
                 ([[status(creator="someone-else")]], False),
                 ([[status(context="Main validation (acceptance)"), status()]], True),
                 ({"exit": 1}, False), ({"exit": 0, "body": "not json"}, False)]
        for response, passed in cases:
            with self.subTest(response=response):
                result = self.run_shell(script(gate), {"statuses?": response})
                self.assertEqual(result.returncode == 0, passed, result.stderr)
                self.assertIn(f"commits/{FROZEN}/statuses?", self.calls())
                self.assertNotIn("dispatch", self.calls())


if __name__ == "__main__":
    unittest.main()
