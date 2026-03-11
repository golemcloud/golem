---
name: analysing-ci-failures
description: "Analysing GitHub Actions CI failures from a run URL. Use when the user pastes a GitHub CI run or job URL and wants to understand why it failed and how to reproduce locally."
---

# Analysing CI Failures

Diagnose GitHub Actions CI failures by downloading logs and CTRF test reports, then providing a summary of what failed and how to reproduce it locally.

## Step 1: Parse the URL

The user provides a GitHub Actions URL in one of these forms:

- **Job URL:** `https://github.com/golemcloud/golem/actions/runs/<run_id>/job/<job_id>?pr=<pr>`
- **Run URL:** `https://github.com/golemcloud/golem/actions/runs/<run_id>?pr=<pr>`

Extract the `run_id` and `job_id` (if present) from the URL.

## Step 2: Download the Job Log

Use `gh` to download the full log for the failed job and save it to a temp file:

```shell
# If a specific job_id is available:
gh run view <run_id> --repo golemcloud/golem --log --job <job_id> > tmp/ci_log.txt 2>&1

# If only the run_id is available, first list the jobs to find the failed one:
gh run view <run_id> --repo golemcloud/golem --json jobs --jq '.jobs[] | select(.conclusion == "failure") | .databaseId'
# Then download the log for the failed job:
gh run view <run_id> --repo golemcloud/golem --log --job <failed_job_id> > tmp/ci_log.txt 2>&1
```

## Step 3: Download CTRF Reports (if available)

CTRF (Common Test Report Format) JSON reports are attached as artifacts to the CI run summary page. Download them:

```shell
# List available artifacts for the run:
gh run view <run_id> --repo golemcloud/golem --json artifacts --jq '.artifacts[].name'

# Download CTRF artifacts (names typically contain "ctrf" or "test-report"):
gh run download <run_id> --repo golemcloud/golem --pattern '*ctrf*' --dir tmp/ctrf_reports/
```

If CTRF artifacts exist, parse the JSON files to extract failed test names, error messages, and stack traces. CTRF files have this structure:

```json
{
  "results": {
    "summary": { "tests": 100, "passed": 98, "failed": 2 },
    "tests": [
      {
        "name": "test_name",
        "status": "failed",
        "message": "error message",
        "trace": "stack trace"
      }
    ]
  }
}
```

Filter for tests with `"status": "failed"` to find the failures.

## Step 4: Analyse the Log

Search the downloaded log file for failure indicators:

```shell
grep -n "FAILED" tmp/ci_log.txt
grep -n "error\[" tmp/ci_log.txt
grep -n "panic" tmp/ci_log.txt
grep -n "timed out" tmp/ci_log.txt
grep -n "test result:" tmp/ci_log.txt
```

Look for:
- **Compilation errors:** `error[E...]` lines with file paths and error descriptions
- **Test failures:** `FAILED` test names, assertion messages, panics
- **Timeouts:** Tests that exceeded their time limit
- **Infrastructure issues:** Docker failures, network errors, resource exhaustion

## Step 5: Provide a Summary

Present the findings to the user including:

1. **What failed:** The specific test(s) or build step(s) that failed
2. **Error details:** The error messages, assertion failures, or panic messages
3. **Root cause hypothesis:** What likely caused the failure based on the logs
4. **How to reproduce locally:** The exact command to run the failing test(s):

```shell
# For a specific failing test:
cargo test -p <crate> -- <test_name> --nocapture --report-time

# For the full test suite that failed:
cargo make <test-command>  # e.g., worker-executor-tests, integration-tests
```

Refer to the `testing` skill's table for the correct `cargo make` command based on the type of test that failed.
