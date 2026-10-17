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

Before upstream integration, the completed executor scope passed the selected body/native checks, 593 affected library checks,
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
proposed for later. [PR #4207 / GOL-920](https://github.com/golemcloud/golem/pull/4207) is now merged
into main, alongside #4199, #4198/GOL-910 and #4212/GOL-898/GOL-897. The user directs this branch to
current main; #4207 is no longer a separate pinned dependency or handoff gate. The plan targets
those contracts and excludes further repairs to the P2 HTTP ownership chain, atomic/global-idempotence
and suffix-rollback machinery being removed. Host P3 HTTP retry, transaction settlement and
non-HTTP P2 support still require their surviving guarantees.

The user first authorized independent implementation before #4207 lands. The surviving success boundary is locally
validated, including unchanged native assertions and strict lint. A stale background build record
was not accepted as evidence; a fresh foreground build and explicit test executions supplied the
results. Direct inspection of #4207 confirms the boundary and commit-before-notify still exist.
These receipts cover their original branch trees, not the #4207 port. Local integration is now
authorized; push, review replies and tracker changes remain unauthorized. Grant precedence stays
unchanged and deferred.

The validated recovery integration is checkpointed locally. The update against fetched main has
resolved conflicts and passes the selected library, native socket/HTTP, lifecycle, metering and
CLI/client checks, including the new TLS helpers. Strict all-target lint and the production binary
checks pass. One existing privileged-XFS case requires its managed test profile
and was not executed in this shell; it is not counted as covered.
The user has authorized command-scoped hook bypasses for local commits in this task because the
lefthook entry is obsolete. Shared Git configuration is unchanged, and no push is authorized.
The merge keeps #4212's consumer and producer cancellation branches. Removed P2 HTTP producers
and their dedicated tests are retired together. Non-HTTP P2 support and P3 HTTP retry remain.
Monthly HTTP/TCP expectations now use the original Starts, exact typed results and full raw-prefix
checks instead of suffix repair. The canonical wallet installer resets the owner generation once;
the early tool-admission reset is removed. The broader retained-change review remains open A1 work.

The next main integration includes #4195's Rust custom scopes and owned async finish, plus #4220's
owned accessor Starts and dropped-wait recovery. The merge retains InterruptSources and both tool
test modules. Rebuilt host-API, HTTP and Rust tool fixtures validate against the combined SDK/WIT.
Guarded native batches pass the upstream custom-scope/recovery proofs, dropped-wait restart cases
and changed HTTP/clock cancellation cases. New production-Context promise tests also pass automatic
suspension and a monthly-memory stop during an accepted custom End. Additional stopped-prefix and
generation assertions now pass in both completion orders. A final-cohort timeout first led to a
reverse-order regression that reproduced a test-targeting bug: the function-name
gate selected the other sibling's Start, so the intended End completed instead of remaining held.
The correction binds the same oplog fixture to the exact recorded Start after both waits enter, with
no Worker hook or runtime change. Both completion orders and the existing gate consumer pass after
that correction. Failed baselines remain recorded. Combined library, strict executor/test-utility
lint, production compilation and offline browser checks pass on the integrated source. This is a
local integration checkpoint, not completion of the open feedback. The guest-drop plus quota-stop conjunction for an owned
Start remains a separate gap, as do the existing A1/F10/F11 dispositions. Details are in
[the custom-scope integration assessment](PR-3894-pr4195-impact.md).

The combined library scope and native P2/P3 socket and P3 HTTP cases now pass. Raw-socket fixtures
explicitly opt into main's new socket setting; the production security default is unchanged. The
permission-denial library test runs without root's DAC-bypass capabilities and keeps its original
backend assertion. Lifecycle composition, accounting/stream checks, production compilation without
`test-utils`, strict CLI lint and account checks pass too. The remaining TCP expectations require
exact ConnectionBroken rather than the removed InvalidState fallback. This preserves the typed-loss
contract without accepting arbitrary errors or weakening identity, history or cleanup assertions.

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
  - Next: reduce the executor delta against main and use surviving interruption
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
    outcome/history assertions stayed. The separate establishment restoration is recorded below;
    this result-writer removal alone did not close A1.
  - [x] Separate monthly-window failure ownership before deleting the generic stop registry.
    The existing monthly target stores monitor failure and operational policy-outage data,
    rather than adding synthetic completed stop futures. A cleanup fault
    cannot be erased by policy recovery; an outage alone cannot become failed cleanup. Native health
    assertions are unchanged. Focused and consolidated library/native checks, strict lint and the
    production check pass locally. The registry removal below has its own validation.
  - [x] Restore and validate ordinary interruption establishment. The local change deletes
    `StopPublication`, `StopAdmissionSeal`, the stop-task vector, release successor and retired flag.
    It uses main's independently owned establishment, request states and startup tail check. Monthly
    acceptance keeps its exact capacity/owner checks; actual window health and cleanup retain their
    existing owners. Loading and permit-wait cancellation remain required monthly composition.
    Frozen/leader/seal projections retire with their protocol. The native tests retain their actual
    result, effect, identity, raw-history, terminal, accounting, cleanup and reconstruction assertions.
    Combined library/native scopes, real entity-blob ownership and repair, ordinary deadline/no-Store
    cases, strict all-target lint, production compilation and browser checks pass locally. Source
    receipts retain the initial failures and each correction; none is waived. This does not close
    the broader retained-change review in A1. Existing teardown and other F10/F11 controls remain
    explicitly test-only and unfinished. Managed-XFS and the genuine metadata-Pending proof remain
    outside this evidence.

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
    actor-loss family is addressed separately below; time controls remain open.
  - [x] Remove WebSocket reader-order gates and their complete Worker module. An own-agent guest
    promise and real native-frame/lock observations preserve all original native assertions. Promise
    ownership, reader locks, timeout start and recovery behavior remain unchanged.
  - [x] Remove the idle-after-success observer, native-idle wrapper and pending/committed-writer
    projections with the discarded result protocol. Native setup uses completed invocation and
    actual execution-permit release, not an invented idle checkpoint.
  - [x] Preserve causal tests of actual production bodies plus separate real-Worker effects, markers,
    receipts, accounting, physical cleanup and ordinary recovery. No sleeps, retries, lower concurrency
    or weaker assertions substituted for these removed controls.
  - [x] Remove teardown-fence controls with the discarded generic protocol. Preserve surviving
    terminal/history/physical-cleanup guarantees; do not rebuild the old private-election conjunction
    if its production state machine is removed. Pre-commit append backpressure is not that checkpoint.
    The local change removes the whole field/type/setter/await family after native consumers
    pass without calling it. Real pending-writer FIFO release, later-start identity/error/marker
    checks and compute-admission Counter effects remain strict. Artificial pre-fence snapshots,
    forced await-ready Pending and the release-before/after-join matrix retire with the pause.
    The admission case adds a complete stopped raw-prefix check. Separate post-removal library,
    affected native and custom-suspension/acceptance regression batches, strict executor lint,
    production compilation and browser checks pass. This completes this control family only;
    overall A1/F10/F11 remain open.
  - [ ] Address remaining idle-close and deletion-stage gates.
    The local idle-health narrowing removes both monitor cells' private idle holds. Initial
    readiness failure uses a real close-time usage-flusher fault; later monitor failure uses its
    selected Finished append and still requires the original successful result. The existing idle
    barrier remains only for actor loss after an already-published result. Moving that fault to an
    uncommitted writer produced a real caller error, so it is not an equivalent replacement and
    the success assertion is not relaxed. Exact physical-root deletion, retained error, startup
    refusal, identity, generation, permit and history proofs remain. No new runtime hook or append
    registry is added. The narrowed helper, unchanged monitor/cleanup/after-End collateral,
    strict integration/test-utils lint and formatting pass. This is a proposed narrow retention,
    not complete family removal or reviewer acceptance; this checkbox remains open. The barrier
    delays natural idle close only after successful publication and makes no normal timing claim.
    The separate unused Boolean panic selector is removed and the sole caller uses a unit release
    signal. Dropping that sender still releases the barrier. Separate library, affected native,
    custom-suspension/real-timer regression, strict lint, production and browser checks pass.
    The actor barrier itself remains; this is not full idle-control removal.
    The deletion-stage candidate removes the quota case's redundant ExecutionFenced hold and both
    private DurableStateRemoved waits. Actual OplogService::delete backpressure belongs to the
    test-only dependency adapter. The decorated service is constructed before metadata, enumeration,
    scheduler and runtime consumers capture it; the old late bootstrap shim is removed. Production
    defaults keep the same Arc and no Worker/lifecycle callback is added. Mode-correct service history
    retains a nonempty complete committed prefix and exact storage absence after deletion; the
    original terminal/result/key assertion block is byte-identical. The deleting-first native fence
    remains to isolate the production outer-Deleting guard, which mutex contention would mask.
    Retirement, metadata/replay/health/after-End and custom/quiet-stdin checks pass, along with
    colocated library checks, strict executor/test-utils lint, production compilation, formatting
    and browser verification. This is a proposed narrow retention, not complete deletion-gate removal.
    The separate selector removal deletes the remaining hook's unused arbitrary-stage argument, stored
    selector and generic matching branch. Its API is fixed to the existing deletion fence. Main's
    WorkerDeletionStage/WorkerDeletionHook remain unchanged and still receive every stage. Separate
    library, native retirement/replay/window/health, main stage-failure/retry and cache-removal/
    cancellation, strict lint, production and formatting checks pass. The isolated native fence
    and this checkbox remain open; only the unused selector is removed.
  - [x] Remove the completed-history replay-gate family. The local change deletes its Worker
    field, type, setter, history scan and pre-invocation await after both native consumers pass at
    the exact stored input-payload acquisition boundary. The supported external-payload setting
    provides real uncached data; test-utils holds its actual download without changing replay state.
    All native assertion bodies remain apart from observation substitutions, with full raw prefixes
    and no-Jump checks added. The receipt does not claim native syscall or guest-instruction Pending.
    Separate producer-removal library, affected native and custom-suspension/acceptance regression
    batches, strict executor/test-utils lint, production compilation and browser checks pass.
    This completes this family only; overall A1/F10/F11 stay open.
  - [x] Remove the complete invocation-deadline control family. The Worker watch field, forced-expiry
    setter and test-feature timer branch are removed. Native selection tests use the configured
    timer and preserve their original assertions, with complete raw-prefix and exact timeout-text
    checks added. Those cases, the ordinary parked-P3 deadline case, strict lint and the production
    check pass locally. Ordinary establishment is now restored; monthly time remains open.
  - [x] Remove the complete settlement-loss selector. The local change deletes
    its Worker/window flags, setters, constructor wiring and post-success reply-drop branch.
    The real close-task proof first passes without that selector: the existing clock dependency
    panics in the spawned close task, which returns exact ObserverLost, reports no successful
    settlement and releases capacity to the same queued contender. The two native fake reply-drop
    cells retire with their producer. This is a body proof, not native settlement-panic evidence;
    the native health suite retains its other exact cleanup, startup and generation assertions.
    Post-removal library, combined native, strict lint, production and browser checks pass locally.
  - [x] Remove the actor-loss control family. The real append-fault baseline passes
    without calling either runtime stopper. Later-idle tests run Counter.allocate_memory, then fail
    its held positive GrowMemory append through the existing oplog fixture. Pending TCP uses its
    actual growth checkpoint. Test-side capture of the existing exact owner/failure fence event
    replaces the monitor-exit token. The local change removes both stop methods, the target's
    exit token, its constructor wiring and its task guard. The pre-guest initial-idle actor cell
    retires with the manual stopper; initial-idle real monitor failure and startup refusal remain.
    Other exact error, cleanup, generation and permit assertions remain. Separate post-removal
    library/native, strict lint, production and browser checks pass locally. No production capture,
    new trace event or synthetic growth hint is added. A1/F10/F11 stay open for the remaining work.
  - [ ] Address monthly time controls.
    Completion-first P2 input/splice/multi-poll and P3 promise/UDP consumers no longer install
    clocks they never advance. Their only observation substitution is an inactive actual monthly
    target instead of a virtual timer's receiver count, after the existing monitor/physical cleanup
    joins. Zero-subscription checks remain. An exact source audit retains all other result, effect,
    identity, history, accounting, timeout and concurrency assertions. Baseline and ordinary-timer
    native batches plus scoped strict lint pass. This is an unused-freeze consumer cleanup only;
    the clock producer and genuinely time-driving consumers remain open.
    The retry case now uses the normal timer. Exact no-early/update-rearm/two-period
    properties pass on the actual MonthlyMemoryCheck and queue functions with standard Tokio
    virtual time. The native case passes real contention, repeated applied-check rejection and
    subsequent real-timer acceptance without changing its timeout. It retains window/policy facts,
    result/history/accounting/cleanup and adds an explicit key with complete stored raw prefixes.
    An initial timeout was a test-side refresh-join deadlock: the full batch awaited a filesystem
    command blocked behind the silent invocation. Both joins now follow physical cleanup; policy
    application remains proved by exact rejection snapshots. Failed runs remain recorded. Final
    strict lint, monthly library checks and the small custom-suspension composition batch pass.
    No timer interface or production change was added; the clock family and this checkbox remain open.
    Provider/KV/RDBMS/entered-commit, blob/filesystem-adapter and quiet-stdin consumers also no longer
    install never-advanced clocks. Their existing provider/session checkpoints plus the actual
    active target and watch subscription establish the pre-update prerequisite. Entered effects
    retain their target/subscription; joined cleanup requires an inactive target and no subscription.
    An exact source audit retains all other assertion bodies, timeouts and in-scene concurrency.
    Unchanged baselines, ordinary-timer native batches, scoped strict lint and formatting pass.
    No production or fixture change is included. The filesystem case remains adapter evidence,
    not a native metadata-future Pending proof.
    The WebSocket native-lock and timeout-first proofs also no longer install never-advanced
    monthly clocks. Actual target/subscription observations replace virtual-sleep observations;
    an exact source audit retains the WebSocket timers, existing timing checks, peer counts,
    results, histories, cleanup and the separate active-clock helpers. Their unchanged baseline,
    ordinary-timer cases, scoped strict lint and formatting pass. WebSocket replay remains deferred.
    The prepaid-compute cases remove their virtual clock without substituting a policy update
    for the negative tick. A test-owned existing weak usage flusher routes exact window observations
    from initial pre-wait through post-wake and next pre-wait; lifecycle FIFO drainage follows the
    completed check. An unchanged capacity watch excludes a new applied update. The original
    prepaid/exhaustion/result/refund/history/cleanup assertions and native bounds remain, with
    actual-mode complete stored-prefix checks added. No runtime code or timer interface changes.
    Both native modes, the real-timer/tail composition batch, scoped strict lint and formatting pass.
    The clock producer and this checkbox remain open.
    The matching scripted-storage cases reuse the same test-owned weak flusher and ordinary
    timer, with no new policy revision driving exhaustion. Exact revision/window/resource, opposite
    storage-class availability, typed outcomes, overshoot/disabled meters, closed sampling, physical
    cleanup and recovery assertions remain, with actual-mode complete stored prefixes added. Only
    the matching positive cases migrate in that slice; dynamic nonmatching-update coverage is separate.
    The storage matrix and compute/retry/tail composition, scoped strict lint, formatting and
    assertion audit pass. No production, test-utils or fixture code changes. The clock family remains open.
    The nonmatching-storage cases retain the native applied-update-plus-periodic-check proof
    without its virtual clock. Exact window/revision usage-flusher observations, one watch change,
    no further updates/no memory-rate wake and a real cadence lower bound establish the check.
    Conservative observation count also covers a tick racing the pending policy update. All original
    non-clock native assertions remain, with full actual-mode stored prefixes added. Precise virtual
    timing/queue/mode projections move to passing tests of the actual paused-Tokio primitives.
    The storage matrix and compute/retry/tail composition, monthly library, strict all-target lint,
    formatting, source audits and browser checks pass. The first primitive test failure used the
    helper's generation as its policy revision;
    only that input was corrected and its exact revision assertion remains. The failed log is retained.
    The whole clock producer and remaining active consumers stay open.
    The P2 input-poll cases remove compute/storage clock setup and the repeat-advance delay
    loop, reusing the same test-owned flusher with only a shared-support visibility change. Memory
    and repeated-memory-stop retain their existing normal-timer path without a new source/watch.
    All original non-clock ensure/assert bodies remain, with full actual-mode prefixes added through
    stop/repeated stop/reconstruction. Existing input/connect Start, terminal/result/key, effect,
    refund, closed sampling and physical-cleanup checks stay strict. This helper has no elapsed8s
    trigger assertion; its original limits remain. Build, scoped strict lint, formatting and source
    audits pass. All P2 memory/compute/storage/repeated-stop cases and affected metering/support-move,
    nonmatching/retry/tail regressions pass. The remaining consumers and full clock producer stay open.
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
  - [x] Delete the invocation-deadline watch state and Worker expiry interfaces with their producer
    branch. The native tests now use the existing timer, as recorded under F10.
  - [x] Delete the Worker-owned teardown-fence field, pause methods, snapshot/gate types and
    invocation-loop test await. The consumer baseline and separate producer-removal checks pass,
    as recorded under F10. No replacement runtime coordination or trace producer is introduced.
  - [x] Delete the Worker-owned completed-history replay field, gate type, setter, history scan and
    pre-invocation await. Exact stored input acquisition belongs to the existing test-utils oplog
    dependency adapter, without replay-state mutation or another production conditional. Both
    consumer and separate producer-removal checks pass, as recorded under F10.
  - [ ] Address the keyed filesystem gate; oplog backpressure is not actual metadata Pending.
  - [x] Remove the monthly-acceptance observer/completion family. The local change
    removes the Worker sender, constructor wiring, setter, exported acceptance envelope and producer
    branches. Tests capture the existing completed-acceptance event with exact sampled attributes
    and acceptance boolean. A dual-observation baseline verifies Registry rejection, tick rejection
    and later acceptance against the old channel before removal. Native window checks use the unique
    diagnostic window ID rather than a projected Arc address; actual Arc checks remain in unit tests.
    Other native assertion bodies are unchanged after formatting. Library checks, strict lint and
    production compilation pass on the same Rust source. The capture-only canary, contention/refund
    cases and all cases that timed out previously pass in exact-name guarded batches under the
    user-approved local profile. The earlier broad failure and killed run remain recorded; this is
    not a claim that their resource behavior is fixed or that the full parallel cohort passes.
    The read-only current-window snapshot remains separate from this removed transport. The
    subsequent #4195 combined-source proofs have their own validation scope above.
  - [x] Remove the complete Worker-owned tail observation family.
    The unchanged native baseline and dual comparison pass before producer deletion. A normal
    root-completion debug event distinguishes guest execution from tail settlement; capture stays
    in the test binary with exact owner/key/primary fields. The existing pinned OwnerExecution
    tracker supplies live positive/zero counts, not the diagnostic snapshot. The change removes
    the Worker sender, setter/notify methods and root-wrapper test producer. Original effects,
    invocation identity, exact incomplete chunk/terminal history, receipts, accounting and physical
    cleanup remain, with complete stored-prefix/no-Jump checks added. The ordinary tail deadline
    and Notify are unchanged. Separate producer-free capture/native and custom-suspension/real-timer
    regression batches, library checks, strict lint, production compilation, formatting and browser
    verification pass. This completes this family only; overall A1/F10/F11 remain open.
  - [ ] Address idle observations,
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

The earlier executor checkpoints had publication authorization. Current follow-up engineering and
main integration are authorized for local commits only. Keep them unpushed until new authorization.
Grant precedence stays unchanged until the other work is complete and the user chooses the policy.
Checkpoint publication does not imply reviewer acceptance or thread resolution.

## Source coverage

This list covers the substantive human feedback: the review containing findings 1-10, the four
numbered follow-up comments, the later lifecycle request, the grant-precedence comment and the
CLI inline thread. GitHub's inline thread remains unresolved; this checklist does not resolve it.

Deployment-preview bot messages require no code action. The
[older Blacksmith failure report](https://github.com/golemcloud/golem/pull/3894#issuecomment-6046016925)
is not treated as a fresh review request or a current CI verdict. CI watching remains stopped.
