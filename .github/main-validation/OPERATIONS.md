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

## Analysis agent (not configured)

Use a new small Golem orb under Daniel's personal Amp account, separate from the
daily benchmark orb. Follow that benchmark agent's schedule and Slack webhook
mechanisms: a 30-minute schedule wakes this same thread; an incoming webhook
posts to `#golem-dev-internal`. Resolve webhook credentials through Amp secrets,
never the prompt or CI. No health issue, investigation orbs or reproduction
builds are needed initially. Obtain authorization before setup or posting.

Proposed instructions for that thread:

> Read newly finished `main-validation.yaml` runs in `golemcloud/golem` with `gh`,
> including the exact source in the selection summary, job conclusions and CTRF
> reports. When nothing is new, end with one short sentence. For each new failure,
> report the source, failed tests/checks, recovered tests, run link and evidence
> limits. Read `git log` and relevant `git show` diffs between the last comparable
> green source and this failure. Rank credible suspect commits with confidence,
> or say that no commit is a credible cause. Distinguish findings from hypotheses.
> Analyze in this thread without investigation orbs or reproduction builds.
> Post once per run through the configured Slack incoming webhook: a new failure
> with analysis, a repeated failure in one line, or a recovery. Green runs without
> recovery stay silent. Before posting, exclusively create a receipt named by
> the run ID under `.amp/in/orb-backup/`; if it exists, inspect thread history and
> delivery evidence instead of reposting. If delivery is uncertain, report that
> uncertainty here. Never push, trigger workflows or change infrastructure from
> this reporting task.

Accept the agent on one failed and one green real run, including a repeated wake
which must not double-post. `.github/tools/flaky-tests` still needs adaptation
for this workflow and in-report retries before cutover.

## Coordinated cutover (not performed)

Under an approved merge pause, enable production sampling, remove main/PR full
suite triggers from `ci.yaml` and `builtin-bash.yaml`, and retain manual/reusable
CI and full CI on maintenance branches (`N.N.x`). Extract the cheap path-filtered
skill-harness PR checks. Split required checks so main keeps CLA while maintenance
lines keep today's full checks. Update contributor/agent guidance and remove the
three experiment branches only with authorization. Verify a real main merge,
PR and Slack report before calling the ticket complete. The own machine and
parked outer-VM scripts follow later; they are not prerequisites for Blacksmith.
