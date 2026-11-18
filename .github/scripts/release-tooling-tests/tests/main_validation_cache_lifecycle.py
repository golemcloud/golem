"""Execute the workflow shells without GitHub or Blacksmith side effects."""

import itertools
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[4]
WORKFLOW = ROOT / ".github/workflows/main-validation.yaml"
COMMAND = (".validation-control/.github/main-validation/progress.py observe -- "
           "cargo make --profile ci main-validation")
SUCCESS = {
    "needs.linux.result": "success",
    "needs.linux.outputs.workload_exit_code": "0",
    "needs.apfs.result": "success",
    "steps.linux-reports.outcome": "success",
    "steps.apfs-reports.outcome": "success",
    "steps.report.outcome": "success",
}


def step(workflow, name):
    """Extract one named step, retaining its literal Bash and expressions."""
    marker = "      - name: " + name + "\n"
    return workflow.split(marker, 1)[1].split("\n      - ", 1)[0]


def script(step_text):
    return textwrap.dedent(step_text.split("        run: |\n", 1)[1])


class CacheLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.workflow = WORKFLOW.read_text()
        self.workload = step(self.workflow, "Validate the selected source")
        self.publish = step(self.workflow, "Publish the exact-source result")
        scratch = ROOT / ".amp/in/orb-backup/cache-lifecycle-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        tools = self.directory / "bin"
        tools.mkdir()
        for name, body in {
            "python3": 'printf "%s\\n" "$*" > "$WORKLOAD_ARGS"\nexit "$WORKLOAD_STATUS"\n',
            "gh": 'printf "%s\\n" "$*" >> "$GH_CALLS"\n',
        }.items():
            executable = tools / name
            executable.write_text("#!/bin/sh\n" + body)
            executable.chmod(0o755)
        self.output = self.directory / "output"
        self.summary = self.directory / "summary"
        self.calls = self.directory / "gh-calls"
        self.arguments = self.directory / "workload-args"
        self.env = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                    "GITHUB_OUTPUT": str(self.output), "GITHUB_STEP_SUMMARY": str(self.summary),
                    "WORKLOAD_ARGS": str(self.arguments), "GH_CALLS": str(self.calls),
                    "GITHUB_SERVER_URL": "https://github.example.invalid",
                    "GITHUB_REPOSITORY": "fixture/repository", "GITHUB_RUN_ID": "123",
                    "SOURCE": "a" * 40, "REQUEST": "42", "CONTEXT": "Main validation"}

    def bash(self, body, **env):
        return subprocess.run(["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", body],
                              env={**self.env, **env}, cwd=self.directory,
                              capture_output=True, text=True, timeout=10)

    def passed(self, values):
        expression = re.search(r"PASSED: \$\{\{ (.+) \}\}", self.publish).group(1)
        # Accept only the workflow's conjunction of explicit string comparisons.
        # A new expression shape must receive a matching fixture, not Python eval.
        terms = []
        for term in expression.split(" && "):
            match = re.fullmatch(r"([\w.-]+) == '([^']*)'", term)
            self.assertIsNotNone(match, term)
            terms.append(match.groups())
        self.assertEqual(dict(terms), SUCCESS)
        self.assertEqual(len(terms), len(SUCCESS))
        return all(values.get(key, "") == expected for key, expected in terms)

    def assert_publication(self, values, expected):
        self.calls.unlink(missing_ok=True)
        result = self.bash(script(self.publish), PASSED=str(self.passed(values)).lower())
        self.assertEqual(result.returncode, 0 if expected == "success" else 1,
                         result.stdout + result.stderr)
        calls = self.calls.read_text().splitlines()
        self.assertEqual(len(calls), 2)
        self.assertIn("api --method POST repos/fixture/repository/statuses/" + "a" * 40, calls[0])
        self.assertIn("-f state=" + expected + " -f context=Main validation", calls[0])
        self.assertIn("issue close 42 --repo fixture/repository", calls[1])
        self.assertIn("finished: " + expected, calls[1])
        self.assertIn("Validation of " + "a" * 40 + ": " + expected, self.summary.read_text())

    def test_shell_preserves_success_and_real_failure_exit_codes(self):
        for status in (0, 1, 17, 127, 143):
            with self.subTest(status=status):
                self.output.unlink(missing_ok=True)
                result = self.bash(script(self.workload), WORKLOAD_STATUS=str(status))
                # Both action implementations see a successful workload step.
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.arguments.read_text().strip(), COMMAND)
                self.assertEqual(self.output.read_text(), f"exit_code={status}\n")
                values = {**SUCCESS, "needs.linux.outputs.workload_exit_code": str(status)}
                self.assert_publication(values, "success" if status == 0 else "failure")

    def test_missing_output_cannot_publish_success(self):
        result = self.bash(script(self.workload), WORKLOAD_STATUS="0", GITHUB_OUTPUT=os.devnull)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.output.exists())
        values = dict(SUCCESS)
        del values["needs.linux.outputs.workload_exit_code"]
        self.assert_publication(values, "failure")

    def test_output_write_failure_remains_a_job_failure(self):
        result = self.bash(script(self.workload), WORKLOAD_STATUS="0",
                           GITHUB_OUTPUT=str(self.directory / "missing/output"))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())
        self.assert_publication({**SUCCESS, "needs.linux.result": "failure",
                                 "needs.linux.outputs.workload_exit_code": ""}, "failure")

    def test_result_truth_table_fails_closed_for_every_non_success(self):
        keys = list(SUCCESS)
        choices = [("0", "1", "17", "", "success") if key.endswith("workload_exit_code")
                   else ("success", "failure", "cancelled", "skipped", "") for key in keys]
        for row in itertools.product(*choices):
            values = dict(zip(keys, row))
            with self.subTest(values=values):
                self.assertEqual(self.passed(values), values == SUCCESS)
        # Run the actual publishing shell for each independent failed prerequisite.
        for key in keys:
            for value in choices[keys.index(key)][1:]:
                with self.subTest(key=key, value=value):
                    self.assert_publication({**SUCCESS, key: value}, "failure")

    def test_cache_owner_and_result_wiring_do_not_mask_infrastructure_failures(self):
        linux = self.workflow.split("\n  linux:\n", 1)[1].split("\n  apfs:\n", 1)[0]
        self.assertIn("    outputs:\n      workload_exit_code: ${{ steps.workload.outputs.exit_code }}", linux)
        self.assertIn("        id: workload\n", self.workload)
        self.assertIn("        shell: bash\n", self.workload)
        self.assertIn("        timeout-minutes: 300\n", self.workload)
        self.assertIn("    timeout-minutes: 330\n", linux)
        self.assertNotIn("continue-on-error", linux)
        self.assertNotIn("job-status:", linux)
        self.assertEqual(linux.count("uses: useblacksmith/stickydisk@v1"), 2)
        for name, key, path in (("compiler", "target", "target"),
                                ("dependency", "dependencies", ".validation-cache")):
            cache = step(self.workflow, f"Mount {name} caches")
            self.assertIn(f"          key: main-validation-linux-{key}\n", cache)
            self.assertIn(f"          path: {path}\n", cache)
            self.assertIn("          commit: 'true'", cache)
        reports = step(self.workflow, "Retain Linux reports even after failure")
        self.assertIn("        if: always()\n", reports)
        self.assertIn("          if-no-files-found: error\n", reports)
        self.assertIn("          retention-days: 90", reports)
        self.assertIn("    needs: [select, linux, apfs]\n", self.workflow)
        self.assertIn("    if: always() && needs.select.outputs.selected == 'true'\n", self.workflow)
        self.assertIn("        if: always()\n", self.publish)
        self.assertNotIn("continue-on-error", self.publish)


if __name__ == "__main__":
    unittest.main()
