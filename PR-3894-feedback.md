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
| CLI command-handler organization | Done locally; focused tests and strict lint pass |
| Plan/grant precedence proposal | Deferred by user decision; existing replacement precedence unchanged |
| Delivery of the repairs | Checkpoint records completed local work; full feedback resolution and review replies remain pending |

The completed executor scope passed the selected body/native checks, 593 affected library checks,
278 integration checks, strict Clippy and the normal build without `test-utils`. The aligned
walkthrough passed offline browser checks. The later CLI handler reorganization passes the same
17 focused tests before/after and strict lint for all CLI targets with server-command support.
This validates the completed local scopes, not all remaining feedback or final MVP acceptance.

The rebuilt host-API fixture passes its native consumers. An additional fixture-wide strict Clippy
run reports three `await_holding_refcell_ref` diagnostics in unchanged WebSocket methods. Those
method bodies are byte-identical to the prior validated input. They are not suppressed or reported
as passing; the scoped executor Clippy result above is separate.

## Revised delivery plan

[PR-3894-delivery-plan.md](PR-3894-delivery-plan.md) replaces the previous incremental repair queue.
The user confirmed that the full recovery report's conclusions are being implemented now, not merely
proposed for later. [PR #4207 / GOL-920](https://github.com/golemcloud/golem/pull/4207) is the active
implementation, now targeting main. #4199, #4198/GOL-910 and #4212/GOL-898/GOL-897 are merged.
#4207 is now ready for review and its selected integration head includes main and #4212's stdin
fixes. The user explicitly authorized local integration of that pinned revision. The plan targets
those contracts and excludes further repairs to the P2 HTTP ownership chain, atomic/global-idempotence
and suffix-rollback machinery being removed. Host P3 HTTP retry, transaction settlement and
non-HTTP P2 support still require their surviving guarantees.

The user has authorized independent implementation before #4207 lands, without importing that
branch or resuming obsolete recovery repairs. The surviving success boundary is now locally
validated, including unchanged native assertions and strict lint. A stale background build record
was not accepted as evidence; a fresh foreground build and explicit test executions supplied the
results. Direct inspection of #4207 confirms the boundary and commit-before-notify still exist.
These receipts cover their original branch trees, not the #4207 port. Local integration is now
authorized; push, review replies and tracker changes remain unauthorized. Grant precedence stays
unchanged and deferred.

## Additional posted requests

- [ ] **A1. Remove the general worker lifecycle redesign from this PR.** Open.
  [Vigoo's later architectural request](https://github.com/golemcloud/golem/pull/3894#issuecomment-6169447422).
  - [ ] Review general interruption, outcome selection, startup, generation and cleanup against
    the surviving owners on the chosen recovery revision. Remove changes not required for monthly
    enforcement; do not restore discarded old-main machinery.
  - [ ] For each broader fix retained here, demonstrate its concrete dependency. Distinguish an
    existing-main defect, required monthly integration, intermediate regression and unnecessary
    stronger guarantee. Separate independent executor fixes where no dependency requires them here.
  - [ ] Remove unnecessary exact ordering between quota detection and successful invocation completion.
  - [x] Restore main's execution-window settlement and permit release before filesystem deletion.
    Failed deletion retains its actual filesystem reservation, repair owner and physical-completion
    receipt, not a closed billing window or execution permit. Initial/later native permission cases
    prove capacity returns while the failed root exists and startup still refuses structural failure.
  - [x] Remove the duplicate frozen-allocation disposal path and its retained metering state.
    Ordinary close owns final sampling, settlement and its existing deadline. Main's final-read
    retries and customer-favoring timeout remain; genuine active-window enforcement faults still
    survive successful physical cleanup. Exact charge and reconstruction assertions are preserved.
  - Keep the policy/accounting/API work, independent capacity detection, stale-window checks,
    admission checks and targeted host-interruption fixes. Use existing lifecycle owners.
  - Completed control removals below help simplification but do not close this architectural request.
  - Next: reduce the executor delta against the active #4207 contracts and use surviving interruption
    and unload owners. Do not restore old-main files wholesale or resurrect removed recovery paths.
    The completed narrow corrections do not by themselves close A1.
  - [x] Remove failure-origin arbitration and its extra callback. Failure keeps its actual boundary
    cause without waiting for private stop publication. The ordinary typed callback acknowledges
    only an exact matching, published terminal request. All native assertions remain unchanged;
    body/native checks, strict lint, production build and aligned browser checks pass locally.
  - [x] Restore the ordinary terminal check before success finalization. Remove the later success
    arbiter and private-publication wait. A later stop cannot replace an already-decided success;
    commit-before-notify, exact identities, history and native cleanup assertions are preserved.
  - [x] Remove the private selected-result slot, writer receipts/verdicts and stop-side writer join.
    Ordinary callbacks own writes and commit-before-notify. Stop signalling may overlap an append.
    Invocation exit uses FIFO-folded status to avoid recording the same outcome twice. Native
    failure, timeout, Suspend, fenced-prefix, completion, cleanup and reconstruction checks pass.
    Only discarded private-writer projections and stricter signal ordering were retired; exact
    outcome/history assertions stayed. Stop publication, admission seals and early generation
    handoff remain open, so this does not close A1.

- [x] **C1. Follow the CLI command-handler organization.** Done locally.
  [noise64's inline comment](https://github.com/golemcloud/golem/pull/3894#discussion_r4270447747).
  Account usage, limits and permission shares have separate named CommandHandlers under
  `cli/golem-cli/src/command_handler/account/`, with `cmd_*` entry methods and the existing Context
  handler factories. Account selection, service calls, permissions, revisions, output and grant
  precedence are unchanged. Existing dispatch assertions are preserved. The GitHub thread remains
  unresolved; this checkbox records local completion only.

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
  - [x] Remove the monitor panic token, selector arm and Worker panic setter. Native tests fault
    the actual weak usage-flusher dependency; all health assertions remain byte-identical. The
    shared actor-exit observation and other fault/time controls remain open.
  - [x] Remove WebSocket reader-order gates and their complete Worker module. An own-agent guest
    promise and real native-frame/lock observations preserve all original native assertions. Promise
    ownership, reader locks, timeout start and recovery behavior remain unchanged.
  - [x] Remove the idle-after-success observer, native-idle wrapper and pending/committed-writer
    projections with the discarded result protocol. Native setup uses completed invocation and
    actual execution-permit release, not an invented idle checkpoint.
  - [x] Preserve causal tests of actual production bodies plus separate real-Worker effects, markers,
    receipts, accounting, physical cleanup and ordinary recovery. No sleeps, retries, lower concurrency
    or weaker assertions substituted for these removed controls.
  - [ ] Remove teardown-fence controls with the discarded generic protocol. Preserve surviving
    terminal/history/physical-cleanup guarantees; do not rebuild the old private-election conjunction
    if its production state machine is removed. Pre-commit append backpressure is not that checkpoint.
  - [ ] Address remaining idle-close, completed-replay and deletion-stage gates.
  - [ ] Address remaining actor/settlement fault controls, monthly time and invocation-deadline controls.
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
  - [x] Remove Worker-owned RPC-result and P3 UDP observer modules, fields, methods and producer
    branches. Tests collect actual native-future state through existing tracing. UDP keeps actual
    socket-binding and rebind proofs; production target identity checks and recovery remain unchanged.
  - [x] Remove the passive WebSocket handshake, reconnect-pool and timed-receive observer families.
    Their native tests use existing tracing at the same futures, with all native assertion payloads
    preserved except observation enum/path names. No recovery repair is included.
  - [x] Remove the remaining WebSocket reader-lock observer with its ordering gates. Existing tracing
    supplies actual Pending/Returned/Dropped and exact connection binding, without Worker test state.
  - [ ] Address the keyed filesystem gate; oplog backpressure is not actual metadata Pending.
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

- [x] Write the delivery plan against the active recovery implementation, with explicit overlap,
  ownership, scope exclusions and exit gates. This is planning, not feedback resolution.
- [ ] Close A1 and the remaining F10/F11 work against the surviving contracts; retain C1's validated
  handler organization. Do not mark findings resolved merely because an upstream change exists.
- [ ] Obtain a decision before changing G1 grant precedence.
- [x] Commit and push the validated executor checkpoint and this checklist:
  [`dd23543175`](https://github.com/golemcloud/golem/commit/dd23543175e51ac86c4c528df99ce39d1de06326).
  The earlier checkpoint was pushed with it. C1 and the A1 window-close corrections are subsequent
  validated local work, not yet pushed.
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
