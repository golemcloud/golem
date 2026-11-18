# Acceptance and operations

The cutover code makes production sampling and the exact-source release gate
permanent when merged. It removes the activation variables and full reusable-CI
release fallback. Preparing this change does not merge it or change live settings.
Do not dispatch workflows, create request labels/issues/statuses, post to Slack
or change rulesets without separate authorization.

## Blacksmith acceptance

1. Run a cold serial cycle from the feature branch in acceptance mode. Compare
   its CTRF test identities and skips against green ordinary CI on that same
   source. Account separately for checks which do not emit CTRF.
2. Run a warm acceptance cycle with `parallel=true`. Measure duration, peak memory
   and disk. Check ports, Redis, files and containers across all three subflows.
   Do not call overlap accepted if this fails; production uses the existing
   parallel flow, so an unresolved isolation failure blocks safe cutover.
3. Exercise a persistent test failure and a recovered flaky test. Confirm two
   failed-test retries, the final result and retained original failures.
4. Cancel a genuine run, then verify the next run can use the sticky disks and
   managed-XFS VM cleanup. The development orb has no KVM and cannot prove this.
5. Exercise sampling: merges B and C during A select C next; unchanged completed
   main starts no workload. A valid request goes first. An invalid request is
   closed and does not stop sampling. API failures do not close valid requests.
6. Verify APFS succeeds on the same source. Measure the provisional 300-minute
   workload and 330-minute Linux job limits before finalizing them.

Acceptance uses `Main validation (acceptance)`, not the release status. Feature
branch acceptance never satisfies release preparation. Replacement acceptance
run [44267744093](https://github.com/golemcloud/golem/actions/runs/44267744093)
uses the already-pushed source `dbf3b9cdd`; it is not evidence for later local
cutover edits. Do not cancel or replace it as part of preparing this change.

Local fixtures execute the real selection, result and release-gate shells with
offline API responses. They cover missing/pending/failed statuses, wrong creators,
acceptance-only success and publication after failed or skipped prerequisites.
They do not establish real Blacksmith test identities, overlap isolation, cold/warm
resource limits, sticky-disk cancellation recovery or same-source APFS completion.
Record the real acceptance outcomes separately, including any outstanding cases.
The exact frozen preparation head must pass the new gate in one real release
before release acceptance is complete. Signing and publication are not exercised
by offline fixtures.

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

## Coordinated cutover (live changes not performed)

Under an approved merge pause, coordinate protection changes with the cutover
merge. The live ruleset `16972918`, named `main`, currently applies jointly to
`~DEFAULT_BRANCH`, `release-*` and `N.N.x`. Its 62 required statuses include
`CLAAssistant` and legacy full-CI jobs. Do not remove the full-CI requirements from
that shared ruleset wholesale: maintenance branches must retain them.

Split main protection from release/maintenance protection, or make an equivalent
scoped ruleset update. Main must no longer require jobs removed by the cutover;
release and maintenance protection must not lose their existing full-CI checks.
Preserve reviews, CLA, bypass policy, deletion restrictions and non-force-push
safeguards. Keep Vouch, docs previews and billing deployment safeguards.
Do not require `Main validation` on each PR: it samples main after merge and does
not validate every PR or intermediate main commit. Remove main PR requirements
for skill-harness, billing tests and Bash oracle checks; those do not run on main PRs.

The merged code activates the ten-minute schedule without a variable update and
stops automatic full-platform/heavy Bash checks on ordinary main PRs and pushes.
Main PRs run no tests, including skill-harness, billing and Bash oracle tests.
Main removes the legacy platform and built-in Bash workflows, including their
manual/reusable entry points. Existing maintenance branches retain their own
copies; new branches from main do not inherit them. The existing main-push
billing and Bash oracle paths remain. No optional PR tests are introduced.
Release preparation always requires the exact-source production result and
release-only prerequisites. Existing activation variables no longer control it.

Verify a real main merge, unrelated PR, skill-only PR, maintenance PR/push and
Slack report before calling the ticket complete. Confirm there is no duplicate
automatic full-platform flow and that removed checks no longer block main PRs.
Daniel Vigovszky owns outages and merge/release pauses; do not automatically
restore paid full PR CI during an outage. Update contributor/agent guidance with
the same merge. Branch deletion requires separate authorization. The own machine
and parked outer-VM scripts follow later; they are not prerequisites for Blacksmith.
