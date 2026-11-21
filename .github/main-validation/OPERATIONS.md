# Acceptance and operations

The cutover code makes production sampling and the exact-source release gate
permanent when merged. It removes the activation variables and full reusable-CI
release fallback. Preparing this change does not merge it or change live settings.
Do not dispatch workflows, create request labels/issues/statuses, post to Slack
or change rulesets without separate authorization.

## Scheduling and tooling qualification

SDK preparation produces packages and templates before their consumers. Lint,
formatting, drift and test verdicts run independently on CI (`CI=true`) and still
fail the final result. Local validator invocations fail fast, even with the `ci`
profile. A failed producer blocks only consumers admitted by its current-cycle
status. Final checks use five static lanes after every stateful lane joins;
managed XFS and Docker follow their join serially. Preserve this barrier when
tuning the tail, including when a stateful lane fails.

Solo preparation uses 32 Cargo jobs. Service compilation retains 20 while it
overlaps worker tests. Final lanes use four Cargo/native harness workers, with
separate Bash and Rust SDK targets. Compare acceptance `build-jobs=20` and `32`
on the same source before claiming a compilation speedup. Measure disk and
memory contention as well as CPU use.

The flow and observer helpers now use the pinned TypeScript package. Install
dependencies in both selected and control checkouts; keep each helper's imports
beside its own package. Shared generators, Bash conformance and Docker inspection
still require Python. Dashboard deployment must ship the bundled server and Bun,
not just the TypeScript source. Coordinate the observer checkout, its prompt pin
and the dashboard deployment with the merge, using the reporting rollout below.
Existing summaries do not acquire new layout unless deliberately republished.

### Reporting rollout after merge

1. Wait for the approved changes to merge. Let any in-flight observer analysis
   finish, and retain the existing owner, credentials, state path, seen markers,
   webhook configuration and delivery receipts.
2. Update the existing observer checkout and operator prompt pin to the merged
   revision. Install the pinned TypeScript dependencies. Do not create another
   observer or resend previously processed reports.
3. Build and deploy the dashboard bundle and Bun from that same revision. Retain
   the existing data directory and separate observer/progress credentials.
4. Verify the deployed health/read endpoints, then confirm readable new summaries
   and duplicate suppression on naturally completed validation. Do not dispatch
   validation, republish old summaries or post test Slack reports as part of this
   rollout without separate authorization.

Cache evidence includes visible-cycle markers before and after the workload,
disk capacity and device identity, plus fixture input-group digests and mutation
diagnostics. Compare each restore against the preceding completed cycle and the
provider's snapshot evidence. A completion marker precedes the provider commit
hook and does not prove a durable save. The three repaired fixture lockfiles
must remain unchanged during real builds before completed-WASM reuse is accepted.
Skill inputs remain conservative because the CLI embeds and synchronizes them.
Do not restore checkout timestamps or narrow keys without a discriminating build
experiment. Local fixtures do not establish real warm hits or provider durability.

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
the completion webhook handoff, credentials, state transfer and prompt updates. The helper
scripts live beside the prompt. Keep receipts and seen markers in durable local
state, not Git. Updating these files does not update an existing observer's
checkout or trigger automatically.

The current observer is configured under Daniel's personal Amp account in
[its own thread](https://ampcode.com/threads/T-01a1e9ca-bfca-772d-aa93-7feaf0270dc1).
Its operator instructions pin the checkout revision, state path and acceptance
destination. Preserve those values and the existing receipts when adopting the
completion webhook. Generated receipt state is separate from the repo-owned helpers.
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
