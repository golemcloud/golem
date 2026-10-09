# Owner-wide automatic suspension: working design and evidence

**Historical design and evidence.** The authoritative execution sequence, acceptance
criteria and progress board are now in [the suspension-only delivery plan](gol-706-implementation-plan.md).
That plan supersedes the execution sequences below; preserved prototype evidence
does not mean the reduced implementation passes those checks.

Working design for [GOL-706](https://linear.app/golem-cloud/issue/GOL-706).
Updated 2026-10-05. This is a living design, not an implemented or fully proven fix.

## Reduction completed and approved (2026-10-05)

The user authorized reduction/reverts and an explanation of next work. All 38
modified tracked files are restored exactly to the original baseline
[`accae0e435b5097cd1d7940a5c1f568bde8a055a`](https://github.com/golemcloud/golem/commit/accae0e435b5097cd1d7940a5c1f568bde8a055a).
The four new prototype Rust files are parked outside active source paths. Both
tracked and staged diffs are empty; only this living plan remains untracked.
This removes the connected implementation, including partially integrated policy,
instead of leaving dormant lifecycle dependencies. Useful suspension code remains
recoverable for selective reapplication, not active or approved functionality.

Preservation directory: `tmp/gol706-reduction-20261005/`.
- `golem-prototype-sources.tar.gz`: all 43 changed/untracked source members,
  including the pre-reduction plan and new prototype Rust files.
- `golem-prototype.patch`: binary-capable tracked diff against the named baseline.
- `wasmtime-observation.patch`, `wasmtime-base.txt`, `local-wasmtime.toml`:
  exact fork patch, base and explicit local override.
- `evidence.tar.gz`: 238 top-level evidence/configuration/log entries; original
  scratch/probe directories remain in place rather than being deleted.
- `source-sha256.txt`, `archive-sha256.txt`, and NUL-delimited path manifests:
  verified recovery inventory. Four prototype files also reside in `parked-untracked/`.

Archive/source hashes verified before rollback; archive verification and a forward
`git apply --check` of the preserved patch pass afterward. The external fork remains
preserved in `/tmp/gol706/wasmtime`, but restored Cargo.lock/default configuration
do not activate it. Existing ignored WASM/build artifacts may still come from the
prototype: rebuild selected fixtures before baseline runtime tests.

Verification: `git diff --exit-code` and `git diff --cached --exit-code` pass;
`cargo metadata --locked --offline --no-deps --format-version 1` and
`cargo fmt -p golem-worker-executor -- --check` pass. No runtime suite rerun or
passing suspension claim: the active source/tests are byte-identical to baseline.
Bug-finder `gol706-suspension-only-reduction` run 1 returned **no bugs found** after
archive/restoration checks. Oracle inspected the result and returned **APPROVED,
ON TRACK for reduction only**. No commits, pushes or merges were performed.

Next: select exact lifecycle/recovery integration revisions and order; rebuild
fixtures and reproduce GOL706 there; reconstruct only runtime-idle observation,
suspension accounting/policy and durable wake handling around existing discard/replay.
Use a reproduced suspension-specific failure to justify any lifecycle correction.
P2/stream gates remain observable behavior checks, not new cleanup protocols.
Keep the independent ReplayFinished candidate separate. The historical reviews
below describe the now-archived prototype, not current active source.

## Governing scope correction — suspension only (2026-10-05)

The user's latest instruction explicitly rejects an executor lifecycle refactor.
GOL706 must fix suspension and only the changes demonstrated necessary for it,
leaving lifecycle/recovery ownership with the parallel work. This section supersedes
the cleanup-barrier requirement, ownership extraction prerequisites and proposed
new activation authority in all earlier sections. Preserve those sections as
historical evidence, not current implementation requirements.

The parent reviewed the local diff, original HEAD behavior, both referenced threads,
current PR manifests and selected overlap patches. Oracle approves this scoped
salvage direction and rejects the current diff and previous B/C/D formulations as
the delivery plan. No production code was changed or reverted in this review; no
tests were run. The prototype remains local and uncommitted, not an approved fix.

### Why the earlier scope was not justified

Original suspension already signals Suspend, marks teardown, discards the Store
and reconstructs from the oplog. Its eligibility predicate explicitly describes a
scheduling heuristic, not a recoverability boundary. Unfinished calls are allowed
to remain incomplete; runtime loss must not manufacture guest cancellation.
The raw first-error probe established that sibling futures can be dropped, using
modeled obligations. It did not establish that actual Golem durable recovery fails.
Requiring new acknowledgment from every disposable future was a stronger contract
introduced by this prototype, not a proven prerequisite for correcting eligibility.

Full accepted-continuation extraction then required shared atomic state, late
metadata publication and primary-finalization rendezvous. These are consequences
of the extraction, not demonstrated requirements of GOL706. Excluding the whole
connected rewrite from a reduced candidate does not assert existing recovery is
bug-free. Existing persistence/terminal ordering guarantees must remain intact.

### Parallel-work ownership and observed delivery state

- [GOL708 thread](https://ampcode.com/threads/T-01a0f7d6-5ee1-7410-96e8-2ddc8bcc7adb),
  [PR4034](https://github.com/golemcloud/golem/pull/4034), observed OPEN/draft at
  `16237d4326b2c2fb98dd3b95d140e77abf8f1f18`: owns pre-reconstruction interrupt
  handling, queue/fence/signal/drain, exact owner/generation teardown and entity
  admission. Do not replace these with GOL706 lifecycle machinery.
- [GOL710 thread](https://ampcode.com/threads/T-01a0f7d1-e59c-754c-9fa6-b9881c470835),
  [PR4038](https://github.com/golemcloud/golem/pull/4038), observed OPEN/non-draft at
  `afa65199ac41bee91c6d048a2d5172cb3fe2c696`: owns complete-suffix recovery, accepted
  writer drainage, Jump replacement, cancellation ordering and replay/live rules.
  The current head is newer than the thread's summary. Its runtime-teardown terminal
  guard suppresses guest cancellation without GOL706's new cleanup-token protocol.

Neither PR is assumed merged or jointly validated. An integration baseline must
name exact revisions and integration order; do not silently combine the branches.

### Reviewed disposition of the prototype

| Disposition | Change group |
|---|---|
| Salvage, still requiring verification | Minimal Wasmtime blocked evidence, activity IDs, wake invalidation and queued-work wake correction; suspension-only owner accounting; timer/promise/RPC policy, earliest durable wake and revalidation after persistence. |
| Salvage behavior/evidence | GOL706 reproductions, actual unload/resume, mixed-work progress, provider-side effect counts, stream observations and follow-up invocations. Preserve diagnostic tests separately when their prototype APIs disappear. |
| Conditional production integration | Narrow P2 owned-timer dispatcher, preserving supported borrowed-source behavior and actual SDK progress/replay. |
| Exclude coherently from reduced GOL706 | Generic cleanup tokens/barriers; StoreFuelState/into_data/asynchronous UnloadRuntime and resource-table drainage; generic startup cleanup; native cancellation rewrite; accepted-continuation extraction/private receivers; dependent atomic registry, metadata watches and primary replay-tail rendezvous. |
| Rework, not transplant | publish_unloaded/stop_internal and coordinator startup/unload authority. Use existing lifecycle/generation protocol; reproduce a suspension-specific lost wake before changing its critical section. |
| Independent candidate | Primary-only ReplayFinished dequeue. Coordinate with GOL710; independently test both adapters and update success/failure without the extraction/rendezvous machinery. Do not silently include it as a suspension prerequisite. |

Accounting must cover existing primary/entity/native/stream work without moving its
ownership. Unknown is temporary conservative classification, not a permanent way
to avoid supported cases. Remove competing local suspension elections in the final
candidate. Preserve GOL708/GOL710's accepted-writer and terminal-ordering guarantees.

### Finite reduction sequence

1. Preserve the complete prototype, untracked files, fork/configuration, fixture
   provenance and logs before reduction. Select the integration baseline; do not
   reset the checkout or transplant the entire dirty branch.
2. Run the original suspension regressions and focused handoff/P2/entity/stream
   controls there. The existing promise handoff tests are useful, but no baseline
   negative control for their locking change was established by this review.
3. Reconstruct the narrow patch around existing discard/replay: runtime evidence,
   suspension policy, participant accounting and wake persistence. No generic
   teardown barrier or second lifecycle state machine.
4. Execute revised gates: B proves actual stream suspension/reconstruction semantics,
   not new cleanup acknowledgments; C proves production P2 progress/unload/replay;
   D proves the existing-lifecycle handoff, with only reproduced local corrections.
5. Require exact results/effects/stream observations and follow-up use. Only a real
   semantic failure on the baseline or reduced patch admits an additional scoped
   repair, coordinated with the owning work. It does not reinstate the broad rewrite.

The recorded-parent-cancellation case remains recovery evidence to classify by
baseline provenance: its current child-before-parent assertion fails before crash,
so that run demonstrates neither successful nor failed reconstruction. Earlier
prototype results must not be silently generalized to an independent baseline.

Oracle trajectory: the current implementation remains out of scope; the reduction
is a credible corrective direction, not a validated solution. This review does not
approve merging either PR, deleting work, publishing a new PR or weakening tests.
Every actual implementation checkpoint still requires the user's Oracle/bug-finder
and progress-report gates. There is no completion estimate or suspension-fix claim.

## Replacement delivery gate — read-only review completed (2026-10-04)

**NO-GO on resuming production implementation.** The user authorized this bounded
read-only review after the trajectory pause. No production files were changed in
the review. Oracle agrees that the remaining work is not yet a fixed set of
adapters over a proven ownership protocol. Preserve the local prototype and tests;
do not discard, ship, or automatically resume the old Step 3. The earlier sequence
below is historical and no longer constitutes an implementation-ready plan.

The central design is not disproven. The failure is implementation readiness:
stream stopping, supported P2 dispatch, and the production activation authority
are not established. Local cleanup fixes cannot close those contracts by themselves.
The previously running session-custody test run finished: **4 passed**, in
`tmp/gol706/source-session-custody.log`. The P2 HTTP disposal run passed **7**, in
`tmp/gol706/source-http-session-disposal.log`. Neither closes these design gates.

### Required invariant and ownership boundary

1. Commit only with complete resident participant accounting, fresh runtime-blocked
   evidence, recoverable waits and durable wake coverage. Admission/readiness must
   race atomically with commitment.
2. Before the first stop error escapes, work needing the runtime must have settled,
   become explicitly reconstructible, or moved to genuinely Store-independent
   custody. Counter decrement and dropped futures are not cleanup acknowledgments.
3. Before publishing successful unload, finish resident drivers/resources and
   activation-scoped persistence/delivery receipts. Independently owned services
   may survive Store unload; do not require all detached services to terminate.
4. Route durable activation either to the current residency or its successor,
   without loss or duplicate startup. Stale runtime callbacks cannot affect a new
   residency, and owner retirement prevents resurrection.

### Participant map: established ownership versus remaining proof

| Participant | Before / after runtime destruction | Remaining work |
|---|---|---|
| Primary driver and lifecycle | Store drives guest work; OwnerExecution and retained unload own cleanup afterward | Coordinator activation/unload APIs have no production callers; timestamp arbitration remains. Requires gate D, not merely wiring calls. |
| Component entity, native and MCP body | Accepted-operation supervisor and runner retain continuation/context outside the disposable Store | Known custody repairs and actual parent-first reconstruction proof. The latest regression fails its ordering assertion before simulated crash; this is not proof of replay failure or success. |
| P3 timers and promises | Runtime activity is classified; durable deadline/promise supplies recovery | Connected bounded slices exist. Durable activation must use D before complete ownership is claimed. |
| P2 timer and borrowed I/O | Existing synchronous binding retains Store-local readiness; proposed dispatcher releases owned timer waits | Production still uses local suspension election. Actual binding, supported SDK progress, unload and replay require gate C. |
| RPC | Durable Start/key and target acceptance survive resident transport loss | Grace/recheck adapter is bounded after D; streaming materialization also depends on B. |
| Durable input/output and guest transfer | Owned receive future waits, journals and constructs nested endpoints; Destination encoding is later Store-bound work | Explicit wait/persistence/delivery phases and receipt ownership are unproven across stopping. Requires gate B. |
| Selected terminal/marker appends | Existing retained cleanup owns persistence receipts outside Store | Integrate complete admission and error reporting; receipt must precede successful unload. |
| Late P3 indexed-Start callback | Oplog actor can outlive source and enqueue span cleanup after guest abandonment | Source-bound retained actor receipt still missing; join producer before final source drain. |
| Durable producer terminal dispatcher | Owner service operates on producer/bus/durable records, not guest memory | Preserve independent lifetime/fencing; do not incorrectly make service termination an unload prerequisite. |

`RetainedCleanupOwner` permits runtime destruction but still requires a final
receipt before unload; it is not an indefinite owner-service exemption. Unknown
classification is temporary fail-closed behavior, not permission to omit supported
suspension cases. Full continuation extraction made shared atomic membership,
metadata publication and replay-finalization changes necessary for this prototype;
those costs are consequences of the chosen ownership design, not independently
unavoidable requirements of automatic suspension.

### Proposed next authorization: three bounded gates, not implementation resumption

**D — specify the activation contract (no feasibility probe needed).** Choose one
owner-lifetime arbiter; ordinary unload changes activation identity rather than
retiring the owner authority. Runtime callbacks carry activation identity. Durable
source events address the owner and route to its current/successor activation.
Coordinate one startup reservation with WorkerInstance/start_attempt. Specify one
lock order, preserve entity drainage before taking the lifecycle lock, and perform
notification/cleanup outside the coordinator lock. Explicitly define what an
existing driver may poll after commitment. Race matrix: wake before/during scheduler
persistence, before/after commitment, during cleanup/unload publication; simultaneous
startup requests; stale callbacks; retirement at each boundary; cleanup failure.
This is a recommended contract, not an implemented guarantee.

**B — one actual durable-stream stopping experiment package.** Use non-flat values
and nested handles. Test valid commitment at reconstructible source wait followed
by unload/replay/exact observation. Gate a real journal append: voluntary commitment
must be denied, while separately forced teardown must retain/join its receipt.
Test coincident readiness/delivery and every supported transfer direction, without
invented cancellation, EOF, duplicate observation or stranded sibling cleanup.
The owned receive future makes executor extraction plausible but does not prove it.
Output: executor-owned transfer suffices, or the exact runtime dispatch/first-error
guarantee that is missing. Forced-stop evidence does not certify voluntary eligibility.

**C — one production-path P2 experiment package.** Integrate only experimental
binding/driver glue needed to run real SDK exports. Prove long-timer unload,
activation, replay and exact result; prove supported independently scheduled mixed
work progresses with exact effect count and follow-up invocation. Enumerate relevant
P2 source/replay/blocking coverage. Raw dispatcher success and the earlier joined-
future causal stall do not substitute for this production topology. Do not solve a
failed case by permanently making a previously supported case ineligible.

Each gate returns **pass, fail, or inconclusive**. Stop on fail/inconclusive; report
the exact missing contract instead of expanding the experiment into a production
refactor. Any larger fork requirement returns for explicit review. No completion
estimate is justified. No further code changes are authorized by the read-only review.

### Conditional implementation sequence, only after the gates

1. Close the known lifecycle custody repairs, secondary-error handling and the
   genuine parent-first reconstruction test. Do not weaken or remove that history.
2. Implement D's activation/unload authority with its race matrix.
3. Attach participant adapters using B/C's established contracts; replace every
   old local suspension authority rather than leaving competing paths.
4. Execute the full supported-case acceptance matrix and remove superseded code.

Each implementation checkpoint still requires Oracle and bug-finder review,
updated Linear evidence and the user's intermediate progress report. None of these
four conditional phases is approved or complete. The original acceptance matrix
remains required; separating work does not defer or waive any supported behavior.

Source anchors reviewed: `worker/suspension.rs::RuntimeStore::drive`,
`worker/instance.rs::finish_suspension_activation`,
`worker/invocation_loop.rs::publish_unloaded`,
`durable_host/durable_session/mod.rs::{DurableInputProducer,DurableInputEndpoint}`,
`durable_host/durable_stream/publication.rs::start_terminal_dispatcher`,
`durable_host/io/poll.rs`, and `durable_host/wasm_rpc/mod.rs`. The local fork diff
contains runtime observation changes, not the production host/linker dispatcher.
The activation API call-site search finds definitions and tests, no production
callers. Oracle's final gate verdict is **NO-GO; recoverable prototype, failed
implementation readiness**. Overall implementation remains paused.

## Status and scope

The user requested a centralized replacement for automatic-suspension heuristics,
preserving existing behavior and tests. Minimize changes to the Wasmtime fork,
prefer no fork change when correctness can be established without one, and do not
replace runtime evidence with executor-side guesses merely to avoid a fork patch.

Five bounded experiments in three child orbs have completed. Their results support a genuine
runtime-blocked observation, Store-releasing owned P2 waits, and a small per-call
execution dispatcher. None proves complete suspension eligibility, production P2
integration, or owner-wide lifecycle arbitration. The owner coordinator is now
connected to real P3 timer execution in a bounded local integration slice. The parent built the executor
integration target, CLI and selected fixtures
and ran the step-1 baseline below; this is not a full regression-suite run.
Probe code is local to the child workspaces; nothing
has been pushed or published. GOL-706 is assigned to Daniel Vigovszky and In Progress.

The bounded narrow P2 execution-mode dispatcher experiment completed in the
[dispatcher experiment child orb](https://ampcode.com/threads/T-01a0fb17-7c37-73ce-8cf7-e27d32b5695e).
The dispatcher mechanism passed raw-runtime probes; real Golem lifecycle acceptance
was not reached. The borrowed fallback blocks other Store work, so its progress
limitation must be resolved or explicitly accepted before this is a complete design.
Do not silently broaden the work into a rewrite of all built-in WASI sources or
treat keeping the Store resident as proof that mixed work can progress.

## Implementation checkpoint and reporting requirement

After every implementation step, update this document and its Linear copy, obtain
Oracle and bug-finder review, and give an intermediate report: position in the plan,
remaining work, adjustments, and Oracle's explicit ON TRACK / AT RISK / DERAILED
judgment. Pause implementation if derailed rather than silently changing scope.

### Current gate: implementation paused after trajectory review (2026-10-04)

The user challenged the multi-day Step 3 expansion. Oracle reconsidered the whole
trajectory rather than the correctness of individual fixes and returned
**DERAILED for execution trajectory; AT RISK, not disproven, for the architecture**.
Implementation is paused under the user's required stop rule. Preserve the local
prototype and tests; nothing is committed, pushed, merged or approved as Step 3.

Safe stopping is necessary, but this implementation has not demonstrated a finite
remaining ownership boundary. Full accepted-continuation extraction brought shared
atomic membership, replay finalization and metadata publication into scope. These
are real consequences of that architecture, not independently unavoidable pieces
of automatic suspension. Local fixes have converged; the complete project has not.
Parts of Steps 4, 6 and 7 were pulled forward, so the original step count now hides
the extent of the lifecycle refactor. P2 integration, full activity coverage and
atomic activation/unload handoff remain substantive unfinished work.

Before this pause, unfinished-session teardown custody and context-owned P2 HTTP
disposal were patched locally. Seven P2 session tests passed; the session custody
test build is still running. The queued P3 indexed-Start producer is not fixed.
The seven production regressions on the preceding draft passed six; the parent
cancellation fixture failed its required parent-first ordering before replay.
No new bug-finder approval or complete checkpoint approval is claimed.

Proposed next action, requiring a user decision after this pause: one bounded,
read-only architecture decision session, with no further production code expansion.
Produce a short replacement delivery plan mapping each required participant to its
owner before stop, after runtime destruction and at final receipt. Distinguish fixed
integration work from unresolved mechanisms, retain the parent-first replay case,
and separate the lifecycle refactor from suspension integration. The gate asks
whether all remaining work is a fixed set of adapters over the current protocol.
If any participant requires an unspecified ownership mechanism, an unproven runtime
guarantee or an open-ended further audit, stop rather than extend implementation.
There is no demonstrated smaller complete alternative and no defensible completion
estimate yet. Permanently disabling suspension for required participant classes is
not an acceptable shortcut.

### Earlier resumption before the current pause

Implementation resumed on the user's explicit Continue instruction. The
prior Oracle verdict was DERAILED for execution trajectory. Two accepted-continuation assignments produced no retained
implementation of the approved handoff. The first attempted teardown out of order
and was reverted; the second made no edits and only reproduced the known orphan.
The metadata prerequisite remains approved. This is not a newly proven design
blocker, and the expanded Step 3 scope does not require renewed approval. Resumption
requires a changed execution approach for the already-approved complete handoff,
not a third equivalent assignment, another baseline rerun, or a smaller unused
prerequisite. The changed execution approach is direct implementation in the parent
thread, starting with complete acceptance-time custody before changing teardown.
The previous diagnosis and unfinished acceptance gates remain recorded below.

### Accepted-continuation handoff in progress (2026-10-04)

Oracle judges the resumed ownership boundary **ON TRACK**, not implemented or
approved. The source-context drain in `prepare_owned_terminal_adapter` can be
removed only after the accepted call's own terminal cleanup is independently
retained. Admission already drains earlier source events; this adapter's pre-body
snapshot never guaranteed eventual drainage of unrelated source work. Destructive
teardown must still retain and drain the actual source context and its resources.

Retaining the full accepted continuation alone is insufficient: panic unwinding
and armed-guard error exits can enqueue `CleanupAfterTerminal`. The same retained
accepted-operation supervisor must therefore own an operation-local receiver
outside the continuation's unwind boundary. Rebind only this concrete markerless
`GolemEntityInvoke`/`WriteLocal` call, not the source's whole receiver. Keep the
existing `AccessTerminalGuard` and append task; do not add another terminal
supervisor. Successful End's atomic side-effect marking must remain with the
retained append, before lease release. Preserve append/join failures, the live-call
permit, cleanup receipts and exactly-one terminal semantics after observer loss.

At synchronous acceptance, register an Unknown accepted activity before first poll.
Transfer existing runtime cleanup with `transfer_with_child`, receipt the original
obligation once custody is established, and retain the child through complete
operation settlement. With no original cleanup, register retained accepted cleanup
directly. Replay repair uses the accepted activity, never an expired source
Accessor identity. Only exclusive body waiting may certify accepted-to-body
dependency; revoke before independent work. Preinstall primary replay finalization
while the Accessor exists; its dormant listener certifies a dependency on accepted
activity, never a guest-root classification. Revoke the accepted wait before
requesting finalization and the listener certificate before processing it.

A bounded module implementation has added the local receiver/handoff API and moved
End side-effect marking into its append task. Parent review corrected missing
repair-cleanup acknowledgment and the no-original-cleanup case, and expanded tests
to lose both the source receiver and terminal observer, hold persistence while
the drain is polled, verify permits/leases and End-versus-Cancelled side effects,
and surface an injected terminal failure. Parent validation recorded **7 passed**
in `tmp/gol706/accepted-custody-parent-tests.log`. Final corrected-source validation
recorded **8 passed, 0 failed** in `tmp/gol706/run2-baseline.log`, using
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=6 cargo test --config tmp/gol706/local-wasmtime.toml -p golem-worker-executor --lib -- owned_entity accepted_entity_custody --report-time`.
These are concrete primitive tests, not full continuation custody evidence.

Oracle's intermediate review was **AT RISK, not DERAILED**. The local receiver and retained
End marking match the approved boundary, but this must lead directly into the
production continuation handoff rather than another approved disconnected
prerequisite. Oracle found a stop-observation regression in the temporary Accessor
repair adapter: it must retain its coordinator even when cleanup registration is
refused after committed stop or runtime activity lookup fails. Bug-finder run 1
reproduced this (`accepted-repair-registration-drops-stop-observer`). Both entry
points now retain the coordinator before lookup/registration. The accepted test
also commits an actual coordinator stop, races an already-ready body with local
cancellation, and checks registration attempted after stop. Bug-finder run 2 marks
the sole finding **RESOLVED**, with no new findings; no further certificate-seeking
run is needed on this snapshot. Oracle's final focused follow-up: **ON TRACK**,
the defect is resolved and the primitive may proceed into intended integration,
explicitly not Step 3 approval.
The drain itself is intentionally not independently cancellation-safe: the owning
supervisor must never select it away, must retain its receiver outside unwind,
and must apply drain failure to authoritative operation settlement before receipt.

**Partial production integration, not approved:** the bounded cross-file integration
now consumes the handoff API and detaches ordinary tool execution. Its recorded
evidence is **13 units passed**, recorded-parent cancellation **1 passed**, native
session **1 passed**, completed reconstruction barrier **1 passed**; logs are
`tmp/gol706/acceptance-handoff-{units-final,recorded-parent-rerun,native-session,completed-reconstruction}.log`.
Primary update-finalization tests report **2 passed / 1 failed** in
`tmp/gol706/acceptance-handoff-primary-finalization-final.log`; the failure variant
times out before its injected finalization gate. Its cause is not established.
These passes are progress, not proof of full custody or Step 3 completion.

Parent inspection and Oracle identified three integration blockers. Oracle's
verdict is **AT RISK, not DERAILED**: retain the production progress and correct the
same approved ownership boundary, not introduce a new architecture.

1. Native-direct acceptance still awaited owned execution inline, including inside
   a `try_join!` that can drop it. Ordinary Tokio execution lacked an unwind boundary
   preserving accepted custody and error authority.
2. Primary handoff certified the initiating Accessor activity instead of the
   separately allocated background listener activity. The listener stayed Unknown;
   a certificate on the sender did not fix the activity identity.
3. Terminal custody drained only after operation settlement/error handling; new
   drain failures bypassed owner failure authority. The lifecycle observer shortcut
   could receipt accepted custody before retained owner cleanup finished, and the
   final wrapper erased typed execution errors even after successful drainage.

Parent corrections are now implemented: route both paths
through one retained task; certify inside the listener's own run and revoke before
processing; retain custody outside continuation unwind; drain before error/operation
settlement; wait for actual owner cleanup before receipt; preserve typed results;
and release interrupted ordinary/cancel selection before failure election. The
scoped build check is recorded in `tmp/gol706/accepted-owner-parent-check.log`.
The scoped `cargo check --tests` completed successfully. Oracle's next review is
**AT RISK, not DERAILED** and confirms removing the duplicate Runtime cleanup
registration at accepted handoff: retain accepted cleanup across replay-to-live;
do not recreate source-Store Runtime custody. The torn terminal/failure tests now
exercise retained-only custody; that intermediate run passed 12 tests.

Two further corrections have been implemented and reviewed:

- Retain body resources outside continuation unwind starting when `drive_owned`
  receives them, not only after an error has been returned. Early terminal decode
  errors and panics before/after parent-end preparation previously could lose resource
  settlement. Cleanup accounting failed closed, but that did not perform settlement.
  Use the existing supervisor, not another terminal supervisor or asynchronous Drop.
- Preserve authoritative typed execution failures when terminal drainage or later
  cleanup also fails. Secondary cleanup failures must remain visible in custody
  reporting; a cleanup-only failure must still select owner failure before settlement.

The resource holder now lives outside continuation unwind and receives resources
at actual body completion, before terminal decoding or arbitration. Obsolete
resource-carrying outcome/failure paths were removed. Cleanup errors no longer
replace the original typed execution failure, and accepted receipts retain the
secondary cleanup diagnostics. Combined targeted library validation passed
**25 tests** (`tmp/gol706/accepted-integration-review-units.log`). Bug-finder task
`gol706-accepted-continuation-integration`, run 1, returned **no bugs found**.
Oracle confirmed both reviewed defects fixed, with **AT RISK, not DERAILED**
because production validation and source-context teardown remained incomplete.

The six targeted production regressions passed once
(`tmp/gol706/accepted-integration-production.log`). Repeating primary finalization
then reproduced the pre-injection timeout in the success variant: repeat 1 passed
3 tests; repeat 2 passed 2 and failed 1 (`accepted-finalization-repeat-2.log`).
This invalidated any claim that the prior green run resolved the race.

### Primary replay-finalization event ownership correction (2026-10-04)

The failed run shows an entity consuming `ReplayFinished` from the shared replay
queue. Its local pending update is empty, so it consumes the trigger without
performing primary finalization; the primary later sees an empty queue. Oracle
confirmed the bounded fix: reserve `ReplayFinished` at dequeue for the primary,
after live publication, in both accessor and direct adapters. Other replay events
retain existing routing; card events have caller-local authority semantics, so
blanket primary-only routing is not established by this finding.

The explicit-role dequeue change is implemented. A negative-control library run
with the old policy failed because a nonprimary drain stole `ReplayFinished`
(`replay-finished-negative-control.log`). Three focused library tests passed, and
the three production finalization/metadata cases passed three runs (9 total).
These results did not yet close the checkpoint: Oracle found that the new test
could observe a pre-publication drain, and that it replaced the original
promise-still-pending schedule. At that review no production negative control had run.

Test corrections are now locally implemented: arm the drain
observer only after primary live publication; retain both theft-race and original
pending-promise schedules for success and injected failure; assert exact update
terminal counts. The arm test passed, and the full replay-state module passed
**167 tests** (`replay-finished-complete-gate-unit-final.log` and
`replay-finished-complete-replay-state-module.log`). Adding a real synchronous
wall-clock call after promise completion exercised the direct adapter, but exposed
a fixture-ordering flaw: the test also required an accessor observation that was
not guaranteed after publication. The corresponding production negative control
failed before both intended observations, so it is not yet the required proof.

The parent is correcting that fixture sequence: only the theft-race schedule
enables an explicit post-promise zero-duration P3 timer followed by P2 wall-clock
read. This forces both adapters while primary processing is paused. The original
pending-promise schedule does not enable these calls, since admitting new durable
calls under held finalization would test a different blocking condition. The
fixture rebuilt successfully via the Golem CLI; the combined five-case production
run passed **5/5** (`replay-drain-both-production.log`), including both adapter
observations in both theft-race outcomes and the two pending-promise outcomes.
The corrected forced-sequence production negative control failed at the intended
boundary: both adapter-observation waits completed, then real automatic-update
finalization was not reached (`replay-drain-both-negative-control.log`). The primary
role restriction was immediately restored. Three corrected five-case repetitions
then passed **15/15** (`replay-drain-both-restored-{1,2,3}.log`), covering theft-race
and pending-promise success/failure plus metadata publication. The command used the
documented debug-disabled Cargo environment and local Wasmtime config with
`--test integration -- primary_incomplete_entity primary_pending_promise
incomplete_tool_uses_metadata_published --nocapture --test-threads 1 --report-time`.

Bug-finder `gol706-replay-finished-primary-ownership`, run 1, returned **no bugs
found**. Its focused ownership/gate checks passed in
`bug-finder-run1-primary-{ownership-unit,gate}.log`. Oracle inspected the code,
positive and negative evidence and gave **APPROVED, ON TRACK**, bounded to this
correction. No new approval is required to proceed with the already-approved
source-context destruction/receiver-drain scope. This closes the replay-event
checkpoint, not Step 3 or the suspension fix. `git diff --check` passes.

Pinned-fork source review supports `Store::into_data()` for retained teardown:
Store-owned fibers/futures are destroyed before the host context is returned.
It does not join detached tasks or perform asynchronous cleanup. The implementation
must retain the returned context, destroy its resource table while the event
receiver remains alive, then drain the actual queued cleanup before releasing
custody. Primary shared Store ownership and entity retained-Store settlement still
need integration and executed acceptance evidence; source review alone is not proof.

### Connected source-context teardown draft (2026-10-04)

Source-context teardown is now implemented locally as an unapproved connected draft.
The entity runner owns component/native contexts outside its abort/unwind boundary,
including partial construction. The existing supervisor prepares parent completion
while retaining those contexts. Staged Store ownership uses `Store::into_data()`;
the retained context destroys its resource table and drains source cleanup events.
Primary unload and post-instantiation failure paths use the same retained source
drain before filesystem disposal. Before-terminal accessor guards now transfer
reconstructible abandonment to retained cleanup rather than inventing cancellation.

Six targeted Store/unload tests passed (`source-retention-runtime-tests.log`), and
four guard/custody tests passed (`source-retention-abandonment-tests.log`). The broader
unit run passed 145 and failed one: a test signalled settlement entry before polling
the settlement body. That signal now follows the action it certifies; rerun pending.
The original broad run also exposed a missing FutureExt import, now corrected.
The seven production regressions passed six and failed the recorded-cancelled-parent
case before its replay phase (`source-retention-production.log`): the child Cancelled
entry preceded the parent Cancelled entry (69 versus 70), violating the fixture's
required crash-prefix ordering. This is not a passing recovery regression and must
be diagnosed without weakening the acceptance case.

Oracle reviewed this draft and returned **AT RISK, not DERAILED**. It supports the
ownership boundary but withholds checkpoint approval. Concrete remaining work:

1. Retain/join the P3 HTTP indexed-Start actor callback through source teardown.
   Real guest abandonment may precede the callback; it can enqueue span cleanup
   after an otherwise empty source drain. Join its actual completion/error receipt,
   then drain the resulting cleanup. Pure teardown must not synthesize abandonment.
2. Transfer unfinished live DurableCallSession custody on runtime teardown, not
   only AccessTerminalGuard custody. Preserve uploads, atomic leases, live permits
   and cleanup receipts; retain ordinary drop/replay/shutdown distinctions.
3. Dispose or disarm concrete context-owned P2 HTTP sessions in open_http_requests
   before final drainage. ResourceTable destruction alone leaves these last owners
   alive. Preserve genuine selected cleanup; do not fabricate owner-stop terminals.
4. Close secondary-error reporting and obsolete native post-body preparation paths;
   exercise construction, panic/abort, observer loss and deadline behavior.
5. Run deterministic producer/session/map regressions, the corrected units and all
   seven production regressions on final source; obtain bug-finder and Oracle gates.

Do not add an anonymous sender-count barrier: the identified issue is a concrete
actor-owned producer. Store-local accessor drain guards requeue before into_data
returns and need no extra supervisor. Accepted entity operations retain their own
independent receiver; do not move them back into the source context. No checkpoint
or Step 3 approval is claimed for this draft.

Step 3 and Steps 4–8 are not complete, and no production/fork changes have been pushed.

Current checkpoint: step 1 baseline recorded; step 2 policy-only implementation
approved by Oracle and bug-finder, with 16 passing tests. **The user approved resuming
the directly owned, real two-timer integration checkpoint after the DERAILED pause.**
The bounded real two-timer and promise checkpoints have passed their review and validation gates;
complete step approval is still withheld.
Steps 3–8 remain unfinished. The five baseline regressions remain acceptance
obligations, with the all-long agent timer case now passing once locally.
All work is local, uncommitted and unpushed.

### Resumed: accepted-operation ownership design checkpoint (2026-10-03)

**Oracle trajectory: DERAILED if implementation proceeds into the additional
ownership refactor without a user checkpoint. The user has now approved the revised
Step 3 scope, starting with the exact recorded-parent-cancellation reproduction.
Implementation has resumed within that scope. Step 3 and Steps 4–8 are not complete
or approved.** The remaining work is not waived. This approval lifts the pause,
not the outstanding Oracle/bug-finder and production verification requirements.

The entity supervisor alone is too low an ownership boundary. A parent entity
Store can contain `ToolExecutionTask` executing an accepted child operation. Aborting
the parent destroys that Store's AccessorTask futures. `OwnerToolOperation::drop`
only releases a lease: it does not settle an Open/SelectingOrdinary operation.
Retained scope cleanup then waits for a continuation that no longer exists. The
completed-reconstruction monitor also sends cleanup-bearing state to a Store-owned
receiver and ignores failed delivery. This is a source-backed unresolved path, not
merely a hypothetical ownership concern: the exact reproduction below now fails.

The new `recorded_cancelled_parent_replay_retains_accepted_child_cleanup` test
records real parent and child cancellations (parent terminal first), keeps the
invocation unfinished with an independent sibling, then performs **one** simulated
crash. After releasing the replay sibling it times out: parent start 15 is Cancelled,
child start 25 remains SelectingCancelled, outer start 12 is Ordinary, and no owner
failure is reported. Destruction receipts include reconstructed starts 15 and 25.
Two runs reproduced this; see `tmp/gol706/recorded-parent-cancel-replay.log` and
`tmp/gol706/recorded-parent-cancel-replay-metadata.log`. These are real executor/oplog
observations, not modeled sessions. The Oracle inspected the reproduction and judged
the approved ownership refactor **ON TRACK**. The test intentionally remains red
until that refactor completes; its later uniqueness/drain assertions are not yet
reached. The first implementation checkpoint is a shared originating-context atomic
registry, preserving existing region semantics and synchronizing completion against
the lease's current owner with close/transfer. Separate entity Stores keep separate
registries; shared owner oplog does not mean shared atomic stacks.

#### Shared atomic-registry prerequisite: approved

The originating context now owns an `Arc<Mutex<Vec<ActiveAtomicRegion>>>`. All
existing users synchronize on that registry. Completion resolves the lease's
current owner under the registry lock, and close conditionally transfers each
lease under its own lock so a concurrent release cannot resurrect membership.
Direct/accessor terminal ordering, `EndAtomicRegion` append-before-close,
registration, incomplete repair and logical idempotency positions are unchanged.
No entity capability or operation supervisor has moved in this checkpoint.

Validation: the final scoped run in `tmp/gol706/atomic-registry-final-tests.log`
reports **203 passed, 0 failed** (atomic regions, concurrent durable calls,
suspension, entity invocation and tool operations). Command: the documented
debug-disabled environment with `cargo test --config tmp/gol706/local-wasmtime.toml
-p golem-worker-executor --lib -- atomic_region durable_host::concurrent
worker::suspension worker::entity_invocation durable_host::entity
durable_host::tool::operation --report-time`. Four added tests cover completion
before/after transfer, detach versus a later region, missing-owner rejection, and
release-before-transfer. Formatting and `git diff --check` pass.

Oracle: **APPROVE, ON TRACK**, bounded to this prerequisite. Bug-finder
`gol706-shared-atomic-registry` run 2: **no bugs found**. Run 1's cross-registry
finding was rejected after Oracle audited production P3/entity/RPC callsites:
its test directly supplied registry B with registry A's lease, an unsupported
helper invocation that already failed before the refactor. The invalid test was
removed, not weakened to hide a production failure. The original test log includes
that rejected provisional failure; the final clean log above is the current result.

Next: capture the originating registry alongside the entity-specific owned
`GolemEntityInvoke`/`WriteLocal` durability capability, then move the entire accepted
continuation before Store-task handoff, including native direct accepted execution.
The real recorded-parent-cancellation regression remains unresolved and mandatory.
Step 3 overall and Steps 4–8 remain unfinished; nothing is committed or pushed.

#### Primary replay-finalization rendezvous prerequisite: approved

The primary Store now owns a one-shot replay-tail request on its existing Wasmtime
event loop. The request synchronously acquires tail-work accounting before send;
dropping the observer does not drop that accounting or cancel finalization. Its
receipt follows the real `publish_incomplete_replay_tail_access`, including pending
automatic-update/card processing under the existing boundary lock. Losing the
Store returns an error, never a fabricated live transition. A finalization error
goes both to the receipt and, as a typed error, to the Store driver, so an absent
observer cannot swallow it. Nonprimary finalization is unchanged.

Four helper tests exercise synchronous accounting, observer/Store loss, blocked
root progress, post-root driver reentry, and error propagation using actual Wasmtime
with modeled finalization. Two real executor tests hold automatic-update
finalization, complete a provider promise, observe and release the reconstructed
body's real return, and assert that no invocation result, primary-tail result, or
original entity `End` escapes before finalization. Success checks the exact result,
one original-Start terminal, and operation drain. Failure checks the actual task's
injected error plus `FailedUpdate`, not permanent failure of later old-revision
recovery. The hooks observe actual finalization; they do not replace it.

Validation: `primary-replay-tail-settled-units.log` reports **4 passed, 0 failed**;
`primary-replay-tail-final-integration.log` reports **2 passed, 0 failed**. Commands
use the documented debug-disabled environment and local Wasmtime configuration:
`cargo test --config tmp/gol706/local-wasmtime.toml -p golem-worker-executor --lib --
primary_entity_replay_tail --report-time` and the corresponding `--test integration
-- primary_update_finalization --report-time --nocapture --test-threads 1`.
Bug-finder `gol706-primary-replay-tail` run 1: **no bugs found**. Oracle final review:
**APPROVED, ON TRACK**, for this prerequisite only. The initial Oracle ready-body
objection was addressed with the actual body-return gate, not a longer sleep.

Next bounded extraction, before moving the supervisor: replace only accepted
entity completion/replay with a concrete owned `GolemEntityInvoke`/`WriteLocal`
capability, retaining the current task, admission, resource tables and settlement.
Keep dropped-event drains in the Store adapter. Capture originating shared state,
not copies of atomic membership or liveness. Preserve terminal guards, span and
trap context, request-upload-before-append ordering, and incomplete-repair cleanup.
Prove the producer is host-owned at its executing-task boundary. Select markerless
terminal persistence before the first append await: suppressing a delivery token
afterward is too late if cancellation already generated a discard marker. Reject
real guest-delivery obligations rather than acknowledging or synthesizing markers.

For this extraction, do not preinstall a dormant primary listener. An unused runtime task remains
`Unknown` and can veto suspension indefinitely. At a primary `Incomplete` branch,
the current adapter installs and immediately requests finalization, then hands the
receipt to the owned continuation. Nonprimary continuation uses captured inputs to
the existing replay-to-live algorithm. General scope-closing/commit machinery is
unnecessary for WriteLocal with no forced commit. The next gates cover terminal
tears, guest-marker rejection, completed/incomplete/no-body/cancel paths, atomic
transfer, and the primary finalization tests above. Full operation custody and the
recorded-parent-cancellation regression still follow; Step 3 is not complete.

#### Owned entity completion/replay extraction: approved

Accepted entity terminals now use a concrete owned `GolemEntityInvoke`/`WriteLocal`
context sharing the originating oplog, replay, atomic registry and live counter.
The existing task, resource-table admission, body dispatch, and supervisor remain
in place. Primary incomplete replay still requests real Store finalization at its
branch; nonprimary replay uses the existing algorithm with captured owned inputs.
Owned terminals select markerless persistence before their first append await and
reject genuine guest-delivery obligations rather than synthesizing markers.

Review caught three regressions in the first draft, all corrected before approval:
observed append errors could queue an already-consumed JoinHandle; unpersisted
snapshot calls could append an orphan terminal; and cleanup registration used an
expired admitting activity rather than the executing task. The corrected complete
and cancel paths disarm observed failures, skip upload/span/append when unpersisted,
and register incomplete-repair cleanup at the executing Accessor's live transition.
The owned context no longer captures an admission runtime identity.

Tests invoke the new owned methods for complete and cancel: append fencing errors
and subsequent cleanup drainage; unpersisted return/accounting with no uploads,
span closure or oplog change; tears before append leaving Start incomplete; and
gated append surviving observer loss with its permit retained through drainage.
Delivery tests accept markerless replay-tail and reject Live/Armed/AtMarker/Discarded
obligations. A real Wasmtime task test retires the admitting activity before a
different executing task registers cleanup; the coordinator refuses to unregister
that activity until the actual owned terminal settles. This uses modeled session
inputs and an in-memory oplog, not full executor reconstruction.

Validation: `bugfinder-owned-entity-baseline.log`: **110 passed, 0 failed**;
`owned-entity-executing-activity.log`: **6 focused tests passed**. Final-snapshot
integration logs `owned-entity-final-review-integration.log` and
`owned-entity-final-review-reconstruction.log` report **2 + 3 passed, 0 failed**:
both primary update-finalization cases, incomplete attachment-upgrade rejection,
completed reconstruction barrier, and completed replay under current memory pressure.
All use the documented debug-disabled environment/local fork. `git diff --check`
passes. Bug-finder `gol706-owned-entity-capability` run 2 resolves its sole accepted
finding and reports no new findings; no further certificate-seeking run is needed.
Oracle: **APPROVED, ON TRACK**, conditioned on the now-passing final integration gate.
Step 3 and Steps 4–8 remain unfinished; the recorded-parent regression is not fixed.

#### Accepted payload extraction: bounded prerequisite approved

Actual stdin/stdout/stderr entries now leave the originating resource table
synchronously, before the accepted Store task can run. The native direct accepted
path uses the same boundary. Stable services, owner identity/execution and lifecycle
signal travel in a concrete execution context. Output writer materialization still
waits for execution-mode resolution, preserving replay memory-accounting policy.
The task itself remains Store-owned; this is not full continuation custody.

The first service-extraction draft also captured owner metadata and caller revision
early. Oracle identified a real ordering hazard: automatic-update finalization can
replace primary metadata between acceptance and either original read. Those early
snapshots were removed; both reads now remain at their exact baseline boundaries.
Oracle's initial verdict was **AT RISK**, corrected to **conditionally APPROVED,
ON TRACK**, conditioned on final-source tests that subsequently passed. Bug-finder
`gol706-accepted-payload-extraction` run 1 reported no bugs; the six passing
integrations in `accepted-payload-final-integration.log` preceded this correction
and did not establish revision-crossing coverage. The corrected snapshot passed
**8 integrations** in `accepted-payload-late-metadata-integration.log`, including
native direct sessions and capable-stream limits, plus **2 output units** in
`accepted-payload-output-units.log`. Neither review approves all of Step 3.

A preceding attempted monitor-only patch was rejected and removed: moving terminal
completion into the reconstruction monitor still sent cleanup-bearing resources to
a disposable Store receiver. It did not fix the recorded-parent regression. The
approved entity implementation was restored; no monitor-only fix is claimed.

Oracle approved the next narrow metadata protocol within the existing ownership
scope: one read-only publication per originating context lifetime, initialized from
its actual owner metadata and caller revision. Publish synchronously at both
existing metadata-replacement sites through one replacement helper, before later
fallible wallet work, matching the Store's actual state. Accepted work captures a
reader early but reloads separately at each original late boundary. Entity owner
metadata stays pinned and distinct from executable revision; native semantics stay
unchanged. After origin destruction readers retain its last snapshot, never bind
to a replacement context, and cannot infer finalization success or live permission.
The primary replay-tail listener remains a finalization mechanism, not a general
metadata-request server. Required tests cover sibling updates, updates between the
two reads, both publication paths, entity/native distinction, origin isolation, and
failure receipts despite readable metadata.

#### Origin metadata publication: bounded prerequisite approved

The per-origin publication now uses a private watch sender with immutable snapshots
and a reader captured by the accepted execution. Both existing mutation paths use
one replacement helper. Owner metadata and caller revision are still loaded
separately at their original late boundaries. The helper tests cover distinct
primary/entity/native values, mutation between reads, immutable earlier snapshots,
origin loss and isolation from a replacement. They do not prove executor replay.

Bug-finder `gol706-originating-metadata` run 1 reported no bugs. Oracle found no
implementation blocker and rated the work **ON TRACK**, but withheld bounded
sign-off pending a real automatic-update regression and final-snapshot checks.
The original metadata test build disappeared without a result; a subsequent build
overlapped test-hook edits and failed with missing hook types. Neither is a pass.

The new active integration test
`incomplete_tool_uses_metadata_published_by_real_automatic_update_finalization`
stages a capable tool before body dispatch, reconstructs through an actual automatic
update, and asserts Captured A → OwnerDispatch B → CallerMaterialization B. It uses
a separate guest fixture; existing cancellation fixtures are unchanged. Initial
attempts timed out waiting for the test's finalization gate. The first diagnosis
of a production deadlock was incorrect: both logs explicitly recorded successful
automatic-update finalization. Oracle traced it to the uninstrumented direct
finalization path entered by the checkpoint's atomic-region recovery. The fixture
also dropped the replay checkpoint sender instead of sending its success signal.

Those bounded test-infrastructure errors are corrected: the direct path invokes
the same finalization hook with existing failure recording, the hook documents its
Store-holding contract, and the reconstructed checkpoint receives its required
release. The temporary ignored-test attribute was removed; skipping is not an
accepted resolution. No reconstruction fence was moved or weakened. Oracle's
diagnostic trajectory verdict is **AT RISK, not DERAILED**, pending execution of
the repaired regression, existing success/failure controls, and negative controls
against frozen metadata. The first repaired run reports **2 passed, 1 failed** in
`tmp/gol706/metadata-publication-direct-gate.log`: metadata and failure controls
pass; the success control times out waiting for its reconstructed body.

Oracle traced this to a second test-setup race, not the publication delta: the
checkpoint HTTP request precedes admission of the provider's promise wait. In the
failed schedule that Start was absent, so reconstruction needed a fresh Start under
the same boundary lock deliberately held by update finalization. The test then
waited for the body before releasing that lock. The passing failure control had
already recorded the promise Start. The accessor debug message appearing only at
shutdown follows the gate handle's release-on-drop; it does not prove earlier
finalization. Direct-path instrumentation and reconstruction fences are unchanged.

The success/failure setup now waits for the original entity's incomplete promise
Start before requesting the update. A new identity assertion requires replay to
reuse that Start. All ready-result nonescape, terminal-order, exact-result and
drain assertions remain. The first attempt to observe that Start via `get_oplog`
timed out because the API sees persisted entries, not the live buffered Start.
The setup now uses the existing `commit_oplog` test helper before inspection.
`tmp/gol706/metadata-publication-persisted-promise.log` reports **3 passed, 0 failed**
(success, failure and real metadata-update integration). The scoped unit run in
`tmp/gol706/originating-metadata-primary-entity-replay-tail-units.log` reports
**6 passed, 0 failed**. Bug-finder `gol706-originating-metadata` run 2 reports
**no bugs found** for this materially changed snapshot.

The frozen-at-acceptance negative control in
`tmp/gol706/metadata-publication-frozen-negative.log` fails at the intended
observation assertion: Captured 0 → OwnerDispatch 0 → CallerMaterialization 0,
versus expected Captured 0 → OwnerDispatch 1 → CallerMaterialization 1. The actual
automatic update and guest invocation completed; this is not a timeout or build
failure. The temporary mutation has been removed and the subscription restored.
Scoped formatting and `git diff --check` pass. Restored-snapshot integrations report
**3 passed, 0 failed** in `metadata-publication-restored-final.log`, and units report
**6 passed, 0 failed** in `metadata-publication-restored-units.log`, both under
`tmp/gol706/`. Commands use the documented debug-disabled environment and local
Wasmtime config: `cargo test -p golem-worker-executor --test integration --
primary_update_finalization --nocapture --test-threads 1 --report-time`, then
`cargo test -p golem-worker-executor --lib -- originating_metadata
primary_entity_replay_tail --report-time`.

Oracle judges this prerequisite **ON TRACK, APPROVED subject to the final checks
above; those conditions are now met**. It found no additional production blocker
and requested no new abstraction or unrelated test layer. Bug-finder run 2 remains
clean; the restored source is the reviewed snapshot. A local source snapshot is
preserved under `tmp/gol706/metadata-publication-approved/`. This is bounded
metadata approval only; full Step 3 and accepted-operation custody remain
unapproved. No changes have been committed or pushed.

#### Accepted-continuation attempt: reverted, no completion credit

A bounded subagent attempted Store retention/destructive teardown before moving
the entire accepted continuation. Its prototype passed eight entity units
(`tmp/gol706/step3-entity-units.log`), but the exact recorded-parent test failed
in live setup: parent and child cancellations were not both recorded before the
timeout (`tmp/gol706/step3-recorded-parent.log`). It never reached the simulated
crash or the targeted replay orphan. The prototype was reverted; metadata
checkpoint files were subsequently compared byte-for-byte with their saved copy.

The subagent inferred a design blocker from a forwarding-oplog panic. The parent
and Oracle rejected that inference: shutdown starts at log line 43757, the test
failure is reported at 43802, and the panic appears at 43846. It does not establish
the cause of the earlier live-setup timeout. Because the prototype was reverted,
its precise failure cause cannot be reconstructed from current source alone.

Oracle trajectory: **AT RISK, not DERAILED**. No new scope approval is required;
the accepted scope still applies. The next production change must start at
acceptance and move every post-handoff continuation off the originating Accessor,
including native and pre-body branches. Destructive teardown comes after that
ownership transfer. Retention-only retries, shutdown-panic theories, and passing
helper tests do not discharge this requirement. No new implementation was retained
from this attempt, so it is not an approved implementation checkpoint.

The second assignment explicitly excluded teardown and targeted the acceptance-time
handoff. It made **no source changes**. Its exact baseline run in
`tmp/gol706/accepted-handoff-recorded-parent.log` reached simulated restart and
reproduced the targeted failure: parent Start 15 Cancelled, child Start 24
SelectingCancelled, no owner failure selected. This is evidence of the unresolved
bug, not implementation progress or a new cause.

After inspecting that evidence, Oracle changed the trajectory verdict to
**DERAILED: pause implementation**. Its earlier AT RISK recovery action was the
actual accepted-continuation handoff; neither assignment delivered it. No smaller
independently complete custody boundary was established that avoids hiding the
source drop-event drain, primary-listener dependency, and terminal cleanup-sink
obligations. Moving only post-body settlement would omit accepted-but-unpolled and
pre-body paths. This is not proof that the approved handoff is impossible.

The required invariant remains: before an accepted operation can lose its source
Store task, an independently retained owner must hold the entire remaining
continuation and settlement obligations; the Store may retain observation only.
Implementation is paused per the user's reporting requirement. No third equivalent
assignment or baseline rerun is authorized by this checkpoint. The existing scope
approval remains valid; resumption requires a changed execution approach, not a
new scope vote. Metadata remains approved, Step 3 remains incomplete, and Steps
4–8 remain unfinished. All changes remain local, uncommitted and unpushed.

The next full-continuation handoff needs these ownership changes beyond this
extraction, source-reviewed with Oracle as **ON TRACK**, not yet executed or proven:

1. Register accepted-operation activity and cleanup before first poll, synchronously
   extract actual attachment entries and services, and transfer the whole payload
   to an owner-retained supervisor before returning an observer. Route native direct
   acceptance through the same boundary. Body activity alone misses pre-body and
   skipped-body obligations. Remove cleanup-bearing result channels to parent Stores.
2. For a primary operation that may need incomplete finalization after handoff,
   preinstall the narrow listener with a proved runtime dependency on the accepted
   operation. Use `certify_runtime_dependency`, not guest-root certification. Drop
   the certificate before processing a request; revoke the operation's own wait
   classification before sending it. Retain synchronous request tail accounting,
   real finalization, and driver-authoritative errors. This replaces the branch-local
   adapter only when the entire continuation moves; an uncertified dormant listener
   is still forbidden. Unknown staging/finalization work remains ineligible.
3. Transfer existing runtime cleanup with `transfer_with_child(accepted_activity)`;
   moving a token alone keeps a dead runtime activity Unknown. Register repaired live
   cleanup against the accepted activity at transition. Own terminal append cleanup
   in the supervisor rather than leave a fallback queue in the disposable source Store.
4. Preserve source-context dropped-event drains during destructive teardown. The
   pinned fork's `Store::into_data()` destroys fibers/concurrent futures under Store
   TLS and returns the context; it does not run another guest event loop. Move the
   body-owner abort boundary so it retains the Store until this extraction, destroy
   resource-table entries while the receiver lives, drain the returned context,
   then acknowledge local cleanup and settle descendants via owner services. The
   current `Abortable` around the entire runner loses that context on cancellation.
   Any overlapping resident drain ticket must transfer to this teardown owner;
   dropping a listener/channel never acknowledges an unperformed drain.

These are implementation obligations, not completed guarantees. Required evidence:
accepted-but-unpolled child and native path survive recorded parent cancellation;
primary request revokes a prepared suspension; missing result observer cannot hide
finalization failure; source destruction retains actual drop events/receipts without
re-driving the Store; transferred cleanup leaves no dead-runtime blocker; whole-owner
stop retains its distinct non-fabricating semantics. The full recorded-cancellation
regression and remaining lifecycle matrix stay mandatory.

The new `cancelling_replayed_early_return_parent_retains_accepted_child_cleanup`
integration passes (3.225s), and its early-return/restart control passes (2.555s),
in `tmp/gol706/nested-reconstruction-abort.log`. It confirms replay parent Ordinary,
child Open, then uses a second **simulated crash**. That is whole-generation
cancellation, not the precise recorded-parent-cancellation path. It neither proves
nor disproves the narrower orphan scenario. Do not label it a reproduced bug.

Other evidence: `tmp/gol706/entity-supervisor-integration.log` reports **4 passed,
0 failed, 39.002s** (tool clock races, nested guest trap, completed-replay memory
pressure). `entity-supervisor-gated-units.log` reports **182 passed** before the
latest guard-custody refactor; `entity-guard-custody.log` reports **34 passed** for
that bounded draft. Final current-snapshot validation in
`entity-checkpoint-before-pause.log`: **182 passed, 0 failed**, using
`cargo test --config tmp/gol706/local-wasmtime.toml -p golem-worker-executor --lib --
worker::suspension worker::entity_invocation durable_host::entity
durable_host::concurrent durable_host::tool::operation --report-time` with the
documented debug-disabled build environment. Formatting and `git diff --check`
pass. Integration results predate the final guard-custody corrections. None of these
constitutes step approval. The adapted Abortable unit probes model supervision and
exercise real retained resources; they do not execute full admission/slot/lane setup.
Bug-finder run 1 and Oracle have not approved the complete current checkpoint;
the exact production abort ownership and nested operation gates remain open.

Current local supervisor uses a body-only `futures::Abortable`; the task retains
activity, slot registration and acquired lane permit. Ticket acquisition is inside
the abortable scope; a reconstruction abort does not run the ordinary finalizer
with a fabricated interrupted result. Normal finalization stays owned. Parent-end
preparation remains at its existing consumer boundary, before permit release and
descendant settlement. Native and component contexts now receive the reconstruction
teardown latch. These changes still lack the complete production guard-order and
nested accepted-operation proof; no automatic successful receipt on Drop is used.

#### Proposed approval scope: accepted operations own their full continuation

Oracle recommends an executor-side ownership refactor, **not** retaining the parent
Store and inventing a cleanup-only runtime driver. The latter requires a new
Wasmtime guarantee excluding guest execution across legacy fibers, callbacks and
reallocation; our probes do not establish it. No additional fork change is proposed.

On approval, first reproduce the exact recorded-parent-cancellation path described
below. The passing whole-generation crash case is not sufficient justification for
the refactor. Use that result to confirm or revise the source-backed diagnosis before
changing the accepted-operation durability boundary.

1. Make the pre-Store-task handoff in `spawn_tool_execution` synchronous. Register
   admission and accepted-operation accounting; take actual owned attachment entries
   rather than retain caller-table resource indices; capture dispatch/services and
   durability capabilities; spawn an independently owned operation supervisor before
   returning an observer. Cover accepted-but-not-first-polled work. Route native
   direct accepted execution through the same ownership boundary.
2. Move the entire accepted-operation continuation, not a backup clone, into this
   supervisor: body handle, actual selected terminal, reconstruction fences, retained
   resources, stream sessions, persistence receipts and cleanup obligation. Store
   task/result resources become observers. Observer loss is not cancellation.
3. Move entity reconstruction and the existing Completed/Cancelled/failure settlement
   branches into that owner. Eliminate the cleanup-bearing channel to the Store-owned
   receiver; publish only the result after owned settlement. Preserve body abort/join,
   exact terminal arbitration and ordinary finalizer semantics.
4. Introduce an entity-specific owned durability capability for accepted background
   `GolemEntityInvoke` (WriteLocal), rather than a generic detached-host-call layer.
   Capture shared oplog/replay, terminal-persistence, liveness/admission, live-call and
   cleanup capabilities. Keep generic Accessor-bound completion for real guest imports.
   Verify that background entity producers are markerless by construction; consume
   real recorded Cancelled/End and await live terminal persistence without synthesizing
   delivery/discard markers. A genuine recorded guest-delivery obligation must not be
   suppressed or silently converted to background completion.
5. Share the existing atomic-region registry under one synchronized owner between
   the parent context and owned entity-call context. Preserve registration, transfer,
   detachment and side-effect algorithms. Terminal persistence must update the lease's
   **current** region membership, serialized against region close/transfer. Capturing
   the original region index or copying the registry is incorrect. This additional
   durability boundary makes the change larger than the previous three-file correction.
6. Preserve ordering: join body; prepare parent-end; resolve actual operation winner;
   release lane at its existing terminal boundary; settle descendants/resources;
   release registration/accounting; publish result. Parent recorded cancellation must
   not recursively abort independently accepted children. Children follow their own
   recorded outcomes or incomplete-repair paths. Owner-wide committed stop remains a
   separate explicitly authorized path, never inferred from observer loss.
7. Prove the exact recorded-cancellation case with nested actual replay: child unpolled,
   admission-blocked, running and SelectingOrdinary; observer loss before/after body
   completion; completed/incomplete component and native children plus a grandchild;
   held persistence/cleanup; atomic-region close/transfer. Assert exact oplog markers,
   side-effect counts, no orphan operations, correct guard ordering and follow-up use.
   Obtain Oracle and bug-finder approval before resuming later plan steps.

This is estimated **XL / several days including production tests**. Non-goals remain:
no second scheduler, replacement Store, generic detached call framework, replay
tolerance, operation-table clearing, synthetic Cancelled/EOF/discard, fabricated
owner failure for recorded cancellation, or acknowledgment of lost cleanup.

### Entity accounting review checkpoint (2026-10-03)

The real external-tool timer regression now passes with entity-to-runtime-root and
parent-to-entity certificates restricted to the actual guest export/body wait.
Admission, response materialization, finalizers and teardown remain Unknown.
`tmp/gol706/entity-accounting-real-clock-races.log`: **2 passed, 0 failed, 28.292s**.
The all-long case checks Suspended, actual unload, resumed result 12, one HTTP effect
and follow-up invocations; the short races also pass. This is not whole-step approval.

Oracle rates the checkpoint **AT RISK, not DERAILED**, withholding bounded acceptance
for two retained-receipt defects: failed parent-end preparation could be hidden by
later successful settlement, and the non-winning owner-failure observer held the
receipt outside independently retained settlement. Local corrections retain the first
preparation failure until final acknowledgment and spawn the complete non-winning
resource/operation/receipt settlement. Dedicated held-cleanup failure/observer-loss
regressions still need to be added; passing existing tests does not establish them.

Bug-finder run 1 (`gol706-entity-accounting`) additionally identified a real production
ownership gap: legitimate replay cancellation hard-aborts the entity task containing
its activity and receipt. The submitted reproducer is not a valid positive test:
it both models unauthorized loss and omits a blocked driver. Oracle confirmed the
production gap while requiring unauthorized loss to stay fail-closed. The test is
now a negative test with a blocked driver, not evidence that authorized replay
teardown is fixed.

A local supervisor implementation keeps accounting outside the abortable inner
task, marks reconstruction teardown before abort, joins task destruction, and
returns retained scoped cleanup even after an authorized abort. Normal completion
retains the real Store; panic/unexplained abortion retains a failure. Component
contexts include the reconstruction teardown latch in their teardown probe. This
subchange is **not yet approved**: native latch propagation, nested operation custody,
abort-before-first-poll, completion-versus-abort, panic, and actual replay integration
need direct review and executed tests. It must preserve parent-end/terminal/lane/child
settlement ordering rather than wait for all children before releasing a parent.

`tmp/gol706/entity-accounting-correction.log`: **179 passed, 0 failed**, covering
coordinator, entity invocation, concurrent durability, entity reconstruction and
tool operation units. Scoped formatting and `git diff --check` pass. This validates
the existing suite and raw-runtime certificate revocation, not the missing supervisor
matrix or actual replay-created parent stop/cleanup gate.

Remaining sequence: finish these ownership regressions and both review gates;
prove actual replay-parent acknowledgment before held child cleanup and retained
teardown blocking unload/reuse; implement explicit primary-idle accounting; complete
the remaining Step 3 coverage and Steps 4–8. No acceptance criterion is removed.

### Entity cohort in progress (2026-10-02)

`tmp/gol706/entity-checkpoint-baseline.log` executes the two unchanged external-tool
clock cases against the integrated timer/promise implementation: short races pass;
the all-long race fails because it returns `U64(12)` without entering Suspended.
This is a remaining regression, not an accepted reduction in suspension coverage.

Oracle rates this checkpoint **AT RISK, not DERAILED**. The primary Store is actively
driving an unclassified `NativeToolTask` in this case; its caller/body dependency
must be certified only around the exclusive body-result wait, not admission,
output drains or terminal work. Separately, a truly idle primary Store currently
has a lifetime-long driver registration without a current blocked witness. Store
lifetime, active driver runs and explicit idle executor phases need distinct
accounting. Absence of an active poll is not evidence of quiescence.

Before enabling entity eligibility, close the parent cleanup cycle: live
`GolemEntityInvoke` owns a runtime cleanup obligation while awaiting its child;
the child's stopped imports await all runtime obligations before releasing their
errors. Parent abandonment/acknowledgment must precede child join. Retained entity
resources and teardown receipts remain a separate unload-complete obligation.
The local parent-session change passed bounded review; it does not enable eligibility.
Its initial 75 concurrent tests passed. Bug-finder reproduced cancellation taking
precedence over an already committed stop and deadlocking that same parent/child
barrier. The fix keeps observing stop even after cancellation selects, acknowledges
the parent first, and retains the child. New tests exercise the production helper
with both stop-before-cancellation and cancellation-before-stop ordering.
`tmp/gol706/entity-parent-cleanup-verified.log`: **119 passed, 0 failed**, covering
concurrent durability, entity reconstruction and coordinator tests. An earlier
validation attempt failed compilation because the test placed `futures::poll!` in
a synchronous closure; the corrected test executes both orderings. Bug-finder
`gol706-entity-parent-cleanup` run 3 reports no bugs found. Its additional ready-body
probe also passed (`tmp/gol706/bugfinder-stop-ready-probe.log`). Oracle accepts this
bounded building block conditional on passing validation, now satisfied, while
retaining **AT RISK, not DERAILED**. This is not entity-checkpoint or step-3 approval.
The repaired incomplete-replay path still needs equivalent parent custody before
entity eligibility is enabled; completed replay must preserve its current fences.

Component retention is now locally implemented but not approved. Component Store
contexts install a lifecycle token before instantiation, separate from local tool
cancellation. Slot cancellation increments the engine epoch and continues driving
the pinned body. The existing owner-failure winner supplies its interrupt reason.
`tmp/gol706/entity-retained-cancellation.log`: the existing blocked-sibling trap and
short tool-clock-race tests both pass (**2 passed, 0 failed**). The real provider,
caller and middleware fixtures were built via the CLI; no tracked migration edits
were produced.

Oracle remains **AT RISK** and identifies a nested self-drain dependency: entity A's
Store task can await retained failure cleanup for child B while that cleanup waits
for A's slot to drain. The lifecycle signal does not interrupt this cleanup observer.
The proposed correction releases only the entity-context observer after cleanup
has transferred to owner-retained custody; the primary/owner boundary must still
join cleanup. Do not select/drop the entire execution or cancel retained cleanup.
The new `nested_guest_trap_drains_waiting_ancestors_and_blocked_siblings` case wraps
the existing trap fixture in pass-through middleware and checks all four Store
destruction receipts, exact trap provenance, drained owner state and absence of
fabricated entity terminals. Contrary to the predicted deadlock, its before-fix run
passed in 5.067s (`tmp/gol706/nested-entity-stop-before-fix.log`). All four bodies
finished before the fence drain in that execution; output-stream failure may
provide independent progress. No speculative cleanup-observer fix was applied.
The scalar variant, with a minimal awaited-child trap in the provider fixture and
no output-stream signal, reproduced the cycle: **2 passed, 1 failed**, scalar timeout
at 31.207s in `tmp/gol706/nested-scalar-stop-before-fix.log`. The retained middleware
ancestor still had its Store attached after owner Trap selection. This establishes
that the streaming passes alone did not prove retention safety.

The bounded correction captures the calling entity Store's lifecycle token and
releases only its cleanup observer after `start_owner_failure_cleanup` transfers
task/result custody to the owner. Primary callers still await cleanup unconditionally;
the original error is preserved, no cancellation terminal is synthesized, and an
already-ready cleanup result takes precedence over releasing the observer.
`tmp/gol706/nested-scalar-stop-fixed.log`: **3 passed, 0 failed, 8.852s** for the scalar
and both streaming trap cases. Assertions still require exact original trap, Store
destruction before failure notification, drained slots/lanes/operations, no fabricated
entity terminals and exactly one owner error. The two retained-cleanup custody/error
tests also passed (`tmp/gol706/bugfinder-component-retention-owner-failure-cleanup.log`).
Bug-finder `gol706-component-retention` run 1: **no bugs found**. Oracle approves this
bounded component-retention checkpoint and judges **ON TRACK to continue**, with no
blockers in the observer correction. This is not approval of step 3 or eligibility.

Next bounded change: native retention, followed by incomplete replay-live parent
custody. Preserve pre-start cancellation without polling/effects; an already-started
native handler must receive a distinct lifecycle unwind signal and retain its future,
context and resources through cleanup. Do not replace select/drop with an uninterruptible
await or reuse local MCP cancellation, which writes a durable cancellation response.
Acceptance requires gated asynchronous native cleanup without stream attachments,
owner completion/reuse blocked until cleanup, selected reason and terminal integrity,
local cancellation/readiness races, and existing native modes/streams/cancellation/
overlap/replay plus all three component-trap regressions. Entity eligibility stays
disabled until native retention, replay-live custody, explicit executor-phase accounting
and unload-complete parent-resource custody pass their gates.

Native retention is locally implemented and **approved as a bounded checkpoint**. Oracle approved
the concrete-context cooperative contract: expose an owned lifecycle-only interrupt
future on `DurableWorkerCtx`, retain the existing macro API, and require indefinite
native waits to observe stop, finish cleanup, then return the typed interrupt. Arbitrary
Rust futures cannot be forcibly unwound while retaining their asynchronous cleanup;
adding an injected handle would not enforce cooperation either. Setup remains
cancellable before first handler poll. Once started, the handler and context remain
owned through cleanup and parent-end. Local tool cancellation remains distinct.

The adapter now forwards `anyhow` errors without stringifying typed interrupts or
call-owned traps; the native runner performs classification while context is still
available. MCP uses the same boundary and observes lifecycle stop at transport,
presence refresh, unauthorized feedback and stdout backpressure, without synthesizing
`Cancelled`. Already-selected durable writes retain their existing ownership. The
fixture handlers observe lifecycle stop, including a no-attachment native body whose
asynchronous cleanup is explicitly gated and whose future drop is separately observed.

`tmp/gol706/native-retention-unit.log`: **69 passed, 0 failed, 1 ignored** (existing
ignored case), covering native and durability filters. Initial attempts failed due
to a missing test-utils dependency and a test owner-ID field mismatch; neither was
reported as a test pass. Bug-finder `gol706-native-retention` run 1 reports **no bugs
found**, including a passing typed-interrupt-context probe in
`tmp/gol706/bug-finder-native-typed-precedence.log`. Integration acceptance passed in
`tmp/gol706/native-retention-gated.log`: **6 passed, 0 failed, 13.681s**, covering
the no-attachment native cleanup gate, MCP lifecycle reconstruction, existing native
behavior and all three component trap regressions. Existing MCP cancellation and
offline replay regressions passed in `tmp/gol706/native-mcp-replay-regressions.log`:
**3 passed, 0 failed, 25.378s**. The tightened disposal assertion checks the receipt
immediately after interrupt completion, not after joining invocation completion;
`tmp/gol706/native-retention-disposal-order.log`: **1 passed, 0 failed, 2.461s**.

Oracle approves this bounded native-retention checkpoint and rates it **ON TRACK**.
This is not approval of step 3 or native suspension eligibility. Review
required two test corrections: native disposal must match the interrupted Start,
not a later counter-read context; MCP restart must preserve incomplete presence/output
work rather than demand an unrelated invocation finish while those waits remain
blocked. The corrected test retains the live server/credentials to detect resends,
observes actual reconstructed presence/backpressure, then interrupts again. The
stdout fixture consumes one chunk before holding its live reader, and tests require
actual backpressure plus delivered bytes before stopping. Unauthorized feedback has
its own service gate rather than being omitted. The caller fixture was rebuilt with
the CLI; no additional migration changes were produced. No native or entity suspension
eligibility is enabled by these tests. Deterministic simultaneous lifecycle stop,
local cancellation and normal readiness remains a mandatory acceptance gate; separate
passing cases do not prove that arbitration.

Next bounded repair: incomplete replay-to-live parent custody. Oracle source review
found that replay-created handles currently retain live-call membership but do not
register suspension cleanup/coordinator observation. Simply routing them through
`await_live_entity_body` would therefore leave stop unobserved. Add that production
wiring at the reconstruction live-repair boundary, preserving stop visibility even
if commitment rejects new cleanup registration. Use one narrow continuation callback
for both `Incomplete` and `LiveAdmissionCancelled`, reusing the existing live helper.
Arbitrate saved body results without polling a completed future again; acknowledge
the parent before joining child cleanup; retain historical body membership until
actual settlement. Preserve completed replay and ordinary Skipped admission-cancel
semantics. Required tests include real replay-created handles, held child cleanup,
stop/cancel orderings and saved-result handling. Oracle rates this next repair
**AT RISK, not DERAILED** because callback-only wiring would be incomplete. No claim
that the currently unregistered replay handle already causes the parent-barrier
deadlock is established. Steps 3–8 remain unfinished.

Replay-parent custody is now locally wired. `replay_reconstruction_access` registers
cleanup at live repair and retains coordinator observation when registration is
rejected. Both incomplete branches use the production stop-aware continuation;
completed replay settlement timing is unchanged. The initial higher-ranked
`AsyncFnOnce` callback did not compile. An owned pinned body with ordinary
`FnOnce -> Future` resolves the lifetime/Send constraint without dropping the body.
`tmp/gol706/replay-parent-custody-owned-callback.log`: **124 passed, 0 failed**.
Bug-finder `gol706-replay-parent-custody` run 1 reported the compilation regression;
run 2 reports **no bugs found**, including a ready-result/local-cancellation probe.

The first integration run could not start because the large-dynamic-memory WASM
was missing. Its targeted CLI build produced the required artifact without tracked
migration changes. The subsequent unchanged integration run
(`tmp/gol706/replay-parent-custody-integration-built.log`) had **2 passed, 1 failed**:
incomplete attachment rejection and completed replay fencing passed; retained child
publication/reconstruction timed out after 300s. Logs locate the stall in forced
entity drainage while the test completion hook remains held. The test releases the
hook only after simulated crash returns. Retaining the entity body exposed this
test-hook cycle. A local correction makes only the before-invocation/completion
test hooks lifecycle-aware, preserving the body and cleanup, preserving an existing
body error, and replacing late success with the selected typed interrupt.
`tmp/gol706/replay-parent-custody-stop-aware-hooks.log`: **5 passed, 0 failed, 17.401s**,
covering those three unchanged reconstruction tests and both nested-trap cases.
Oracle accepts the hook correction and replay-parent continuation at source-review
level, conditional on these regressions passing, now satisfied. Bug-finder
`gol706-retained-body-test-hooks` run 1 reports **no bugs found**. This closes the
test-hook regression, not real entity suspension/cleanup acceptance or step 3.

Oracle continues to rate the entity checkpoint **AT RISK, not DERAILED**. The real
replay-parent committed-stop gate cannot currently execute because entity eligibility
is still default-denied. No force-suspend-worker switch or blanket certification is
accepted. Unit tests with manually assigned handle fields prove only helper behavior.
Keep the actual replay-created handle/child cleanup/unload acceptance open and combine
it with the following necessary accounting subchange, without marking step 3 complete:

1. Register a component entity execution activity before its task starts. The task,
   then retained resources, own it; dropping the observer must not remove it.
2. Keep that activity Unknown during admission, materialization, finalization and
   teardown. Only the exact generated guest-export await may certify a dependency on
   that Store/run's real runtime root; revoke the certificate before response work.
3. Certify the parent accessor only around its exclusive body-result wait, after
   incomplete replay resolution/registration. Do not certify the shared body future
   while it is polled alongside replay bookkeeping, or all of NativeToolTask.
4. Register a separate unload-complete receipt before eligibility and retain it with
   independently owned entity cleanup. It must not remain Runtime custody, which
   would cycle with stopped imports. Carry it through parent-end, Store settlement,
   slot/lane release and enclosing owner-failure cleanup. Unacknowledged loss fails.
5. Prove admission blocks preparation, the genuine long-wait dependency graph commits,
   the specific replay parent acknowledges before held child cleanup, and held
   retained teardown blocks unload/reuse. The unchanged all-long tool test must
   suspend/unload/replay, return 12, and perform its HTTP effect once.
6. Add explicit primary idle accounting in the same open checkpoint after the active
   external-tool path. Idle requires an owner-loop proof after tail settlement and
   permit release, no remaining runtime work, a stop wake, and atomic commit-versus-
   resume arbitration. Absence of polling is not proof; rejected begin_poll must not
   authorize execution. Native/background/non-flat-stream coverage and the full
   stop/cancel/readiness matrix remain mandatory, not waived by this sequencing.

The next integration must also preserve the distinction between owner lifecycle
stop and local tool cancellation. Component bodies already install local tool
cancellation late in invocation; replacing that token with runner cancellation
would lose the reason and conflict with its single-install contract. Install a
separate reason-aware lifecycle source before instantiation, retain component and
native drivers rather than selecting/dropping them, and preserve selected owner
trap/interrupt reasons. Retain root/parent-end custody through generated cleanup.

Acceptance remains the unchanged all-long tool suspension/unload/replay/effect-count
test plus gates before child-driver attachment, during actual child cleanup, and
during retained parent-end teardown. Native/background/non-flat-stream coverage and
forced-stop terminal persistence remain required, followed by steps 4–8.

### Resumed real-timer checkpoint (2026-10-02)

- `tmp/gol706/two-real-timers.log`: `wasi::p3_all_long_timers_suspend_and_resume_at_earliest_deadline`
  **PASS, 14.580s**. Actual SDK-generated 12s/60s timer imports, real persisted scheduler
  wake, Suspended status, unload, reconstruction, result 12, successful healthcheck,
  and exactly one HTTP effect. This is the first executed end-to-end coordinator slice.
- `tmp/gol706/adapter-cleanup-tests.log`: **99 passed, 0 failed** in concurrent
  durability and coordinator tests, including the actual-runtime delayed blocked
  callback after a concurrent wake. This closes the previous callback-publication
  reproducer at the real adapter, not by changing its contract.
- Bug-finder run 2 (`gol706-step3a-step4-prerequisite`): **no bugs found** for the
  bounded current integration review. This is not approval of the unimplemented cases.
- Oracle: **AT RISK — materially recovered from DERAILED**. The real timer milestone
  counts; continue directly. Approval withheld pending lifecycle repair and race gates.
  The old DERAILED verdict below is historical, superseded by this review.
- Oracle identified activation state surviving forced teardown as a concrete regression:
  an abandoned sibling could poison the next Store's eligibility, or leave it Stopping.
  The local repair retires the coordinator at the existing invocation-loop unload
  boundary and replaces it when that owner constructs its next instance. Observer Arc
  destruction no longer chooses activation lifetime. Voluntary unload awaits cleanup;
  forced reconstruction retires old observations without asserting successful cleanup.
  Old callbacks retain the old coordinator.
- `tmp/gol706/two-real-timers-gated.log`: baseline and both held sibling-cleanup
  arrival orders passed (14.372s / 14.357s / 14.331s). The overall run failed on a
  superseded forced-reconstruction test trigger: public oplog reads did not expose
  buffered timer Starts before suspension. This run is not wholly green.
- The revised reconstruction test gates after actual scheduler persistence and
  before stop commitment, then issues a real simulated crash. Its earliest timer is
  20s to retain eligibility after reconstruction; the ordinary case remains 12s/60s.
  Additional cases hold cleanup 13s past the earliest deadline and hold commitment
  until readiness invalidates the persisted plan (requiring no Suspend).
- Oracle follow-up: **ON TRACK**, prior activation-lifetime defect resolved, no new
  blockers for this bounded timer checkpoint. Approval is conditional on all six
  real-timer cases and affected coordinator/adapter/cleanup tests passing without
  weakening their assertions. `tmp/gol706/timer-checkpoint-races.log` completed:
  **99 unit tests passed and all six integration cases passed**, zero failures.
  Integration duration 95.943s; cases took 15.169s (wake during cleanup), 22.383s
  (forced reconstruction), 13.169s (readiness invalidation), 14.368s (baseline),
  14.397s / 14.389s (held first/second cleanup). This supersedes the
  earlier AT RISK judgment for the bounded checkpoint, not the remaining steps.
- Bug-finder run 3 (`gol706-step3a-step4-prerequisite`): **no bugs found** on the
  lifecycle repair and six-case checkpoint. Oracle's bounded approval conditions
  are satisfied. `git diff --check` also passed. No full-step approval is claimed.
- Reproduction: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=6 cargo test --config tmp/gol706/local-wasmtime.toml -p golem-worker-executor --lib --test integration -- worker::suspension::tests durable_host::concurrent wasi::p3_all_long_timers --report-time --nocapture --test-threads 1`.

This slice now covers readiness versus persisted-plan commitment, wake during held
cleanup past the earliest deadline, and same-worker forced reconstruction.
Scheduler errors currently become visible only after
the driver completes; preserve the driver while designing cooperative fault delivery.
Next expand to the unchanged full step-3 acceptance cohort (promise, background,
entity, non-flat stream and forced-stop gated terminal) and steps 4–8. No case is waived.

### Bounded promise cohort approved (2026-10-02)

`HostGetPromiseResultWithStore::get` now uses the common `invoke_access` lifecycle
instead of manually starting/replaying/completing its session. This places cleanup
custody before the common preparation drain and handles incomplete replay through
the same live-action error barrier as timers. A creator-owned live promise is
classified `DurableWake`; stop wins over readiness. The ephemeral/nonruntime path
remains pending the full migration.

Oracle judgment: **AT RISK**, with a concrete final-stop handoff gap, not DERAILED.
Promise completion persists its payload and requests activation, but an activation
can return while the worker still says Running after the invocation loop has
already checked `last_resume_request`; the loop can subsequently publish Unloaded
without rechecking under the lifecycle lock. A timer may hide the lost activation.
This sequence predates the promise refactor but must be closed before relying on
promise-only suspension. The common-driver refactor otherwise preserves replay,
creator validation, handle initialization and interrupt/trap categorization.

The bounded next gate pauses after the initial stop comparison, completes a promise
and awaits its activation attempt, then releases publication. The outstanding call
must deliver exactly `[23,169]` without another completion, timer or rescue
invocation. `tmp/gol706/promise-handoff-before-fix.log` reproduced this exact failure
(deadline after acknowledged completion, 12.025s); six other cases passed, including
ordinary promise completion, executor restart, all three short-timer regressions
and long timer/promise reconstruction.

The local repair routes all four suspension decisions through `publish_unloaded`:
instantiation, main invocation exit, delayed retry and unloaded permit wait. It
fences old entity bodies outside the lifecycle lock, then holds the existing lock
through the authoritative resume recheck, startup-result publication and final
state publication. Only this loop's Running attempt without requested retirement
can reject suspension; an external Stopping/deletion is not vetoed. Rejection
preserves pending startup. The complete stop epilogue remains shared.

Oracle follow-up: **ON TRACK**, no production-code blocker, approval conditional on
validation and directly observing the pending startup after rejection. This is a
bounded repair retaining existing timestamp ordering, not completion of monotonic
owner-wide activation arbitration.

`tmp/gol706/promise-handoff-fixed.log`: 99 unit tests and 12 integration cases passed;
three new handoff cases failed during test setup. Bug-finder run 1
(`gol706-promise-handoff`) surfaced the failures. Its proposed unload-timing cause
was not supported: the unpolled Live invocation had not begun. The actual helper
looked in default test dependencies while these variants use production active
agents. Tests now install gates directly on their retained production Worker, and
the unnecessary test-utils lookup wrapper was removed. A second gate observes the
same pending startup UUID after rejection and before reconstruction can complete it.
`tmp/gol706/promise-handoff-gated.log`: **9 passed, 0 failed, 40.029s**. The formerly
failing promise-only handoff passed in 2.287s, pending startup in 3.374s, external
stop in 2.346s, and mixed long timer/promise in 14.449s. Ordinary completion,
executor restart and all three short-timer negatives also passed. Bug-finder run 2
resolved its finding and reported no new reproducible bugs; no open findings remain
for this bounded cohort. Scoped formatting and `git diff --check` passed.

Oracle explicitly **APPROVED the bounded promise checkpoint, ON TRACK**, after
inspecting the corrected post-rejection assertions and completed logs. Premature
startup success would clear the pending UUID and fail the test. No additional
Wasmtime change was required. This does not approve full step 3 or monotonic owner
activation. Next: background/entity/native/non-flat stream cleanup and forced-stop
terminal custody, followed by remaining steps 4–8. No scope was waived.

Reproduction: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=6 cargo test --config tmp/gol706/local-wasmtime.toml -p golem-worker-executor --test integration -- api::promise api::p3_promise wasi::p3_short_timer_blocks wasi::p3_long_timer_and_promise --report-time --nocapture --test-threads 1`.

The local executor build now resolves the edited fork through
`--config tmp/gol706/local-wasmtime.toml`, pointing to `/tmp/gol706/wasmtime`.
The initial adapter compiled with `cargo check -p golem-worker-executor --tests`;
subsequent cleanup-custody edits require a fresh check and real execution. The local
Cargo.lock path-source changes are experimental and not a publishable dependency pin.

Oracle's earlier trajectory judgment: **DERAILED — pause and agree a revised execution
plan before another implementation attempt.** The required real two-import integration
milestone was missed repeatedly; useful runtime repairs and coordinator tests do not
substitute for that milestone. This verdict concerns delivery, not a demonstrated
impossibility of the architecture. Neither step 3 nor step 4 is approved.

The preceding AT RISK review required an explicit
dependency reorder: step 3's real eligible-stop gate depends on runtime observation
and identity from step 4 plus the thin driver/classification/persistence/commit
adapter from steps 6–7. Implement these prerequisites before claiming step 3 passes;
do not manufacture blocked witnesses or persistence receipts in the fixture.

Two bounded corrections were identified:

- Reporting one cleanup failure must not release an error while runtime-dependent
  sibling cleanup still needs the driver. Preserve the failure without discarding
  sibling ownership or indefinitely awaiting a dropped owner.
- Retain a parent cleanup obligation through driver/entity teardown and all its
  children. An empty registry alone must not race teardown-generated work.

The first correction is implemented and verified in
`tmp/gol706/step3-cohort-corrected.log` (22 passing tests): cleanup failure waits for
live runtime-dependent siblings, and unacknowledged drop records lost custody and
failure rather than a successful receipt. Parent ownership through actual teardown
is still unimplemented. This is primitive verification, not step-3 approval.

The recovered runtime callback probe also passed:
`CARGO_BUILD_JOBS=6 CARGO_INCREMENTAL=0 cargo run --locked > observed.log 2>&1 && python3 check-observation.py`
from `tmp/gol706/gol706-probe`. Decisive output:
`PASS: late-root, fairness, background, stream, selected work, active block, wake invalidation, traditional host`.
The local patch `tmp/gol706/runtime-observer-callback.patch` exposes a diagnostic
generation token and callback. It is not a production eligibility boundary, and it
does not establish activity coverage or atomic wake-versus-commit arbitration.

Next gate: integrate and execute step 3's actual-durability fixture with two imports,
background work, an entity and a non-flat guest-facing durable stream, including
stop/readiness coincidence and a separate gated-terminal forced-stop case. The
coordinator-only tests do not satisfy this gate. If the real eligible path requires
the planned narrow runtime hook, test that branch; if it instead requires excluding
supported cases, weakening replay or replacing general teardown semantics, pause
and present the concrete counterexample and scope decision to the user.

### Paused checkpoint, evidence and proposed resumption (2026-10-02)

The parent and bounded integration assignments have not connected the coordinator
to real durable session cleanup or driver commitment. Three integration assignments
failed to deliver the required slice. One asserted that another outer-poll hook was
mandatory, but source review identifies `run_guest_call_settled` as a concrete place
to wrap the actual `run_concurrent_and_settle` future with a poll guard. The reported
local Cargo source conflict is also not a proven design blocker: a controlled local
dependency/lock update remains a reversible integration experiment. Do not use either
unproven blocker to justify another prerequisite-only checkpoint.

Evidence preserved locally:

- `tmp/gol706/step3a-coordinator.log`: scoped executor coordinator suite, **24 passed**.
  Two additional tests cover atomic settlement against the exact latched stop and
  reject associating earlier settlement with a later stop. These are model tests.
- `tmp/gol706/runtime-observer.patch`: current local fork diff, three files,
  372 insertions / 25 deletions. No production dependency change or fork publication.
- `tmp/gol706/gol706-probe/identity-probe.log`: `CARGO_BUILD_JOBS=6 CARGO_INCREMENTAL=0 cargo run --locked --bin gol706-probe`
  from `tmp/gol706/gol706-probe` passed, reporting **650 unique balanced activities**.
  The host closure now asserts that its accessor identity was registered before its
  first embedder instruction. The test exercises pending and yielding imports, not
  complete production activity coverage.
- Import activity registration moved to `host_task_create`; `Accessor::runtime_activity`
  carries the immutable ID and projections preserve it. Spawned tasks receive distinct
  IDs. Activity lifetime no longer claims to be a fabricated driver run. Every wake,
  poll and driver-drop invalidation supplies a monotonically increasing watermark.
  Producer/consumer scoped activity identity and complete mapping remain unfinished.
- Bug-finder run 1 for `gol706-step3a-step4-prerequisite` found and executed
  `runtime-blocked-callback-after-wake-invalidation`: a delayed blocked callback can
  overwrite an earlier invalidation. Reproducer:
  `tmp/gol706/gol706-probe/src/bin/provisional_blocked_publish_race.rs`.
  The finding is accepted and **open**. The callback contract now describes retained
  watermarks, but no real owner adapter implements that contract yet; documentation
  and emission changes alone do not close the finding. No clean rerun is claimed.

Proposed resumption, requiring user agreement because of the requested derailment
pause: one directly owned implementation unit delivers the real timer slice:

1. Connect the fork to the executor using reviewed, reversible local dependency
   overrides. Bracket the actual runtime future's polls; apply IDs, blocked candidates,
   invalidation watermarks and run retirement under the owner coordinator lock.
   Reject stale publication and prohibit commitment during an active normal poll.
2. Register real session cleanup before preparation; retain ownership through terminal
   and delivery work. Connect actual wake-plan persistence, its receipt, owner commitment
   and stop broadcast. A host error cannot escape before runtime-dependent siblings
   explicitly settle, abandon or transfer to genuinely independent retained custody.
3. Execute two real timer imports. After valid commitment, gate one cleanup receipt and
   prove the other cannot release the first stop error. Reverse the order and coincide
   readiness with stop. Verify actual unload, durable activation, reconstruction, exact
   results/oplog observations and a successful follow-up. Test hooks control timing,
   not eligibility or persistence. Fix the publication race through this real adapter.
4. Obtain Oracle and bug-finder review of that executed slice, then expand to timer/promise,
   background/entity/non-flat-stream and forced-stop gated-terminal coverage. The initial
   slice is a checkpoint, not step-3 completion or reduced acceptance scope.

Remaining after that checkpoint: full steps 3–8, P2 dispatcher/lifecycle acceptance,
all supported wait migration, removal of old mechanisms, the five baseline failures,
and broader regression verification. No required case has been deferred out of scope.
If the real slice fails, preserve its actual compiler error or executed counterexample
and the specific decision needed; do not replace it with another open-ended probe cycle.

## Sources and exact baselines

- [Parent investigation and oracle consultations](https://ampcode.com/threads/T-01a0f7dc-408b-75ba-9018-72b0024ab10e).
- [Runtime quiescence probe](https://ampcode.com/threads/T-01a0f825-f253-742e-827e-d8d1abdb8151).
- [P2 readiness probe](https://ampcode.com/threads/T-01a0f826-55f9-75eb-bfad-20671756cecf).
- [Narrow P2 dispatcher probe](https://ampcode.com/threads/T-01a0fb17-7c37-73ce-8cf7-e27d32b5695e).
- Golem baseline: [accae0e435b5097cd1d7940a5c1f568bde8a055a](https://github.com/golemcloud/golem/commit/accae0e435b5097cd1d7940a5c1f568bde8a055a), the report's tested revision and this investigation's unchanged production checkout.
- Locked Wasmtime fork: [252ab61f67fc16e49c83575e8477a8c4eca13d0b](https://github.com/golemcloud/wasmtime/commit/252ab61f67fc16e49c83575e8477a8c4eca13d0b). All three child probes verified the lock and executable source revision.
- [Original WASI P3 Migration plan](https://app.notion.com/p/WASI-P3-Migration-360a4cb355ec80d2ac57de37a4cb0b90), retrieved with Unblocked.
- [GOL-82: suspendable wait and lazy-initialized pollable replacement](https://linear.app/golem-cloud/issue/GOL-82/suspendable-wait-and-lazy-initialized-pollable-replacement).
- [P3 migration PR #3638](https://github.com/golemcloud/golem/pull/3638), merged July 24, 2026; [merge commit](https://github.com/golemcloud/golem/commit/4f79f77bf).
- Pre-P3 pinned fork: [106dc8ea896dfb3ca1baef2462063e6db6569b2f](https://github.com/golemcloud/wasmtime/commit/106dc8ea896dfb3ca1baef2462063e6db6569b2f).

Use exact revisions for experiments, not a later `origin/main`. The parent has no
unpublished production changes to transfer. Transfer this document explicitly to
new child workspaces; thread messages and commit references do not transfer files.

## Required contract

Voluntary suspension is allowed only when the executor can establish that the
whole owner has no runnable work and every outstanding source of progress is:

1. a timer sufficiently far in the future; or
2. an explicitly supported durable wait with a detectable completion/activation
   condition, or a persisted bounded recheck and a valid recovery contract.

A long watchdog must not authorize suspension while a short sibling timer is due
soon. An unresolved promise must not override that short timer either. Unknown
work blocks suspension. A pending future is not automatically a suspendable wait.

Suspension in the middle of an invocation remains supported. Arbitrary crashes,
explicit interruption, shard loss, and reconstruction remain possible at any
point. Their correctness must not depend on this voluntary-suspension gate.
Replay must reproduce recorded observations; never weaken replay validation to
make suspension tests pass.

Quota/fuel/memory-driven lifecycle suspension and idle unloading have different
reasons and are not to be redefined as wait-based voluntary suspension.

## Detailed implementation sequence

This section is the current executable plan and takes precedence over the earlier
stage outline and chronological investigation notes below. We have enough evidence
to begin implementation. Remaining uncertainties are bounded acceptance gates in
steps 3 and 5, not a request for another general design investigation. No production
step below is completed yet. Do not push fork/application changes without approval.

### 1. Establish the regression baseline and recover the probe inputs

1. Record the current Golem revision, lockfile, toolchain and Wasmtime revision.
   Recover the child archives described below into scratch space. Keep observation,
   dispatcher and stop patches separate; they were not tested as one combined patch.
2. Load testing and durable-execution guidance. Identify the owning executor test
   targets and build only required fixtures using the test-component workflow.
3. Run existing P3 sleep, promise restart, mixed P2/P3 HTTP, RPC concurrency-slot,
   RPC-plus-HTTP and representative entity/stream tests before changing behavior.
   Preserve logs and baseline failures; do not attribute every failure to this fix.
4. Add discriminating GOL-706 cases: `[2s,60s]`, `[2s,60s,600s]`, all-long timers,
   and promise plus short timer. Exercise both agent and tool/entity entrypoints.
   Assert no premature unload, exact result, one logical side effect, consumed
   recorded completions and a successful subsequent invocation.
5. Strengthen P2 long-sleep testing to observe actual unload and later activation,
   not just elapsed time. Use framework lifecycle hooks rather than arbitrary sleeps.

Deliverable: named tests and a baseline result table. New regression tests may fail
on the baseline for the documented reason; they must pass before the final change
is accepted. Existing unexplained failures stay visible.

#### Executed baseline and review (2026-10-02)

Production remains at the revisions above. Tests and the host-api fixture are local,
uncommitted changes. Rust 1.98.1, cargo-test-r 2.2.5; executor commands use
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=6 cargo test
-p golem-worker-executor --test integration -- <filters> --report-time --test-threads 1`.
Fixture builds use the local `golem-cli build --yes` and manifest release/quick presets.

| Test (module-qualified suffix) | Baseline result |
|---|---|
| `api::p3_promise_suspend_survives_executor_restart` | PASS, 2.674s |
| `wasi::p3_sleep_suspends_and_resumes` | PASS, 26.467s |
| `wasi::sleep_longer_than_suspend_threshold` (strengthened unload check) | PASS, 14.389s |
| `wasi::p3_short_timer_blocks_suspension_with_long_watchdog` | FAIL: premature suspension, 2.185s |
| `wasi::p3_short_timer_blocks_suspension_with_multiple_long_watchdogs` | FAIL: premature suspension, 2.249s |
| `wasi::p3_short_timer_blocks_suspension_while_promise_is_pending` | FAIL: premature suspension, 2.161s |
| `wasi::p3_all_long_timers_suspend_and_resume_at_earliest_deadline` | TIMEOUT, 30.009s; unresolved production baseline |
| `wasi::sleep_longer_than_suspend_threshold_while_awaiting_response` | PASS, 6.249s |
| `wasi::sleep_longer_than_suspend_threshold_while_awaiting_response_2` | PASS, 16.183s |
| `wasi::p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold` | PASS, 16.600s |
| `tool_streaming::polling_loop_beats_watchdog_without_suspending_agent` | PASS, 4.404s |
| `tool_streaming::clock_races_complete_through_tool_entity_without_suspending_owner` | TIMEOUT, 120.008s; replay stall observed, precise cause unresolved |
| `tool_streaming::all_long_clock_race_suspends_and_unloads_tool_entity_owner` | PASS, 14.510s; unload, earliest timer, one HTTP effect and follow-ups |
| `resource_limits::concurrent_agent_limit_allows_rpc_progress` | PASS, 37.196s |
| `rpc::durable_streaming_output_recovers_after_executor_restart` | PASS, 1.430s |
| `rpc::rpc_suspension_retries_after_concurrent_http_wait_finishes` | PASS, 46.413s |

Logs: `tmp/gol706/step1-tests.log`, `step1-corrected.log`, and
`step1-rpc-baseline.log`. The first run's all-long agent failure used an incorrect
HTTP route and is excluded as executor evidence; the table uses the corrected run.
The tool long-wait test is independent so short-wait failure cannot hide it.

Oracle approved the regression-test design after both corrections. Bug-finder run 2
resolved its import/compilation finding and reported no new verified test defects.
Neither review approves production behavior. The three premature-suspension failures
and two unresolved timeouts remain acceptance obligations, not waived tests.

### 2. Implement the owner coordinator as a testable state machine

Owner: `worker/instance.rs::OwnerExecution`; place a focused sibling module under
`worker/` if needed for this coherent responsibility. Do not reuse Store-local wait
counts as proof. This step defines policy/state, not Wasmtime scheduling.

1. Define owner-incarnation, activation, participant and attempt identities. IDs
   must not alias recycled resource slots or callbacks from a previous activation.
2. Define activity states: unknown/running blocker, timer deadline, durable wake,
   durable recheck, and certified internal dependency. Distinguish RPC eligibility
   grace from recheck time. No-source or ambiguous internal waits remain blocking.
3. Implement Running → Preparing → Stopping → Unloaded, plus terminal retirement.
   Preparing stores a snapshot revision and retains its single persistence writer
   even if invalidated. Failed/invalidated preparation returns to Running only
   after writer ownership is settled; stale receipts cannot authorize a new attempt.
4. Implement short-lock registration, classification, per-driver poll guards and
   generation-qualified witness publication. A normal poll invalidates its own
   witness; owner membership/classification changes invalidate preparation. Do not
   require sibling drivers to share one continually changing witness generation.
5. Implement a pure eligibility decision over complete registrations, valid blocked
   witnesses, no active normal polls, and the earliest real deadline. Recompute
   remaining time at commitment. Keep coordinator grace notifications separate
   from application wakes; no repeated yield/readiness heuristics.
6. Implement logical activation sequencing separately from stop-control wakes.
   Retirement wins over all callbacks. Notifications happen outside the short lock.

Verification: deterministic tests enumerate both orders of admission/commit,
poll/commit, wake/publication, activation/unload and retirement/callback races.
Also test two Stores retaining valid witnesses, invalidated persistence still
running, failed persistence, stale receipts, timer becoming short without a wake,
unknown work, and cleanup notifications not causing resume. Use controlled clocks
and gates. These tests validate transitions, not completeness of runtime coverage.

Implemented locally in `worker/suspension.rs`, held by `OwnerExecution`. Oracle
approved the corrected policy-only implementation; bug-finder returned no bugs found.
`tmp/gol706/step2-corrected.log`: 16 tests passed, 0 failed. No runtime integration
or actual suspension behavior is claimed by this result.

The reviewed implementation qualifies IDs with a fresh coordinator UUID as well as
the durable fingerprint and activation sequence: replacing a cached owner does not
change the durable fingerprint. Logical activations invalidate parked and in-flight
driver observations before commitment and request a resident notification; after
commitment they retain a restart obligation. Runtime/control wakes invalidate
observations only and are not interpreted as durable activation. Active drivers
cannot be deregistered or concurrently polled. Fresh blocked observations survive
an invalidated preparation while its persistence writer is retained. Each immutable
attempt exposes its earliest timer/recheck deadline to the persistence adapter.

### 3. Select and verify the durable stopping mechanism

Owners: `worker/invocation.rs`, `worker/entity_invocation.rs`, durable call/session
wrappers and their existing trap/terminal-cleanup paths. Prefer an executor cleanup
acknowledgment barrier; do not start with the experimental runtime stop hook.

Implementation decision after production-source review with Oracle: use two
different completion conditions. **Runtime-safe** means every obligation has been
explicitly abandoned/reconstructible, settled, or transferred to a retained cleanup
owner that does not need the event loop about to be discarded. Only then may the
first stop error escape to Wasmtime. **Unload-complete** additionally joins drivers,
entity resources, append/marker receipts and teardown-generated cleanup before
publishing Unloaded. Existing live-call and tail-work counters cannot certify this:
dropping unfinished work also decrements them.

Registration must precede session preparation/spawn/transfer; ownership moves
through session, terminal guard, delivery token and drop event. Unacknowledged drop
is failure, not success. A retained driver is the cleanup executor, not a participant
whose termination the runtime-safe barrier waits for. Children register under an
existing parent before ownership transfer. After runtime-safe, newly admitted
cleanup must be Store-independent. Cleanup failure must not publish successful
suspension. The coordinator now checks registered cleanup before `finish_unload`,
but production teardown ownership must establish that registration is complete.

No single existing wrapper covers every stop exit. Gate common accessor-session
error returns after abandonment/transfer; explicitly attach custom promise/RPC,
tracked AccessorTask and durable-stream protocols. Do not block inside the wait
helper while its caller still owns an unfinished session. Do not implement task
stopping with select/drop. Replace entity select/drop with retained cooperative
execution, and cover streaming export/materialization as well as the ordinary
settled driver. Preserve terminal appends already selected; lifecycle teardown is
not a guest discard and must not invent CompletionDiscarded, Cancelled or EOF.

The decisive stream test uses `DurableInputProducer` and a non-flat payload. Stop at
its reconstructible source-wait boundary before handing a new value to Wasmtime's
Destination; journal work already started must settle or transfer. A pending append
blocks voluntary commitment. Separately test forced stop with a gated append to
prove cleanup ownership. Do not treat that forced-stop case as voluntary eligibility
evidence. Add a runtime first-error/dispatch hook only if the real fixture proves an
eligible execution path escapes these executor gates. Only the cleanup-ownership
primitive is implemented so far (`tmp/gol706/step3-cohort-unit.log`: 22 passing tests,
including the 16 policy tests). Oracle's checkpoint identified corrections recorded
above; this is not step-3 approval. The actual-durability gate remains mandatory.

#### Dependency reorder and integration boundaries (Oracle review, 2026-10-02)

Execute step 3 in two parts, retaining its full acceptance scope:

1. **3a — durable ownership and pre-error plumbing.** Register cleanup before the
   first await in common accessor-session preparation; retain ownership through
   preparation, session, terminal guard, delivery token and cleanup receipt. Runtime
   import identity must separately exist before first poll. A registration inside
   the live action is too late. The promise path drains before creating its session;
   `wait_for` performs an internal `now` session before its wait session. Account for
   these pre-session phases rather than treating the wait body as the whole import.
2. **Pull forward step 4 prerequisites.** Use actual runtime observation and activity
   identity, with poll/wake/driver-drop invalidation and the final-root queued-work
   wake correction. Preserve active-poll exclusion while the outer poll is active.
   A callback's atomic token read followed by a separate coordinator commit is not
   atomic wake-versus-commit arbitration.
3. **Pull forward the thin adapter from steps 6–7.** Connect driver poll guards,
   wait classification, actual persistence receipts, commitment and stop broadcast.
   Unknown work remains blocking. Test hooks may gate timing or observe boundaries;
   they may not supply suspension eligibility or acknowledge fictitious persistence.
4. **3b — execute the real two-import vertical slice.** Delay one import's actual
   cleanup after valid commitment and prove the other cannot release its stop error
   while the sibling needs the driver. Reverse their order; cover two timers and a
   timer/promise combination, coincident readiness, reconstruction, exact results,
   absence of invented cancellation/discard markers, and a follow-up invocation.
5. Expand to the required background/entity/non-flat-stream cohort and separately
   gate an already-selected terminal append under forced stop. No case is waived.

Place the common pre-error gate after explicit abandonment in
`run_live_action_access` and its deferred counterpart. Preserve the call-owned trap
context, finish or transfer real session-local ownership, acknowledge the current
obligation, await runtime-safe outside Store access/locks, then return the error.
The custom promise lifecycle branches require the equivalent operation after
`abandon_for_trap`. Use the exact owner stop token, not the old per-wait Suspend
error as evidence that the owner committed. Never await a barrier that includes the
current unacknowledged obligation.

Do not await an unconditional cleanup wrapper after `invoke_access`: successful
completion observer arming must remain the host function's tail operation. Arbitrate
stop before choosing normal completion. Already-selected terminal appends must
settle or transfer instead of being retroactively abandoned. A queued DropEvent is
not inherently Store-independent; existing cleanup can still require Store access.
Keep root/teardown ownership through every event-producing resource's destruction
and resulting cleanup, but do not make runtime-safe wait for its own driver to exit.

Oracle trajectory: **AT RISK, not DERAILED**. This adjustment corrects dependencies;
it does not authorize activation with partial coverage or shipping two competing
suspension authorities. Pause if the real fixture requires forged quiescence,
weakened replay, or exclusion of a supported participant.

1. Inventory what each eligible participant owns at stop: durable handles, root
   state, append tasks, retained entity resources, background jobs and stream ends.
   Define acknowledgment as recoverable/drop-safe ownership, not body completion.
2. Retain the outer driver and entity invocation future on lifecycle cancellation.
   Convert cancellation to a stop request and continue driving cleanup. Do not use
   select/drop on unfinished durable drivers or await owner shutdown from a task
   that shutdown itself must drain.
3. Give latched stop precedence over normal readiness at eligible completion/effect
   boundaries. Preserve existing `abandon_for_trap` semantics, pending terminal
   persistence, and completion-delivery rules. Do not invent Cancelled or EOF.
4. Register cleanup ownership before spawning/transferring it. Include cleanup
   children and root obligations in acknowledgments. Do not require a driver to
   terminate before releasing the barrier that lets that same driver terminate.
5. Run a focused actual-durability test with two imports, background work, an entity
   and a relevant guest-facing stream. Gate a terminal append, make stop/readiness
   coincide, and assert the first error cannot strand sibling cleanup. Reconstruct
   afterward and check oplog observations and side-effect counts.

Decision gate: retain the executor barrier if all required eligible paths reach a
drop-safe state without normal guest continuation. If a concrete eligible path
escapes or cannot finish, add only the necessary runtime dispatch fence/first-error
retention hook, reusing its queues, and rerun this test. The raw fiber/queued-batch
negatives did not demonstrate eligible commitment and alone do not justify a larger
fork. Resolve guest-memory allocation needs for the tested stream stop path, rather
than freezing every kind of guest work indiscriminately. Do not silently drop a
supported suspension case to pass this gate.

### 4. Implement the minimal runtime observation and identity boundary

Fork owners: `crates/wasmtime/src/runtime/component/concurrent.rs`, existing host
task creation and stream-transfer paths. Follow dependency guidance for local testing.

1. Port the inner no-work observation, invalidating before driver polls and on
   wakes, with publication tied to the pre-observation generation. Include queued,
   selected-but-not-executed and final-root-created work in the proof.
2. Include the demonstrated final-root queued-work wake correction. A rejected
   blocked observation must not strand runnable work without another wake.
3. Expose opaque lifetime-safe runtime activity identities and creation/completion
   events wherever executor interception is not complete: imports, AccessorTasks,
   transfers and driver/root work. Register before first poll; unknown is blocking.
4. Invalidate on driver teardown and Store/activation retirement. Callbacks must
   not enter a replacement generation. Keep policy, deadlines, promises, scheduling
   persistence and cleanup-cohort decisions outside Wasmtime.
5. Integrate any stop hook selected by step 3 as a separate, narrowly reviewed
   runtime responsibility. Test its interaction with observation; do not simply
   concatenate independently tested experimental patches.

Verification: port fairness, selected-batch, zero-wake, completion-not-delivered,
unknown task, stream-with-no-import, wake/publication and identity/drop/reuse
counterexamples into fork tests. Cover all supported stream directions. A pending
Store-retaining fiber cannot publish a valid inner-blocked witness. Run shared
precompiled-engine compatibility testing when integrating the changed fork.

### 5. Integrate P2 without rewriting generic WASI readiness

Owners: fork host binding/linker mechanics; executor `durable_host/io/poll.rs` and
`clocks/monotonic_clock.rs`. Use the proven per-call dispatch mechanism, reviewed as
a production API rather than copied verbatim from its diagnostic implementation.

1. Resolve a nonempty all-recognized-timer poll set under Store access. Retain
   absolute deadlines and generation-qualified resource identities; preserve input
   validation, duplicates, overrides and resource lifetime rules.
2. Run owned timer readiness through concurrent host execution, allowing an inner
   blocked observation. The owner coordinator alone decides suspension.
3. Keep unknown/mixed source futures in the existing retained borrowed path and
   deny voluntary suspension while pending. Remove its independent suspend trap
   when the new authority is activated. Do not use whole-source mutex adapters or
   generic poll/drop/recreate. Preserve real filesystem replay readiness/state.
4. Cover timer `block` and applicable `ready` behavior as well as `poll(list)`.
   Audit blocking stream/read/skip/write/flush and replay paths: borrowed paths
   stay blockers, not accidental exclusions from accounting.
5. Exercise SDK-generated exports through the production executor driver. Preserve
   existing mixed HTTP/sleep outcomes, exactly-once request counts and follow-ups.
   If a supported independently scheduled task requires stronger progress, test
   that exact topology and adapt its source; do not demand same-Rust-poll reentrancy.

Decision gate: long P2 timers must remain suspendable through the common proof;
unknown readiness must retain state and existing semantics. Failure of an actual
supported independent-progress case requires a targeted decision, not a broad
source rewrite. The joined-future causal probe is not this gate's acceptance test.

### 6. Attach complete owner accounting and the lifecycle handoff

1. Register primary instantiation, pre-call work, driver runs and finalization.
   Register entity/native bodies before spawn in `start_entity_invocation_inner`,
   retaining registration through parent-end/resource settlement. Account for
   streaming results and post-return producers separately from root completion.
2. Connect runtime identities to executor wait metadata. Require explicit tokens
   at creation boundaries; an empty registry never proves coverage. Retain unknown
   classification for operations without a demonstrated recovery contract.
3. Integrate the single preparation writer with existing scheduler persistence.
   Store exact-attempt receipts. Work may continue during persistence but invalidates
   the snapshot. Revalidate deadlines, witnesses, membership and activation at commit.
4. Commit under the short lock with no active normal polls, fence new normal work,
   latch stop, and invoke step 3's acknowledged cleanup protocol outside the lock.
5. In `worker/mod.rs` and `invocation_loop.rs`, replace timestamp-based voluntary
   stop arbitration with a monotonic activation handoff. Acquire lifecycle lock
   before the short coordinator lock; never await under the latter. Atomically
   publish final state and any retained restart obligation after cleanup.
6. Make startup responsibility survive cancellation of the requesting future.
   Preserve shard-loss/deletion retirement precedence, concurrency-permit release,
   and separate quota/fuel/explicit-interruption behavior.

Verification: inject activation before persistence, during persistence, immediately
before/after commit, during cleanup and at final unload publication. Each case must
either abort suspension or eventually resume. Test sibling admission and delayed
cleanup, writer failure, stale callbacks, retirement and absence of restart loops.

### 7. Switch every supported wait to the single authority

1. Route P3 clocks and P2 owned timers to metadata plus ordinary readiness, replacing
   their local election/scheduling loops. Demonstrate long wait → actual unload →
   persisted activation → reconstruction → exact result and follow-up invocation.
2. Route Golem promises with their durable completion/activation contract. Verify
   completion before/after commitment and executor restart; avoid unnecessary rechecks.
3. Route RPC with separate eligibility grace and persisted bounded rechecks, durable
   intent and stable-key redispatch. Preserve one-slot concurrency progress and
   RPC-plus-HTTP behavior. Apply grace to each RPC, not just the triggering wait.
4. Classify durable stream waits only where completion activates the owner or a
   persisted recheck provides recovery. Source opening, journal writes, replay
   transactions and delivery remain blockers. Certified parent/child dependencies
   cannot authorize suspension without an accounted genuine wake source.
5. Replace passive/tail wait suspension choices without changing tail-settlement
   semantics. Verify native/entity bodies and post-return stream producers prevent
   premature suspension and retain registrations through cleanup.

Do not ship parallel old/new authorities. Internal construction may add inactive
plumbing in preceding steps; the final activation replaces all decision paths in
one coherent change. Development-only incomplete coverage must not become silently
reduced suspension support. Existing durability counters stay until their separate
uses are accounted for.

### 8. Remove superseded machinery and verify the complete change

1. Delete local `safe_to_suspend` authority, wait-count eligibility, per-wait wake
   preparation/election, RPC selectors and obsolete shortcuts. Keep durable-call
   ownership counters, completion receipts, replay validation and snapshot safety.
2. Audit every voluntary suspend/`TryStop` producer: it must carry the common proof
   or be a clearly separate non-voluntary lifecycle operation. No timer exemption.
3. Run step 1 regressions, real cleanup/lifecycle tests, P2 replay cases, RPC/promise,
   entity/tool and all affected stream suites; then the broader executor suites.
   Build fixtures explicitly, save logs, and repeat race-sensitive cases.
4. Run the reported polling-loop case through agent and tool entrypoints, including
   restart and follow-up. Investigate remaining replay divergence or SQLite stalls
   separately. Test scheduler isolation with a stalled action; this coordinator is
   not assumed to cure batch starvation or storage faults.
5. Run scoped formatting/lint checks, the bounded bug-finder workflow and final
   user-requested oracle review. Update durable-execution guidance with the actual
   final behavior, loading the skill-editing workflow when editing skill files.
6. Record exact commands/results, remaining defects and fork surface. Preparing a
   fork pin, commits and PR contents is local work; publishing/pushing requires the
   user's authorization. Do not claim deployed/merged delivery from local tests.

Definition of done: a single owner-wide proof governs voluntary suspension; long
eligible waits unload; short/unknown/runnable work prevents it; all required wake
and cleanup interleavings preserve recovery; supported P2/P3/RPC/promise/entity/
stream cases and existing behavior tests pass. Any unresolved reported replay or
storage failure must be stated explicitly rather than hidden behind policy success.

## Earlier stage outline

This is the execution order, not authorization to treat unresolved interfaces as
settled. Gates A and B must close before the production coordinator integration.
Do not preserve two production suspension authorities as a rollout strategy.
Do not reduce supported mixed-P2/P3 progress to minimize the fork.

### Gate A — P2 readiness dependency contract (investigating)

Inventory every production P2 pollable source and trace what makes it ready:
independent OS/Tokio work, Store-polled work, or guest/owner work. Include wrappers,
resource overrides, file replay and calls that await readiness outside `poll`.
Map each source to existing mixed-P2/P3 tests. A source label such as "socket" is
not proof that every await in its durable wrapper is independently driven.

For each source establish ownership, cancellation behavior, retained readiness
state, and whether a Store-retaining wait can prevent its completion. Choose the
smallest complete readiness support required by those dependencies. Keep a borrowed
path only where its supported progress contract is demonstrated; convert sources
requiring Store progress to owned readiness or revise the runtime integration.

Exit evidence: source/dependency table with exact locations, the selected P2
strategy and its limitations, and discriminating mixed-work tests. The raw
dispatcher probe alone does not close this gate. Do not introduce more timer-only
experiments that cannot answer the mixed-work question.

### Gate B — activity and lifecycle contracts (investigating)

Specify the complete activity-registration API and a small owner state machine.
List every runtime/executor creation, classification, completion and drop boundary.
Write invariants and a race table for witness publication, participant admission,
schedule persistence, commitment, wakeup, stopping, reconstruction and retirement.
Include a rule for existing drivers entering their next poll after commitment.

Review the concrete contract with the oracle, then execute deterministic transition
tests before coupling it to the full runtime. Unknown/unregistered work must deny
suspension. Identity reuse, stale-generation callbacks and an external wake during
the persistence/stop gap must have explicit outcomes. Source reasoning or a model
test alone is not evidence that runtime registration coverage is complete.

Exit evidence: selected API/state transitions, coverage table, oracle findings
adjudicated, and passing transition tests with counterexample cases. An unresolved
coverage or linearization gap keeps this gate open.

### Stage 1 — lock behavioral acceptance

Map `tests/wasi.rs`, `tests/api.rs`, `tests/rpc.rs`, `tests/resource_limits.rs`,
tool/entity tests and streaming tests to the validation matrix below. Strengthen
P2 long-sleep coverage to assert observed unloading, not merely elapsed time.
Add GOL-706 timer/polling regressions with agent/tool entrypoints and follow-ups.
Use the existing worker test framework, tagged fixtures and deterministic gates;
build required components separately and run every modified test.

Acceptance: tests fail for premature suspension, false residency-only success,
lost wakeups, or duplicate effects. Keep baseline failures visible and classify
them rather than weakening assertions. Planning/source inventory can proceed now;
code changes follow the gate decisions when they depend on new APIs.

### Stage 2 — implement the runtime boundary

In the pinned Wasmtime fork, implement blocked observation, invalidation and
necessary lifetime-safe activity identities, using existing runtime structures.
Implement the P2 support chosen by Gate A, not a guessed generic fallback.
Keep Golem suspension policy and persistence out of the fork. Regression-test the
final-root queued-work wake fix alongside the observation API. Preserve unrelated
upstream behavior and document the minimal fork maintenance surface.

Acceptance: all prior false-positive probes, unknown activities, all stream
directions, teardown and identity reuse are covered; P2 mixed work progresses.
If changing the dependency pin/configuration, run the shared precompiled-engine
compatibility test and follow dependency guidance. Publishing a fork revision or
pushing remains a separate externally visible action requiring authorization.

### Stage 3 — implement owner coordination

Integrate the Gate B contract under `OwnerExecution`, with admission/driver
registrations, classification, one preparation owner, schedule receipts, monotonic
wake sequencing and the lifecycle stop/resume handoff. Likely integration owners:
`worker/instance.rs`, `worker/mod.rs`, `worker/lifecycle.rs`,
`worker/entity_invocation.rs`, `worker/invocation_loop.rs`, and the runtime driver
in `worker/invocation.rs`. Exact helper placement follows existing ownership.

Acceptance: deterministic registration, wake, persistence failure and teardown
interleavings pass. Cooperative interruption is used; no unfinished durable driver
is dropped as the suspension mechanism. No new oplog records without a demonstrated
gap in the existing contracts.

### Stage 4 — complete the real timer vertical slice

Route real P3 and supported P2 timer waits through the owner coordinator. Persist
the wake plan, observe actual unload and deadline activation, reconstruct and
complete an SDK-generated invocation. Check follow-up invocation and forced restart.

Acceptance: long waits unload; a short sibling prevents suspension; multiple long
waits produce one owner preparation; registration/readiness changes during
persistence invalidate or update the attempt correctly. Run targeted worker tests
with logs saved. Do not count raw Store drop as this milestone.

### Stage 5 — cover every supported wait and owner participant

Integrate Golem promises, RPC grace/rechecks, entity and native bodies, stream waits,
background work and post-return producers under the same accounting authority.
Retain explicit blockers until replacement coverage exists. Keep RPC intent and
same-key redispatch semantics so suspension releases scarce concurrency permits.

Acceptance: one-slot RPC, RPC plus HTTP, promise restart, tool/entity reconstruction,
and durable input/output stream tests pass. Verify wake and side-effect counts,
not only results. No source-free wait accidentally authorizes suspension.

### Stage 6 — remove replaced machinery

Delete per-wait scheduling/election loops, `safe_to_suspend` as an authority,
Store-local eligibility counters/registries, direct legacy auto-suspend shortcuts,
and obsolete configuration/helpers only after their obligations are represented.
Retain counters used for other durability/snapshot contracts. Rewrite helper-level
tests around the new invariants rather than deleting the behavior they protect.
Update the durable-execution skill and executor walkthrough with the final behavior.

Acceptance: source audit finds one voluntary-suspension authority and no alternate
path that bypasses its proof. Validate affected documentation and walkthrough per
repository guidance; no broad unrelated cleanup.

### Stage 7 — close the reported bug and broader regressions

Run repeated GOL-706 reproductions for agent/tool calls, crash/replay checkpoints,
and scheduler isolation with an intentionally stalled agent action. Investigate any
remaining SQLite stall or replay mismatch independently; the coordinator is not
assumed to cure either. Scale verification from targeted suites to broader executor
coverage once integration works. Record commands, outputs and untested limitations.

Acceptance: no premature suspend, replay divergence, lost wake or cross-agent
wakeup starvation in the exercised cases; existing behavior suites remain passing.
If a baseline defect remains, name it explicitly and do not claim GOL-706 is fixed.
Before delivery, run scoped formatting/lint checks and the bounded bug-finder
workflow; user-requested oracle review can examine the integrated change.

## Gate investigations: source audit and contract review

The following findings are source-inspected and oracle-reviewed, not additional
executed runtime results. Both gates remain open. The oracle inspected local
lifecycle boundaries and the pinned fork's poll implementation; it did not rerun
the three probes.

### Gate A findings: execution progress is separate from unload eligibility

Preserve mixed execution supported by the actual component/runtime scheduling
contract. A synchronous P2 call need not allow reentrant polling of another branch
of the same Rust future. Independently schedulable work must retain its supported
progress guarantees. The earlier blanket rejection of a borrowed fallback was too
broad: retain it as a suspension-ineligible candidate, subject to those guarantees.
An owned HTTP/input wait may permit host execution while still blocking suspension
and without allowing the blocked guest future to observe its sibling's result.

| Source | Completion and retained state | Exact inspection boundary |
| --- | --- | --- |
| Timer | Tokio deadline; recreate against absolute deadline | executor `durable_host/clocks/monotonic_clock.rs:82-150`; fork `crates/wasi/src/p2/host/clocks.rs:138-160` |
| File streams | Blocking-pool task; readiness joins and installs data/error; replay must materialize real state | executor `durable_host/io/poll.rs:59-75`, `io/streams.rs:1460-1507`; fork `crates/wasi/src/p2/filesystem.rs:185-201,384-398` |
| TCP | OS readiness; connect/accept and writer completion mutate source state | fork `crates/wasi/src/sockets/tcp.rs:738-776`, `crates/wasi/src/p2/tcp.rs:285-312` |
| UDP | Socket resource immediately ready; datagram streams await OS readiness | fork `crates/wasi/src/p2/host/udp.rs:170-175,256-264,386-394` |
| DNS | Blocking-pool resolver; readiness installs iterator/error | executor `durable_host/sockets/ip_name_lookup.rs:35-64`; fork `crates/wasi/src/p2/ip_name_lookup.rs:79-85` |
| Async pipes/stdio | Reader/writer tasks independent; readiness consumes channel into buffer or observes capacity/flush | fork `crates/wasi/src/p2/pipe.rs:155-278`, `write_stream.rs:134-215` |
| HTTP response | May activate deferred send; joins task and installs response | executor `durable_host/http/types.rs:853-875`; fork `crates/wasi-http/src/p2/types.rs:446-464` |
| HTTP body/trailers/output | Readiness polls frames/handoff or sender capacity; retains body/error/trailers state | fork `crates/wasi-http/src/p2/body.rs:314-471,787-795` |
| RPC/promise pollables | Current P3 RPC has no subscribe; legacy promise map has no insertion sites | executor `durable_host/wasm_rpc/mod.rs:114-145,5526-5594`, `io/poll.rs:132-218` |

All fork locations refer to the pinned revision above. `DynPollable` stores a
table index and `MakeFuture`; `crates/wasi-io/src/impls.rs:44-197` creates and retains
table-borrowing readiness futures. Ordinary poll-list loser cancellation is already
supported. That does not justify repeatedly discarding an in-progress future after
each `Pending`, or assuming custom sources retain all progress outside that future.

Audit coverage includes `pollable.ready`, `block`, blocking stream operations and
file replay, not only `poll(list)`. Immediate non-retained probes can remain borrowed.
The current mixed P3 HTTP/P2 sleep test (`tests/wasi.rs:4131-4190`) checks eventual
result, request count and elapsed time, not HTTP/guest progress before sleep ends.
Existing P2 HTTP replay tests (`tests/wasi.rs:7127-7277`) do not establish mixed
Store progress. No corresponding mixed file/socket/DNS progress test was found.

The whole-source experiment is complete and rejects the long-held mutex adapter;
see its evidence below. Do not begin per-source rewrites based on the joined-future
stall. If independent guest progress remains disputed, compare two independently
admitted component tasks under one `run_concurrent` driver, mapping that topology
to a supported Golem path before treating failure as a production blocker. This
is distinct from separate-Store entity progress. Preserve duplicate grouping,
observer cancellation, deletion and generation semantics in any selected adapter.

### Gate B findings: narrowed coordinator contract

Use a short synchronous mutex under `OwnerExecution`, but do not hold it across
driver polls, storage awaits, lifecycle-lock acquisition, waker invocation,
cancellation broadcast or destruction of callback-bearing objects.

Identity scopes are owner incarnation, activation, Store, driver run and activity.
`OwnerExecution` survives replay-generation changes (`worker/instance.rs:245-280`);
reconstruction therefore needs new activation/run identities, not a reused witness.
Registration starts before spawn/admission and lasts through retained cleanup.
Every creation boundary must require a token: an empty registry cannot prove that
unregistered work does not exist.

Proposed operations, to implement and test before production integration:

- Register an unknown activity before it can execute. Classify only with an
  explicit recovery contract; classification/membership changes invalidate attempts.
- Begin normal driver poll under the mutex: deny after commitment, otherwise take
  an active-poll guard and invalidate that driver's witness, then release the mutex.
- Publish an inner-blocked observation against that driver's captured generation;
  end the poll guard before an owner commit can succeed. Use per-driver generations
  plus an owner attempt revision, not one global generation that makes sibling
  Stores continually invalidate each other's witnesses.
- Elect one preparation writer. Invalidating its attempt does not abandon the
  writer or allow overlapping writers; retain it until storage settles. Its own
  control bookkeeping does not invalidate itself. Ordinary persistence still blocks.
- Consume exact-attempt persistence receipts, recheck deadlines and all stored
  observations, then commit only with zero active normal polls. No arbitrary
  readiness callback may run under this lock.
- Latch a stop token and fence normal admission/polling. Denial retains the future
  and arranges cooperative cleanup driving; it must not drop or strand the driver.
- Finish cleanup using a lifecycle-locked atomic handoff, retaining any activation
  obligation. Retirement invalidates the incarnation and overrides resume.

| Race | Required outcome |
| --- | --- |
| Driver poll/admission versus commit | Earlier poll/admission invalidates proof; earlier commit denies normal execution |
| Wake during blocked publication | Old generation cannot publish a valid witness |
| Wake/classification change during persistence | Attempt invalidated; writer settles; no stale commit |
| Timer becomes short without another event | Final time recheck rejects commitment |
| Preparation receipt from old activation | Cannot modify replacement activation |
| Logical activation after commit | Resume obligation survives cleanup and unload |
| Coordinator interrupt wakes old host futures | Drives cleanup, does not alone request restart |
| Activation versus final unloaded publication | Activation is included in handoff or sees unloaded and requests retained startup |
| Retirement versus any old callback | No resurrection or mutation of replacement generation |

Postcommit activation must be distinguished from generic runtime/control wakes at
their source. Otherwise the coordinator's own interruption can cause an immediate
restart on every suspension. Before commitment, application wakes revoke witnesses;
after commitment, durable completion/activation or persisted rechecks own resumption.
Never infer wake purpose from a shared waker.

Lifecycle lock order: acquire the async worker lifecycle lock without the short
coordinator lock; perform awaited work outside the short lock; briefly acquire the
coordinator lock to validate the stop token and synchronously publish final lifecycle
state plus restart obligation. Release locks before notifying/starting. Startup
responsibility must survive cancellation of the initiating request. Existing
`last_resume_request` timestamps are not this atomic handoff: `start_if_needed_internal`
updates before the lifecycle lock, while `TryStop` checks before eventual stop
publication. This is a source-identified interleaving to test, not an executed bug
reproduction. Do not wrap the whole asynchronous `stop_internal` in the short lock.

### Cooperative unwind is an explicit unresolved gate

Calling a permit "cleanup-only" does not make the runtime enforce it. The inspected
code does not yet prove no normal guest continuation after commitment:

- `worker/invocation.rs:543-617` correctly forbids dropping unfinished durable
  drivers, but its settlement wrapper is not a cleanup-only driver.
- `durable_host/suspendable_wait.rs:135-166` can choose readiness while interruption
  is also ready; the new voluntary-stop protocol needs explicit precedence.
- The epoch callback in `worker/instance.rs:639-669` is not an immediate dispatch
  fence before the next guest instruction.
- `worker/entity_invocation.rs:161-185` races an invocation with cancellation and
  drops the losing future; this path needs examination against the no-drop contract.
- `worker/mod.rs:3869-3889` interrupt handling also drains entity bodies. A participant
  required by that drain must not synchronously initiate and await its own shutdown.

The next runtime contract must show how a latched stop prevents guest dispatch and
new normal host effects while existing durable handles unwind, and whether the
first host trap can end driving before siblings finish cleanup. No dedicated public
host-cleanup-only facility was found in the inspected pinned runtime. This does not
establish that a large fork change is required. It does prevent treating the current
interrupt flag or epoch mechanism alone as a proven fix.

Coverage attachment points: primary instantiation/pre-call/finalization and all
streaming result paths; normal Store driving in `run_guest_call_settled`; entity/native
registration before `start_entity_invocation_inner` spawns at `entity_invocation.rs:695`;
retained resources through parent-end settlement at `:329-387`; runtime task and
transfer creation; detached persistence/cleanup. Existing `ActivityGate` provides
an admission-lock precedent, not this suspension proof.

Before Stage 3, execute a deterministic transition harness for the race table and
a separate actual-runtime cooperative-stop probe. Model success cannot demonstrate
runtime coverage or sibling cleanup. These checks and the owned-source mixed-work
probe are outstanding; no production implementation or new runtime tests were run
as part of this source/oracle audit.

The two bounded experiments have now been dispatched to the existing dispatcher
and runtime child orbs linked above, respectively. Both received this document by
file transfer and must preserve the exact baseline. They own separate experimental
checkouts, not parent production changes. The parent retains ownership of the plan
and integration. Results will be incorporated when reported; dispatch is not passing
evidence. The deterministic coordinator transition harness is still pending.

### Executed stop/unwind probe: bounded success, no universal cleanup guarantee

The runtime child completed the follow-up on the exact Golem/fork revisions above.
Its archive is `tmp/gol706-stop-probe.tar.gz` (41KB), recoverable from the runtime
child with `download_thread_file`. It includes `src/bin/stop.rs`, `stop-fence.patch`,
`check-stop.py`, Cargo manifests/lock, baseline/patched/repeat logs and the input
design. The original runtime-probe archive remains preserved separately. The child
fork currently contains the stop patch, not a combination with the observation patch.

This uses the actual concurrent Wasmtime fixture but four modeled cleanup
obligations: two guest imports, one AccessorTask and one host-to-host stream producer.
Guards detect unfinished drops; they are not real DurableCallSessions or oplog tests.

| Case | Observed result |
| --- | --- |
| Readiness chosen before simultaneously latched stop | Four normal host completions and two guest observations |
| First host trap, ordinary driver | Driver returns with only 1/4 cleaned; later Store teardown drops three dirty obligations |
| First host trap, `run_concurrent_and_settle` | Same incomplete sibling cleanup |
| Entity-like cancellation drops enclosing future | All four dropped without cleanup |
| Stop-first host bodies, all-clean barrier before error | 4/4 cleaned; no normal effects, guest observations or dirty drops |
| Retained enclosing entity-like future, cooperative stop | 4/4 cleaned |
| Retain Store after first error, redrive three times | Remaining siblings clean; no general normal-dispatch fence established |
| Experimental fence plus first-error retention | 4/4 cleaned for first-error and cleanup-then-park variants; no normal effects/observations/dirty drops |
| Selected guest dispatch batch at adversarial stop boundary | 128 observations without fence, zero with fence; this boundary is not claimed to satisfy suspension eligibility |
| Store-retaining async fiber with hook installed | Two normal completions and one new host entry after stop |

The experimental fork patch adds 51 lines in `concurrent.rs`. It checks a monotonic
caller control, stops root and queued guest/fiber dispatch (including selected
`Dispose.ready` work), continues polling the existing host-future queue, and retains
the first error until caller cleanup acknowledgment. It adds no second scheduler.
It does not establish universal cleanup-only execution: the Store-retaining fiber
case uses `TypedFunc::call_async` with the hook installed and bypasses the event-loop
checks on resumption. It is not a reproduction of an owner committing with a valid
inner-blocked witness. Such exclusive execution must be excluded from eligibility
or adapted before this boundary is sufficient.

A further source finding, not an executed test: guest-facing stream cleanup may
need guest realloc through WorkerFunction. Blanket guest dispatch freezing could
prevent that cleanup. Only host-to-host stream cleanup was exercised. Likewise,
the epoch callback was source-reviewed, not independently probed as an interrupt
fence. Successful cases emitted 16 source-labelled cleanup wakes and one modeled
logical activation; a raw waker cannot derive that distinction.

Commands in the child (`tmp/gol706-probe`), before/after applying the patch:

```sh
cargo run --locked --bin stop > stop-baseline.log 2>&1
cargo run --locked --bin stop --features stop-hook > stop-hook.log 2>&1
python3 check-stop.py
```

The child reports both variants and ten repeated runs of each passing, plus rustfmt
and fork whitespace checks. The parent inspected the patch, relevant probe source,
checker and logs and executed the checker successfully:
`PASS: baseline failures, existing-API cooperative cleanup, fence/drain subset, Store-retaining-fiber counterexample`.
The parent did not rebuild the Rust probe.

Design consequence: retaining drivers, explicit stop precedence and acknowledged
cleanup of the complete cohort are necessary obligations. An executor barrier may
avoid a fork stop hook for a fully covered quiescent subset, but that subset is not
yet demonstrated for production. Do not silently reduce supported suspension cases
to choose it. Do not select the 51-line hook as the final fix either. Real durable
sessions, root-owned cleanup, dynamic cleanup children, all guest-facing streams,
cross-Store cleanup and error/retirement arbitration remain open. The combined
adjudication below narrows the next validation; the deterministic coordinator
model remains a separate required check.

### Whole-source result and combined oracle adjudication

The dispatcher child completed the fifth experiment on the exact pinned baseline.
Archive: `tmp/gol706-owned-source-experiment.tar.gz` (65KB), SHA256
`9dd3af57d5fbd9cf5d99a3e236745a7b212ff9a1d28668b67f7100993a6abc6a`.
It contains `OWNED-SOURCE.md`, a 608-line host, 64-line SDK guest, manifests/locks,
full fork patch and logs. The cumulative patch is 210 added lines across three
files; only a 13-line linker helper is new beyond the dispatcher experiment.
No built-in WASI source or production Golem implementation was changed.

The actual SDK-generated async initialize/invoke exports ran with shared engine
configuration, real P3 HTTP and built-in AsyncReadStream. The raw Wasmtime host
stubs fixed-identity `parse-agent-id`; it is not the executor/durable host and uses
`Func::call_async`, not production `run_guest_call_settled`.

The guest joins request and input futures. Input synchronously polls a one-hour
timer plus duplicated input; the peer releases input only after the request branch
observes its response and sends an acknowledgment. The watchdog never releases it.

| Mode | HTTP host completes | Terminal delivered / ack / input released | Invocation |
| --- | --- | --- | --- |
| HTTP-first control | Yes | All yes | Exact `observed-p3:exact-input` |
| Borrowed causal case | No | All no | Expected watchdog failure |
| Whole-source-owned causal case | Yes | All no | Expected watchdog failure |

The test binary succeeds because it asserts those failures, not because either
causal invocation succeeds. Control checks exact duplicate indices `[1,2]` and
reads `[23]`, `[169]`, then empty. Separate assertions pass for ordinary observer
cancellation, successive/concurrent observers, state across generations, child/parent
deletion and slot reuse. Crucially, a permitted empty nonblocking read traps while
unchanged `ready().await` holds the whole-source mutex. Reject that adapter.
Subscription keeps `supports_suspend=None`; no unload or eligibility was tested.

Commands executed twice in the child after the locked guest build:

```sh
cargo build --locked --manifest-path probes/gol706-owned-guest/Cargo.toml --target wasm32-wasip2 --release
cargo run --locked --manifest-path probes/gol706-p2/Cargo.toml --bin owned-source
```

Scoped rustfmt and diff checks passed there. The parent verified the archive hash,
read the report, guest/adapter source, assertion sites and repeated-run results,
but did not rebuild or rerun the Rust probe.

Parent inspection and follow-up oracle review corrected the earlier Gate A
requirement: synchronous P2 polling blocks the current Rust future's poll, so its
`join` cannot reentrantly poll the request sibling. Host completion is not guest
observation. This is not evidence of failure for independently scheduled component
tasks, nor a reason to require a general WASI rewrite. The existing HTTP/P2-sleep
test also uses join but requires eventual output, one request and elapsed time,
not acknowledgment before the synchronous sleep returns.

Selected candidate remains owned timer dispatch plus retained borrowed readiness
for unknown/mixed sets, which deny voluntary suspension. Validate actual supported
independent-work guarantees; do not promise reentrant guest polls. The source-specific
short-lock `AsyncReadStream::poll_ready` idea is unimplemented and unnecessary unless
a required case establishes the need. Blocking stream calls, real file replay,
override identity and cancellation forwarding remain uncovered by this adapter.

For stopping, prefer testing an executor acknowledgment barrier for the eligible
quiescent cohort before requiring the runtime stop hook. Acknowledge only after
actual durable handles and any transferred/detached cleanup reach a recoverable,
drop-safe state; do not confuse acknowledgment with body return. Preserve incomplete
Start on lifecycle abandonment, finish already-started terminal appends, and do not
invent Cancelled/EOF/delivery outcomes. `durable_host/concurrent/call.rs:3145-3159`
and `:3923-3975` own these existing contracts. The barrier belongs after the relevant
wrapper cleanup, not merely inside the low-level parked wait.

Choose a minimal runtime hook only if a required eligible participant can emit an
early error, resume normal guest work, or otherwise cannot be covered cleanly by
the executor barrier. Ineligible fiber and selected-batch counterexamples alone
do not require a universal cleanup-only driver. Guest-facing stream realloc is a
targeted question: payload lowering may require WorkerFunction, while ordinary
close/drop need not. Test the actual eligible stop path rather than all possible
stream states.

Next required evidence is real durable-session/root/entity cleanup with a relevant
guest-facing transfer, gated terminal persistence, coincident stop/readiness, and
unload/reconstruction; plus the deterministic coordinator transition tests. Neither
has run. Both architectural gates remain open; no child experiment remains running.

## Historical contract and design drift

### P2

The old `wasi:io/poll::poll` inspected the complete supplied poll set. Any pollable
without suspension support blocked timer suspension. Otherwise it used the minimum
advertised deadline and compared remaining time with the suspension threshold.

The actual fork implementation in `crates/wasi-io/src/impls.rs` uses
`std::cmp::min(max, maximum_suspend_time)` and then
`duration >= self.io_ctx.suspend_threshold`. The migration prose's description of
the "longest sleep" is imprecise: it was the longest interval during which none
of the polled timers could require attention, hence the earliest deadline.

Golem separately suspended when all polled resources were unresolved Golem
promises. Their completion provided the wakeup mechanism.

### Planned P3 replacement

The original plan explicitly called for:

> per-future suspendability metadata + scheduler-level quiescence detection

It proposed an observation at the runtime's no-ready-work point, with
`pending_unsuspendable == 0`, default-deny classification, deadline/wakeup-key
metadata, and accounting for post-return/background work. It preferred a small
fork hook over a fragile executor-side inference layer. Suspendable waits were to
remain real futures, completing locally while the Store stayed resident.

The plan also proposed dedicated suspension/wake-winner oplog records. Those exact
record shapes are not a requirement of this design: today's durable-call and
completion-delivery machinery already owns observable ordering. Add no redundant
oplog protocol without demonstrated need.

Unblocked and the inspected migration thread supplied no explicit justification
for replacing runtime-level evidence with the current host-call counters. This is
absence of a found decision, not proof that no discussion ever happened.

## Current implementation and reported failures

Important source locations, at the baseline:

- `golem-worker-executor/src/durable_host/suspendable_wait.rs`: per-wait parking,
  local threshold checks, repeated readiness/yield checks, wakeup scheduling.
- `durable_host/mod.rs`: Store-local wait registry/live-call counters,
  `safe_to_suspend`, `suspend_admissible`, and `WakeupScheduler::sleep_until`.
- `durable_host/p3/clocks.rs`, `io/poll.rs`, `golem/v1x.rs`: timer/P2/promise callers.
- `durable_host/wasm_rpc/mod.rs`: RPC registrations and `wait_for_rpc_suspend`.
- `durable_host/durable_session/mod.rs`: passive stream/replay-tail waits.
- `worker/instance.rs`: shared `OwnerExecution`; entity Stores share execution
  identity/resources but currently have their own suspension accounting.
- `worker/invocation.rs`: `run_guest_call_settled` is post-return tail settlement.
- `services/scheduler.rs`: a batch waits for all claimed per-agent action groups
  before another batch/tick is processed.

`park_registered_wait` tests only its own remaining duration against
`suspend_after`. `safe_to_suspend` checks settled completion markers, equality of
live durable host-call and registered-wait counts, no open durable scope, and no
pending P3 HTTP transmission. Later, wakeup scheduling uses the global earliest
registered deadline. Every long wait can independently prepare a wakeup.

The default initial grace is one second; the ten-second threshold concerns
remaining time, not elapsed waiting time. Thus the long timer can suspend a
two-second race after one second. This violates the intended policy independently
of whether replay happens to succeed.

GOL-706 reports replay hangs/divergence and SQLite wakeup starvation. The parent
did not reproduce the full server failure. Recorded completions left unconsumed
at invocation/body boundaries indicate a recording/reconstruction problem, not
acceptable application nondeterminism. The exact replay defect remains open.

One stuck scheduler action can delay subsequent batches. Other actions already
running in the same batch can still complete. Default key-value retries allow
15 attempts; repeated 30-second pool timeouts can delay progress for minutes.
Scheduler isolation and bounded, cancellation-safe action processing remain a
separate robustness obligation, not something this eligibility design alone fixes.

The suspected permanent SQLite connection leak is not established. SQLx normally
returns or replaces connections on cancellation. The "Created promise" log occurs
before its write, so duplicate lines establish attempts, not completed writes.

## Executed evidence

### Parent isolated suspension and SQLx probes

An isolated harness compiled the actual suspension functions with substituted
types and a recording wakeup service; it was not a complete executor test.

| Timers | First result | Elapsed | Wakeup attempts |
| --- | --- | --- | --- |
| 2s | Ready | 2001ms | 0 |
| 2s, 60s | SuspendWorker | 1013ms | 1 |
| 2s, 60s, 600s | SuspendWorker | 1012ms | 2 |

Both attempts in the three-timer case targeted the two-second deadline. SQLx
0.8.6 with a single SQLite writer survived 2,000 write-cancellation attempts and
2,000 same-key write races dropping the loser; every follow-up write succeeded.
These tests refute neither the reported end-to-end stall nor other cancellation
sequences. The temporary parent harness was removed after reporting.

### Runtime probe: actual pinned Wasmtime

Preserved in the runtime child at `tmp/gol706-probe/`:
`src/main.rs`, `Cargo.toml`, `Cargo.lock`, `runtime-observer.patch`,
`check-observation.py`, `baseline.log`, and `observed.log`.
Downloadable archive: `tmp/gol706-probe.tar.gz`. These files are ignored: recover
with `download_thread_file`, not `download_thread_changes`. Its fork clone
`/tmp/gol706/wasmtime` contains the diagnostic patch. The parent downloaded and
inspected the source, patch, checker and logs, then removed its temporary copy.

Commands in that child:

```sh
cargo run --locked > observed.log 2>&1
python3 check-observation.py
```

The unmodified-fork baseline and diagnostic-patched run passed; ten repeated
binary/checker executions passed. Formatting/diff checks passed. The probe uses
the checked-in `concurrent-runtime-events.wasm` fixture and Golem's shared engine
configuration constructor.

Established counterexamples:

- Final root poll enqueues an `AccessorTask`, then outer `Pending` reports zero
  wakes. Manual repoll starts the queued task.
- `FuturesUnordered` fairness returns `Pending` after starting only 2/128
  background tasks, with two wakes.
- Guest invocation returns `Pending` after 128 imports completed but before any
  result was observed. Diagnostic output confirms selected runnable work held
  in `Dispose.ready`; the next poll delivers all 128.
- Traditional `func_wrap_async` can retain Store access across an await and return
  outer `Pending` without reaching the inner blocked observation boundary.
- A host-to-host stream transfer runs with zero host imports. A background
  `AccessorTask` has `has_guest_visible_subtask == false`, unlike an import.

The approximately 68-line diagnostic extension wraps the entire `poll_until`
driver's waker with a generation and blocked bit, invalidates on driver poll and
wake, and publishes with compare-and-swap only at inner no-work `Pending`
branches after rechecking queues, including after the final root poll. It rejected
the false positives, recognized genuinely parked active guest/background/stream
work, and invalidated the saved witness when an import woke before driver repoll.

This proves useful observation behavior for those cases, not suspension permission.
All work remained unclassified. No owner-wide commitment, activity-ID lifecycle,
teardown invalidation, or complete stream-direction coverage was implemented.
Rejecting the final-root false positive does not itself solve its zero-wake
liveness issue: production must continue driving newly discovered runnable work.

### P2 probe: owned readiness and synchronous imports

Preserved in the P2 child at `probes/gol706-p2/`:
`Cargo.toml`, `Cargo.lock`, `guest.wat`, `src/main.rs`, `README.md`.
No production or fork modifications were made.

```sh
cargo run --locked --manifest-path probes/gol706-p2/Cargo.toml
cargo fmt --manifest-path probes/gol706-p2/Cargo.toml -- --check
```

Six cases passed; ten repeated executions and formatting/diff checks passed.
The guest uses the real synchronous-lowered `wasi:io/poll@0.2.6` resource/list ABI.

- Pure one-hour timer: actual `run_concurrent` is pending, no self-wakes in 25ms,
  still pending on repoll. This alone is not a quiescence proof.
- Traditional `func_wrap_async` negative control: Store task progress before
  readiness is zero. `func_wrap_concurrent`: progress is one.
- Stateful duplex input retains first byte 23 across five cancelled observers,
  then receives byte 169; final bytes are exactly `[23, 169]`.
- `poll([long_timer, input, input])` writes exact ready indices `[1, 2]` into
  guest memory; repeated ready/block behavior passes.
- Modeled replay of recorded `ready == true` awaits real readiness, preserves
  the `ready -> bool` ABI, and permits Store task progress.

The successful fixture has an async enclosing component export with synchronous
lifting. Running with `-- --sync-export` fails with `CannotBlockSyncTask`, exit
101. This does not establish that Golem cannot use the approach: actual
`wit/deps/golem-agent/guest.wit` defines `invoke` as async, and invocation dispatch
uses concurrent execution. Real SDK-generated component coverage is still needed.

The tested implementation owns readiness from source creation, alongside actual
pollable resources. It is not a generic adapter for existing `DynPollable`:
`make_future` produces a future borrowing the source table entry. Exposing that
function cannot remove the borrow. Poll-once/drop/recreate is not a valid generic
solution because partial I/O and wake registrations can be lost.

Limits: controlled Store task rather than real HTTP; modeled replay rather than
files/oplog; one readiness generation and one active observer; no full resource
drop/reuse, multiple-observer, dynamic-override, or wake-fanout coverage. Real file
replay must materialize the buffer/state needed by subsequent reads, not merely
return recorded readiness.

### Narrow dispatcher probe: mechanism passes, lifecycle acceptance not reached

The third child verified the same Golem and fork pins. Its archive is
`tmp/gol706-dispatch-experiment.tar.gz`, recoverable with `download_thread_file`.
It includes `probes/gol706-p2/DISPATCH-README.md`, `src/dispatch.rs`, `dispatch.wat`,
the standalone manifests/lock, `check-dispatch.py`, and patches/logs under
`tmp/gol706/`. The parent inspected the README, dispatcher patch, checker and
positive/negative logs; it did not independently rebuild the experiment.

The local fork prototype adds 91 lines in `component/func/host.rs` and
`component/linker.rs`. `func_wrap_dispatch` selects the existing borrowed
`store.block_on`/`HostResult::Done` or concurrent
`store.wrap_call`/`HostResult::Future` path before creating the borrowed future.
It contains neither WASI-source changes nor suspension policy. This is an
experimental API, not a production-reviewed interface.

The diagnostic observer/progress patch adds 106 lines in `concurrent.rs`, including
the earlier observer and formatting. Seven new semantic lines wake the driver
when final-root polling queued work. This addresses the earlier zero-wake case
within the probe; teardown invalidation and a production witness API remain absent.

Executed commands:

```sh
cargo run --locked --manifest-path probes/gol706-p2/Cargo.toml --bin dispatch \
  > tmp/gol706/observed-dispatch.log 2>&1
python3 probes/gol706-p2/check-dispatch.py tmp/gol706/observed-dispatch.log
cargo fmt --manifest-path probes/gol706-p2/Cargo.toml -- --check
```

The run/checker, ten repeated binary/checker runs, and formatting/diff checks passed.
Evidence:

- Timer block/list chose owned concurrent readiness, permitted Store task progress,
  and reached the inner blocked observation. Timer-list duplicates returned `[1,2]`.
- Unknown/mixed sets used the actual unchanged WASI borrowed `Host::poll`, retaining
  first byte 23 in its future until byte 169 arrived; mixed duplicates returned
  `[1,2]`. No blocked witness was published while these operations were pending.
- Crucially, borrowed-case Store task progress was zero until external readiness.
  This fallback does not guarantee progress if readiness needs another task in the
  same Store. It is an exclusion mechanism, not a mixed-concurrency solution.
- Same-instance timer follow-ups passed. They did not reconstruct an instance.
- A one-hour timer reached blocked observation, then the raw probe timed out and
  dropped its driver. This is not Golem suspension, unloading, or durable cancellation.
- Final-root queued work completed under Tokio without manual repoll. Removing the
  added wake failed with exit 101: `timeout wake must not rescue missing driver wake`.
- The synchronous-export negative control failed with exit 1, `CannotBlockSyncTask`.
  Successful exports remained async WAT exports with synchronous P2 imports.

There was no actual SDK invocation, `Suspended` status/unload, persisted deadline
activation, oplog replay, real P3 HTTP, or durable cancellation/delivery evidence.
The child stopped because the baseline lacks owner-wide election, participant
accounting, admission fencing and wake handoff. Reusing the old local suspension
decision would not validate the proposed authority, and inventing the coordinator
would exceed this experiment's scope. Resource reuse/overrides, sibling Stores,
all stream directions and cancellation races also remain untested.

Verdict: GO for feasibility of the narrow execution dispatcher; NO-GO as proof of
the suspension fix. The acceptance criteria below remain integration gates, not
completed results. The fallback's Store-progress limitation is a design constraint
that owner coordination alone cannot remove.

## Proposed centralized design

### Ownership and accounting

One coordinator belongs to `OwnerExecution`, covering all primary/entity Stores,
native bodies, stream work, admission and finalization for that owner. It is a
registry and lifecycle state machine, not a scheduler. Wasmtime and Tokio retain
execution scheduling.

Registrations precede admission/spawn and last through cleanup. Use lifetime-safe,
generation-qualified identities; resource-table indices can be recycled. Dropping
a caller-facing handle does not remove a detached body that still executes.

Every activity is unknown/blocking by default. Runtime-owned work needs opaque
identities or a demonstrated complete executor interception boundary. Imports alone
are insufficient; spawned tasks and transfers lack the same public identity.
Root-driver host phases and work outside Wasmtime require executor accounting too.
Never classify an entire composite future as a timer merely because one branch sleeps.

### Runtime observation

Extend observation, not post-return settlement semantics. A blocked witness must
mean no runnable guest work is queued or selected-but-unexecuted, immediately
runnable host work has been driven, and execution is not parked inside a
Store-retaining fiber. Root invocation completion is not required.

Keep Golem deadlines, promise IDs, rechecks, persistence and suspension policy out
of Wasmtime. The fork surface should be observation and necessary opaque activity
identity plumbing in existing runtime structures, not a parallel scheduler.

A witness is generation-stamped. Every driver poll revokes it before executing
work. Application wakes, admission, and classification changes invalidate it.
Publish only against the generation captured before the observation pass, not a
fresh generation that could overwrite evidence of a racing wake. No semantic work
may occur after witness publication in that poll.

Policy/grace timers notify only the coordinator. They do not pretend to be
application wakes or force Store repolls. Never suppress invalidation by guessing
why a shared application waker fired.

### Wait classes

| Class | Eligibility |
| --- | --- |
| Running, unknown, ordinary HTTP/DB, persistence, cleanup | Deny |
| Timer | Global earliest actual deadline must satisfy `suspend_after` |
| Durable external wake | Completion survives unload and reliably activates owner |
| Durable state with scheduled recheck | Eligibility grace elapsed; recovery valid; recheck persisted |
| Certified internal await of separately accounted owner work | Neutral; never independently authorizes suspension |

RPC recheck time is not a prediction of earliest completion. Keep `eligible_after`
separate from `resume_after`. A caller may need to unload before the target accepts
or starts execution, to release its account concurrency/memory permit. Its durable
intent and stable-key redispatch contract justify recovery, not presumed callee
acceptance. Proposed policy: each RPC's grace must elapse even if another wait
triggers evaluation; this is an explicit behavior choice to validate against tests.

`Deadline(None)` is not enough metadata. A Golem promise has durable completion
and activation. A stream source may only have resident notification/long polling;
require persisted rechecks until a durable consumer activation contract is proved.
Source opening, attachment and journal writes remain blocking.

Tail-settlement-safe parking is not automatically unload-safe. Replay cursor
transactions, delivery and reconstruction work are not external wake sources.
Certified parent/child waits can be neutral without duplicating the child, but
require at least one genuine durable wake source for the owner. A flat registry
does not detect an A/B dependency cycle disconnected from an unrelated timer C.
Do not claim general deadlock detection; ambiguous waits remain unknown/blocking.

### Prepare, persist, revalidate, commit

1. Observe valid blocked witnesses for every participant and classify all progress.
2. Elect one owner-wide suspension attempt with participant/wait/wake generations.
3. Persist required recovery intent and timer/recheck actions outside the short
   coordination lock. Ordinary work continues. Retain ownership of the persistence
   future; do not casually cancel it midway through storage work.
4. Revalidate witnesses, membership, readiness, lifecycle and classifications.
   Recompute remaining time even if the registry is unchanged. A newly short timer
   aborts suspension; merely scheduling an earlier wake is insufficient.
5. Atomically commit against admission and existing drivers' next normal poll.
   No await lies between final validation and commitment.
6. Cooperatively interrupt/unwind all participants, then unload. Never implement
   suspension by selecting an eligibility future and dropping the runtime driver.

Use stable attempt identity for schedule persistence. There is one preparation
owner per attempt, not one per wait. Repeated suspension cycles may have separate
actions. Obsolete persisted actions may cause harmless extra wakes; cancellation
must not be a correctness prerequisite. Promise-only waits with reliable durable
activation need no periodic fallback timer.

External wake before commitment invalidates the attempt; after commitment it must
latch a resume obligation through stopping/unload. Use a lifecycle-locked handoff
and monotonic wake sequence, rather than timestamp comparison as the proof.
An interrupt flag alone does not fence another driver's normal poll. Teardown must
be marked before dropping sibling work so lifecycle loss is not recorded as guest
cancellation, EOF or an invented terminal. Never wait for owner shutdown from a
driver needed to complete that shutdown.

### Replay and lifecycle boundaries

Retain Start/End/Cancelled and CompletionDelivered/CompletionDiscarded as the
authority for replayed observations. Retain existing scheduler persistence and
lifecycle hints. No new suspension-specific oplog representation is currently
justified. Cursor exhaustion is not live execution admission. Incomplete child
reconstruction can contain authorized live continuation, so a blanket "any replay
means no suspension" rule is also not sufficient.

Keep `run_guest_call_settled` for tail settlement; streaming paths must separately
participate in owner-wide suspension accounting. Guest return does not imply that
stream output or background work has finished.

### Removal and retention

Replace per-wait grace/check loops, local suspension decisions, readiness/yield
heuristics, Store-local eligibility registries, independent RPC suspension selectors,
and legacy direct-suspend shortcuts. Do not leave them as alternative authorities.

Retain live-call permits for durable-call/snapshot ownership, completion-marker
receipts, tail settlement, ephemeral restrictions, RPC idempotency and permit
release, lifecycle teardown and durable wake persistence. Existing scope/HTTP
blockers remain until their obligations have explicit accounting replacements.

## Narrow P2 route: experiment outcome and remaining integration gate

A wholesale owned-readiness rewrite of files, sockets, DNS, stdio and streams is
not yet justified. The raw probe demonstrated one P2 poll binding with two execution modes:

| Supplied poll set | Mechanism | Suspension authority |
| --- | --- | --- |
| Nonempty, entirely recognized owned timers | Concurrent, Store-releasing readiness | Owner coordinator only |
| Any unknown/table-borrowing source, including mixed sets | Existing exclusive borrowed readiness implementation | Blocks voluntary suspension while pending |

This describes a candidate, not a proven production contract. Gate A retains the
borrowed fallback as suspension-ineligible, subject to actual supported scheduling
guarantees; it does not require reentrancy within a synchronously blocked Rust poll.
Current P2 promise-backed fast-path registration has no insertion sites; do not
revive it for this experiment.

Public APIs do not currently establish per-call selection of these execution modes.
`Accessor::with` cannot await the borrowed fallback. The third probe demonstrated a
narrow fork binding dispatcher before Store borrowing: traditional
`store.block_on`/`HostResult::Done` versus owned `HostResult::Future`/`poll_and_block`.
The fork selects execution mechanics, not timer/suspension policy. The borrowed
branch retains Store access and can block dependencies within that Store. A
production design must address required mixed-work progress, not merely classify
that branch as ineligible for suspension.

Still-unmet end-to-end acceptance criteria:

1. Run an actual SDK-generated Golem invocation containing a long P2 sleep. Prove
   genuine runtime blocking, actual `Suspended` status/unload, deadline wakeup and
   correct replay/result/follow-up. A synthetic fixture alone is not this result.
2. Through the same binding, exercise an unknown or mixed poll set with stateful
   readiness. Preserve retained-future behavior and remain suspension-ineligible.
3. Pending P3 HTTP and queued/runnable guest/background work must prevent
   suspension. Preserve progress according to the supported scheduling contract,
   not reentrant polling of synchronous guest code. Do not infer Store blocked
   merely because one recognized timer is parked.
4. Validate actual export/blocking contracts, cancellation and durable delivery
   boundaries. Preserve duplicates, absolute deadline, and resource lifetime.

Use local prototype changes, deterministic gates where practical, and exact pinned
dependencies. Record partial results honestly if actual server integration is
blocked. Do not claim source review or modeled readiness is end-to-end proof.

If the dispatcher cannot stay small and policy-free, stop and report the tradeoff:
resident legacy P2 waits (reduced suspension coverage) versus broader owned-source
conversion. Do not silently choose either. A known P2 timer must not become a
special exemption from runtime quiescence proof.

## Validation matrix for the production change

- Runtime: selected batches, cooperative/fuel yields, final-root queue creation,
  fairness self-wakes, guest delivery queued after host completion.
- Coverage: unknown import, AccessorTask, root-host phase, native/entity admission
  before first poll, all stream transfer directions, identity reuse and teardown.
- Timers: `[2s,60s]`, `[2s,60s,600s]`, all-long sets, earliest timer completing or
  being cancelled, new/shortened waits during persistence, eligibility grace expiry.
- Wake races: before prepare, during persistence, before/after commitment, during
  stopping and after unload. Either prevent suspension or preserve eventual resume.
- P2: long-sleep unloading, mixed sources, real file-readiness replay, sync ABI and
  actual enclosing export compatibility, observer cancellation and resource reuse.
- RPC: one-concurrency-slot progress, delayed target admission, stable retry key,
  exactly-once logical execution, HTTP blocker completing after RPC grace elapsed.
- Owner: runnable sibling tool/native body prevents suspension; detached operations
  remain counted; stream output continues after guest result; no-source waits.
- Recovery: crash at Start/End/delivery/suspend boundaries, repeated suspension,
  promise and stream restart; no false Cancelled/EOF or unfinished-handle panic.
- GOL-706: polling-loop race for agent and tool entrypoints, correct result and
  follow-up, consumed recorded completions; independent SQLite/scheduler diagnosis.

Existing behavioral acceptance tests include `p3_sleep_suspends_and_resumes`,
`interrupt_while_parked_in_p3_sleep`, `p3_promise_suspend_survives_executor_restart`,
`p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold`, raw sync/async
RPC suspension/restart cases, `concurrent_agent_limit_allows_rpc_progress`, and
`rpc_suspension_retries_after_concurrent_http_wait_finishes`, plus tool/entity and
durable input/output stream reconstruction. Quota/fuel suspension must not regress.

The existing `sleep_longer_than_suspend_threshold` only checks elapsed time, health
and oplog queryability. Add an explicit suspension/unload assertion: passing it
while retaining the agent would not prove preserved behavior. Helper-level tests
may be rewritten when old APIs are removed, without weakening their behavioral
assertions (ready races, safety invalidation, scheduling errors, earlier wakeups,
registration lifetime and wake-during-scheduling).

## Remaining decisions and proof obligations

- Minimum complete runtime activity-identity API; observation alone is insufficient.
- Owner-wide witness invalidation and atomic commit/activation/teardown handoff.
- Production P2 dispatcher integration and actual Golem component coverage;
  raw dispatcher feasibility is established, but borrowed-fallback Store liveness
  is a demonstrated limitation.
- Durable activation versus bounded recheck contracts for stream inputs and RPC.
- Per-wait grace policy when multiple kinds of waits coexist.
- Invocation and tail-work deadline behavior across unload; do not silently reset
  budgets or treat every deadline as an ordinary replayable sleep.
- Preserve liveness when detecting queued work with no emitted wake; the diagnostic
  wake correction passes the raw positive/negative probe but needs production review.
- Full replay defect and SQLite stall root causes remain separate unresolved work.

Oracle consultations support this architecture conditionally. They do not replace
executed tests. None of the three children implemented complete activity coverage,
a production coordinator, or the lifecycle protocol. No statement here means all suspension
cases are solved or existing integration suites have passed.
