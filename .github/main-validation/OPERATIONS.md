# Acceptance and operations

Neither production sampling nor the new release gate is enabled. Do not change
variables, dispatch workflows, create request labels/issues/statuses, post to
Slack or change rulesets without authorization. Keep today's CI active until
the real Blacksmith flow and reporting are accepted.

## Blacksmith acceptance

1. Run a cold serial cycle from the feature branch in acceptance mode. Compare
   its CTRF test identities and skips against green ordinary CI on that same
   source. Account separately for checks which do not emit CTRF.
2. Run a warm acceptance cycle with `parallel=true`. Measure duration, peak memory
   and disk. Check ports, Redis, files and containers across all three subflows.
   Keep production serial if overlap does not pass.
3. Exercise a persistent test failure and a recovered flaky test. Confirm two
   failed-test retries, the final result and retained original failures.
4. Cancel a genuine run, then verify the next run can use the sticky disks and
   managed-XFS VM cleanup. The development orb has no KVM and cannot prove this.
5. Exercise sampling: merges B and C during A select C next; unchanged completed
   main starts no workload. A valid request goes first. An invalid request is
   closed and does not stop sampling. API failures do not close valid requests.
6. Verify APFS succeeds on the same source. Measure the provisional 300-minute
   workload and 330-minute Linux job limits before finalizing them.

Acceptance uses `Main validation (acceptance)`, not the release status. The
production flags remain independent: `MAIN_VALIDATION_ENABLED` for sampling,
`MAIN_VALIDATION_RELEASE_GATE_ENABLED` for preparation. Accept the full-CI and
new release paths, the exact frozen preparation head, both skip results, and
failed/missing/pending status before switching the release path. One real release
must pass the new gate before calling release acceptance complete.

## Requests and reports

Create `main-validation-request` only with approval. An issue with that label and
a JSON body containing `source` and optional `release_tag` survives pending-run
replacement. Do not edit a running request to select another source. Close it
only after its exact-source result or after rejecting an invalid request with an
explanation. To retry a completed source, open a new request. Full-workflow reruns
repeat selection and can choose a newer main or skip completed work.

Read the exact-source commit status, job results, Linux/APFS artifacts and all
CTRF files. Do not substitute an ancestor's green status. Reports retain original
failed attempts and recovered tests; checks without CTRF use the job log. An
interrupted or missing result is never green. The named owner, Daniel Vigovszky,
reads these manually until the analysis agent is accepted.

## Analysis agent

[observer/PROMPT.md](observer/PROMPT.md) owns the reusable instructions.
[observer/README.md](observer/README.md) documents account-independent setup,
the 30-minute wake-up, credentials, state transfer and prompt updates. The helper
scripts live beside the prompt. Keep receipts and seen markers in durable local
state, not Git. Updating these files does not update an existing observer's
checkout or schedule automatically.

The current observer is configured under Daniel's personal Amp account in
[its own thread](https://ampcode.com/threads/T-01a1e9ca-bfca-772d-aa93-7feaf0270dc1).
Its existing 30-minute schedule reads the repo-owned prompt from a pinned checkout.
The schedule holds the operator-specific checkout revision, state path and acceptance
destination. Generated receipt state is separate from the repo-owned helpers.
The [public dashboard](https://golem-ci-status--vigoo.onamp.dev/) runs on the
always-on `vigoo/golem-ci` Box, independently of the observer orb. Slack reports
link to the corresponding run attempt on that dashboard.
The observer is not yet accepted on real validation runs.
Accept one failed and one green run, including a repeated wake-up which must not
double-post. `.github/tools/flaky-tests` also needs real report-artifact acceptance
before cutover.

## Coordinated cutover (not performed)

Under an approved merge pause, enable production sampling, remove main/PR full
suite triggers from `ci.yaml` and `builtin-bash.yaml`, and retain manual/reusable
CI and full CI on maintenance branches (`N.N.x`). Extract the cheap path-filtered
skill-harness PR checks. Split required checks so main keeps CLA while maintenance
lines keep today's full checks. Update contributor/agent guidance and remove the
three experiment branches only with authorization. Verify a real main merge,
PR and Slack report before calling the ticket complete. The own machine and
parked outer-VM scripts follow later; they are not prerequisites for Blacksmith.
