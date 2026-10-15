# PR #3894 feedback checklist

[PR: Unify account resource limits](https://github.com/golemcloud/golem/pull/3894)

Updated from the posted GitHub review, comments and inline thread on 2026-10-15.

Checked means implemented and locally validated. It does **not** mean pushed, accepted by the reviewer
or resolved on GitHub. Partial and deferred requests stay unchecked. This file is the current
feedback tracker; older investigation notes are history, not the current status.

## At a glance

| Feedback | Local status |
| --- | --- |
| Findings 1-9 and 12-14 | Done locally, within the limits below |
| Findings 10-11: test behavior and Worker-owned orchestration | Partial; several complete control families removed, others remain |
| Later request to remove the general lifecycle redesign | **Open; not satisfied merely by preserving Worker/WorkerInstance** |
| CLI command-handler organization | Open |
| Plan/grant precedence proposal | Deferred by user decision; existing replacement precedence unchanged |
| Delivery of the repairs | Checkpoint records completed local work; full feedback resolution and review replies remain pending |

The latest coherent code passed the selected body/native checks, 571 affected library checks,
207 integration checks, strict Clippy and the normal build without `test-utils`. The aligned
walkthrough passed offline browser checks. This validates the completed local scope, not all
remaining feedback or final MVP acceptance. No build, test or agent is running.

## Additional posted requests

- [ ] **A1. Remove the general worker lifecycle redesign from this PR.** Open.
  [Vigoo's later architectural request](https://github.com/golemcloud/golem/pull/3894#issuecomment-6169447422).
  - [ ] Review the changes to general interruption, outcome selection, startup, generation
    coordination and cleanup against existing main. Remove changes not required for monthly enforcement.
  - [ ] For each broader fix retained here, demonstrate its concrete dependency. Distinguish an
    existing-main defect, required monthly integration, intermediate regression and unnecessary
    stronger guarantee. Separate independent executor fixes where no dependency requires them here.
  - [ ] Check that billing does not impose exact ordering between quota detection and successful
    invocation completion, or couple permits/billing to filesystem deletion solely for billing.
  - Keep the policy/accounting/API work, independent capacity detection, stale-window checks,
    admission checks and targeted host-interruption fixes. Use existing lifecycle owners.
  - Completed control removals below help simplification but do not close this architectural request.

- [ ] **C1. Follow the CLI command-handler organization.** Open.
  [noise64's inline comment](https://github.com/golemcloud/golem/pull/3894#discussion_r4270447747).
  `cli/golem-cli/src/command_handler/account.rs` still handles several command levels in one module.
  Use a separate CommandHandler/module for each level and `cmd_*` methods for handler entry points,
  or provide a specific justification. The inline thread is still unresolved.

- [ ] **G1. Consider `max(plan amount, grant)` instead of unconditional grant replacement.** Deferred.
  [Vigoo's grant-precedence comment](https://github.com/golemcloud/golem/pull/3894#issuecomment-5710615586).
  The user deferred this decision. Do not implement a precedence change or call the proposal fixed.
  An active grant still replaces the plan amount, including after a plan upgrade.

## Numbered review findings

Findings 1-10 are in [Vigoo's review](https://github.com/golemcloud/golem/pull/3894#pullrequestreview-5517289944).
Findings 11-14 have separate linked comments.

- [x] **F1. A temporary unavailable policy must not permanently poison the cached Worker.**
  Operational policy failure is separate from cleanup health. Fail-closed behavior remains while
  policy is unavailable. After valid policy and successful disposal, the same cached Worker can
  execute again. Genuine cleanup failures still prevent startup.

- [x] **F2. Retain entered blob mutations and quota settlement through interruption.**
  Service-owned work retains the original mutation lock, inputs and reservation through the result.
  Unknown outcomes retain failure and ownership rather than refunding an uncertain effect.
  Primary-host and actual ComponentEntityRunner completion/cancellation/recovery checks pass.
  Process loss and partial backend outcomes are not an external exactly-once guarantee.

- [x] **F3. Preserve blob-only usage during month and policy-revision rollover.**
  Both activity detection and capture retain blob deltas even without byte-time remainders or enabled
  monthly meters. This fixes loss before delivery, not ambiguous Registry delivery.

- [x] **F4. Use composed interruption for the new P2 waits.**
  Native waits include invocation/tail-work deadline latches and entity cancellation. Tests cover
  cancellation before subscription and during actual Pending, with typed errors and real resources.

- [x] **F5. Deduplicate live flusher registrations.**
  Registration prunes dead entries and deduplicates the same live flusher. Tests check actual retained
  entries and callbacks, not only a distinct-flusher projection.

- [x] **F6. Reject finite storage policy on an incapable backend.**
  Single-binary validation checks storage capability independently of billing. Split-service startup
  and live updates reject finite policy on unaccounted storage instead of enforcing Unlimited.
  Dynamic Registry discovery of executor-fleet capabilities is not implemented.

- [x] **F7. Return active administrator-grant metadata from owner PUT/DELETE.**
  Memory and storage mutation responses include the active grant and its resolved value. API coverage
  includes owner overrides combined with grants. Grant precedence is unchanged.

- [x] **F8. Sample live grant expiry after acquiring the account lock.**
  Live resolution no longer uses a stale pre-wait timestamp. PostgreSQL contention coverage checks
  expiry during the lock wait. Explicit historical reads retain their supplied time.

- [x] **F9. Do not charge executor-driven reconstruction as live customer work.**
  Live accounting is separate from materialization and replay. Comparisons cover compute, memory
  and filesystem accounting while retaining physical admission, safety and legitimate live storage
  charges. Allocation-rate wakeups and the scoped RPC refund/rejection/recovery checks pass.
  Scripted filesystem evidence is not native managed-XFS or invoice-level validation.

- [ ] **F10. Remove test-only runtime machinery that changes async behavior.** Partial.
  [Posted finding](https://github.com/golemcloud/golem/pull/3894#pullrequestreview-5517289944).
  - [x] Remove TCP-connect, HTTP and splice pre-subscription controls.
  - [x] Remove outcome pauses, publication-wait observation and private lifecycle-selector mutation.
  - [x] Remove close-seal and invocation-admission pauses.
  - [x] Remove the complete raw-driver pause/panic family.
  - [x] Remove the complete stop-publication pause/panic family.
  - [x] Remove the complete stop-freeze pause/panic family.
  - [x] Remove the complete monthly-acceptance pause gate.
  - [x] Preserve causal tests of actual production bodies plus separate real-Worker effects, markers,
    receipts, accounting, physical cleanup and ordinary recovery. No sleeps, retries, lower concurrency
    or weaker assertions substituted for these removed controls.
  - [ ] Finish the teardown-fence family. The admission-marker/monitor-election conjunction still
    needs a faithful replacement; pre-commit append backpressure is not that checkpoint.
  - [ ] Address remaining idle-close, completed-replay and deletion-stage gates.
  - [ ] Address monitor/actor/settlement fault controls, monthly time and invocation-deadline controls.
  - [ ] Address the keyed filesystem metadata gate and remaining protocol ordering controls.
  - [ ] For any hook proposed to remain, document the otherwise untestable guarantee, minimal scope
    and why its scheduling changes do not invalidate the test. Existing main-owned hooks are not
    automatically PR-introduced defects.

- [ ] **F11. Move scenario-specific orchestration out of Worker ownership.** Partial.
  [Posted finding](https://github.com/golemcloud/golem/pull/3894#issuecomment-6167616697).
  - [x] Delete the obsolete TCP-connect/HTTP control modules and their Worker state/interfaces.
  - [x] Delete both P2 input/poll observer modules and their Worker fields, setters and consumers.
    P2 tests use test-side capture of ordinary production tracing.
  - [x] Delete the keyed durable-source observer registry, runtime capture field and producer wiring.
    Actual read-body tests cover Pending/ordinal/Drop and commit ownership. The native counterpart
    retains exact causes, raw prefix, accounting and accepted-session recovery.
  - [x] Add user-approved bounded test-only capture of the existing result-wait span. It establishes
    session startup ordering, not source Pending, and adds no production hook or diagnostic span.
  - [ ] Rework remaining Worker-owned P3 UDP, RPC and WebSocket scenario support.
  - [ ] Address monthly-acceptance observers/completion projections, idle/tail observations,
    private lifecycle projections, scripted allocation support and test-only entity construction.
  - [ ] Keep shared support at real dependency boundaries or in test-utils where appropriate.
    Merely moving files or relocating the same runtime hook does not close this finding.
  - Deferred WebSocket replay and Git partial-download investigations remain out of scope.

- [x] **F12. Replace new "quiesced/quiescence" terminology with explicit words.**
  [Posted finding](https://github.com/golemcloud/golem/pull/3894#issuecomment-6167634525).
  PR-introduced primary-execution names, metrics, tests and explanations use explicit wording.
  Execution stopping, Store destruction and physical resource release remain distinct.

- [x] **F13. Remove test-feature conditionals around useful diagnostics and delete temporary logs.**
  [Posted finding](https://github.com/golemcloud/golem/pull/3894#issuecomment-6167685924).
  Temporary investigation logs/state are removed. Retained diagnostics use ordinary structured
  Rust tracing and the existing OpenTelemetry/OTLP path. Custom capture is test-only; there is no
  new deployed tracing backend, registry, exporter or transport. This does not close F10/F11.

- [x] **F14. Use directory modules when a module gains children.**
  [Posted finding](https://github.com/golemcloud/golem/pull/3894#issuecomment-6167824681).
  The expanded parent modules use `x/mod.rs` with children under `x/`. Source/document references
  are updated. File layout does not substitute for fixing orchestration ownership.

## Agreed limits, not uncompleted review fixes

The numbered review explicitly excluded these MVP limits. Keep them documented; do not claim they
were fixed as part of the completed items above.

- Registry delivery can lose usage or refunds. Safe recovery remains in
  [GOL-847](https://linear.app/golem-cloud/issue/GOL-847/recover-resource-usage-safely-after-registry-delivery-failures).
- Prepaid fuel can retain the borrow revision/month. The default remainder is per Store, not a
  universal overrun bound.
- Chronological allowance allocation across overlapping revisions remains in
  [GOL-848](https://linear.app/golem-cloud/issue/GOL-848/preserve-chronological-allowance-allocation-across-overlapping-policy).
- Applied `allowOverage` usage stays eligible until `hardLimit` reaches the executor normally.

## Delivery checklist

- [ ] Finish or explicitly agree the disposition of A1, C1 and the remaining F10/F11 work.
- [ ] Obtain a decision before changing G1 grant precedence.
- [x] Prepare the validated code, documentation and this checklist for a checkpoint commit.
- [ ] Reconcile the PR body and feedback replies against this checklist. Drafts exist but are unposted.
- [ ] Push/post/resolve threads only with authorization. Do not change GOL-442 status or claim MVP acceptance.

The user authorized committing and pushing the current checkpoint, then continuing the remaining
engineering work. Grant precedence stays unchanged until the other work is complete and the user
chooses the policy. Checkpoint publication does not imply reviewer acceptance or thread resolution.

## Source coverage

This list covers the substantive human feedback: the review containing findings 1-10, the four
numbered follow-up comments, the later lifecycle request, the grant-precedence comment and the
CLI inline thread. GitHub's inline thread remains unresolved; this checklist does not resolve it.

Deployment-preview bot messages require no code action. The
[older Blacksmith failure report](https://github.com/golemcloud/golem/pull/3894#issuecomment-6046016925)
is not treated as a fresh review request or a current CI verdict. CI watching remains stopped.
