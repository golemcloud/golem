# GOL-710: worker status reconstruction and recovery boundaries

Investigation record, 2026-10-02. **Superseded as an implementation handoff by the
[final oracle-approved plan](gol-710-implementation-plan.md).**

This document records the investigation, alternatives, revised direction, and
open questions. It does not claim that the full reported crash was reproduced or
that the proposed changes have been implemented.

**Earlier finalization review:** the oracle returned **NOT READY**. The real accessor /
Worker commit / second-reconstruction experiment in section 5.5 remains a design
gate: it determines whether a replay-transition change is needed and how it must
handle already-issued calls. The replay-state diagnostic alone does not settle
that question. No further product decision is currently required. The projection
horizon, baseline scope, and failure-test requirements below incorporate the
other review findings. This describes the earlier investigation state, not the
current handoff status.

**Resolution:** the [child experiment](https://ampcode.com/threads/T-01a0fbc3-a952-7195-bfaa-3509e483aa14)
completed the real P3/Worker/second-reconstruction schedules. Cleanup-first and
ordinary retained-call adoption passed; claim-before-cleanup and already-issued
calls wrote surviving completions under skipped Start 14 and timed out on the
second reconstruction. The fixture uses logical revert and executor recreation,
not the original process crash. The final plan selects shared destructive-recovery
ownership and existing internal Jump/Immediate reconstruction for invalidated
issued calls, separately from actor-owned status repair. The oracle approved that
plan with no remaining pre-implementation blockers. Open alternatives and future
investigation wording below are retained as historical context, not instructions
to repeat completed work.

## 1. Goal and references

Prevent an invalidated worker-status projection from aborting the executor or
being used to make execution/admission decisions. Give projection maintenance a
reliable owner, preserve early invocation responses, and make ordinary Jump
reconstruction inexpensive.

- [GOL-710](https://linear.app/golem-cloud/issue/GOL-710): assigned to Daniel
  Vigovszky and In Progress during the investigation.
- [Design and investigation conversation](https://ampcode.com/threads/T-01a0f7d1-e59c-754c-9fa6-b9881c470835).
- Inspected revision:
  [`accae0e43`](https://github.com/golemcloud/golem/commit/accae0e435b5097cd1d7940a5c1f568bde8a055a),
  the revision cited by the report. Source locations below refer to that checkout;
  resolve symbols again before implementation.
- Related reports: [GOL-706](https://linear.app/golem-cloud/issue/GOL-706) and
  [GOL-709](https://linear.app/golem-cloud/issue/GOL-709). Their timer/replay issues
  are not assumed to be fixed by this work.

## 2. Reported behavior and evidence limits

An agent races a P3 HTTP POST against a 600-second durable wait. A small HTTP
server holds the POST; the agent is crashed after the request arrives, then the
server releases requests. Recovery sometimes panics with:

```text
worker status was unexpectedly detached from the oplog
```

The report locates the panic in `worker/state_actor.rs:719`, reached from the
second status read in `DurableWorkerCtx::on_invocation_failure`. Development and
release profiles use `panic = "abort"`, so the failure kills the server process.

The reporter observed 4 aborts in 9 POST crash cycles, 5 successful GET cycles,
and 10 successful tool-call cycles. These are observations, not proof that GET
or tool calls cannot encounter related defects. Earlier successful runs do not
establish when the defect was introduced.

POST uses `WriteRemoteBatched(None)`; GET uses `WriteRemote`. Incomplete POST
recovery can append a Jump covering an oplog suffix that also contains another
task's clock operations. The reported completion-delivery error is a plausible
trigger, but the exact first error and schedule have not been established.

### Executed verification

Both commands passed one test, with zero failures:

```sh
cargo test -p golem-worker-executor --lib -- checkpoint_repair_folds_from_checkpoint_after_jump --report-time
cargo test -p golem-worker-executor --lib -- investigation_delivery_failure_prevents_jump_registration --report-time
```

The second was a temporary diagnostic test: inject a completion-delivery failure
into `ReplayState`, then verify `register_replay_jump` rejects it. It was removed
after execution. The first is an existing status reconstruction test.

These prove baseline invalidation/repair and poisoned-cursor rejection
independently. **They do not reproduce the complete POST/timer abort.** No
production changes resulted from that investigation. The subsequent historical
and oracle audit was read-only.

After the storage/API decisions, an additional temporary diagnostic passed:

```sh
cargo test -p golem-worker-executor --lib -- investigation_sibling_claim_during_pending_jump_cleanup --report-time
```

Final result: **1 passed, 0 failed, 2634 filtered out**. This characterizes the
existing behavior; it is not a passing test of the proposed fix. Section 5.5
records the fixture, both schedules, and its limitations. The diagnostic was
removed from source after execution rather than enshrining the suspected unsafe
behavior as a permanent contract. Local investigation evidence remains in
`tmp/gol-710-transition-diagnostic.patch` and
`tmp/gol-710-transition-test.log`; these are not tracked implementation artifacts.

## 3. Status validity is not runtime readiness

`AgentStatusRecord` is a derived projection of oplog history. Normally the actor
folds new committed entries onto a valid earlier status.

Distinguish:

- **Lagging:** valid for an earlier prefix; incremental folding can catch up.
- **Detached:** newly skipped/deleted history invalidates the old summary as a
  fold baseline, or folding failed. Keeping the old record does not make it
  authoritative.
- **Runtime ready:** the resident Store, replay cursor, component revision,
  authority state, and lifecycle permit execution. Status attachment alone
  proves none of these.

For example, a summary through entry 50 may already include contributions from
entries 30–40. A Jump newly skipping 30–40 can invalidate that summary even
though entry 50 itself is outside the skipped region. Rebuild from a baseline
whose incorporated history remains valid.

Reconstruction follows field-specific rules. Invocation results, retry history,
updates, and sticky permission-transfer receipts do not all treat Jump/revert
regions identically. Reuse the existing fold rules rather than inventing a
generic subtraction operation.

## 4. Why the expectation existed: historical findings

Unblocked, Git history, historical Amp threads, and an oracle review were used.

| Change | Evidence and rationale |
| --- | --- |
| [PR #1291](https://github.com/golemcloud/golem/pull/1291) | Explicitly states status is reconstructible purely from oplog; caching and incremental folding are optimizations. Distinguishes skipped from deleted regions. |
| [PR #2173](https://github.com/golemcloud/golem/pull/2173) | Introduced detachment and asserted status reads. The invocation loop was expected to know when reattachment was required; explicit boundaries included instance creation and revert. No substantive PR discussion establishing a stronger generation contract was found. |
| [PR #3638](https://github.com/golemcloud/golem/pull/3638) | Introduced the status/lifecycle actor queues to avoid fair-lock ownership being handed to an unpollable Store task. Preserved the asserted read. |
| [PR #3712](https://github.com/golemcloud/golem/pull/3712) | Introduced attached reads and cancellation-safe append/commit/reattach transactions. The permissions thread explains why transfer deduplication must repair a detached index and preserve acknowledged receipt identity across Jump/revert. |
| [PR #3787](https://github.com/golemcloud/golem/pull/3787) | Added an attached getter for independent host tasks overlapping Jump/replay completion. |
| [PR #3887](https://github.com/golemcloud/golem/pull/3887) | Handles incomplete commit receipts caused by threshold/replica commits outside the status actor; catches up before publication. |
| [PR #3935](https://github.com/golemcloud/golem/pull/3935) | Separates completion acknowledgement from folding. Later authoritative reads/admission remain FIFO-ordered. Ordinary metadata/result readers were moved onto attached actor reads. |

The original comment says only the invocation loop can reason about detachment.
Its intended discipline was: start attached, keep ordinary folds attached, and
reattach or leave the execution on exceptional transitions before making further
decisions. Concurrent host work makes that caller-owned assumption fragile.

FIFO proves preceding jobs finished, **not** that they produced valid status.
A preceding fold can finish by setting the detached flag.

No demonstrated contract requires preserving detached status as the valid view
of an old resident generation. Historical permissions work deliberately repairs
it in place. Nevertheless, publishing a correct durable projection does not
authorize continuing a failed resident execution.

## 5. Confirmed gaps and additional concerns

### 5.1 Repair can be abandoned by its producer

`durable_host::commit_replay_jumps` currently performs:

```text
commit Jump(s) → fallible replay registration → explicit reattachment
```

Registration failure bypasses reattachment. Caller cancellation or cessation of
Store polling can also prevent later steps, while already accepted actor jobs
continue. With multiple Jumps, a later failure can follow a committed prefix.

A source-supported schedule explains the reported second status read:

1. POST recovery submits a Jump but has not completed status maintenance.
2. Another operation fails and failure handling begins.
3. The first status read sees the previous attached projection.
4. Committing the failure also picks up the pending Jump, invalidating status.
5. The second asserted read panics.

This is a reachable source-level schedule, not the established incident trace.
If the Jump already detached status before the first read, that read would fail
instead. Actor FIFO must be respected when constructing the reproducer.

### 5.2 Existing reconstructing reads are not pure reads

`StatusJob::AttachedStatus` calls `reattach()` when detached. `reattach()` first
commits with `CommitLevel::Always`, then reconstructs. Substituting this getter
can therefore commit unrelated buffered work and encounter a shard fence.

`try_attached_status()` also performs unconditional reattachment followed by a
separate read. It is not the desired one-job, attached-fast-path operation.

Separate non-committing projection access from explicit commit barriers. Preserve
durable versus ephemeral deferred-write semantics; do not reinterpret every
acknowledged ephemeral prefix as physically persisted storage.

### 5.3 Admission can ignore reconstruction failure

`StatusJob::AppendInvocationIfVersion` calls `ensure_status_attached()` and then
uses the retained record without checking whether repair succeeded. Replacing
the asserted getters alone would leave this authoritative-state hole intact.

### 5.4 Reattachment has publication and cache consequences

Publishing status updates metrics and the `RunningWorkers` recovery index, then
reconciles the persisted cache. It does not synchronize Store-local wallets or
prove replay readiness. Preserve authority invalidation and synchronization.

`Worker::reattach_worker_status()` clears hydrated invocation results; actor
reattachment does not. Keep runtime-boundary cache reset separate from projection
repair. Cached results include synthetic warm failures as well as durable data.

Normal incremental publication schedules archival; reconstruction currently
does not take the same archive-scheduling path. Review publication obligations
explicitly when centralizing repair.

There are other deliberately fatal policies, such as failure to maintain the
assignment-recovery index after its retry budget. This work must not accidentally
change them or claim to make every storage failure process-safe.

### 5.5 Primary replay may publish Live before Jump cleanup

Incomplete batched-write recovery calls `begin_switch_to_live`, commits/registers
Jumps, then calls `finish_switch_to_live`. For the primary runtime, the first
operation can already publish shared Live. `PendingReplayToLive::finish` does not
defer that primary publication; its additional work includes tool admission and
non-primary local-tail publication.

The ordering is confirmed by source. A deterministic replay-state diagnostic now
also demonstrates sibling claim admission during the cleanup window. It does not
reproduce the reported HTTP/timer crash or execute the full accessor/Worker path.

The fixture contains `NoOp(1)`, an incomplete batched scope `Start(2)`, and an
unowned clock `Start(3)`. Claiming and awaiting the incomplete scope scans to the
tail and retains the sibling. A spawned task calls the production
`ReplayState::switch_to_live(PrimaryAgent)`, signals a oneshot channel, then waits
on another channel before registering the intended Jump over 3..=4. While it is
paused, the test checks the production `store_is_live` predicate and calls
`claim_start_for_store` for the clock. The claim returns the old Start at 3.
After Jump registration, awaiting that already-issued handle still returns
`ResolutionOutcome::Incomplete`, while index 3 is confirmed skipped. In the
cleanup-first control, the same claim returns `ReplayEnded` rather than the old
handle. No sleeps determine this ordering. The test does not proceed into actual
host-call repair, append a terminal, or execute an external effect.

The first fixture attempt did not scan the scope to its incomplete resolution
and failed because the sibling was not retained. Correcting that setup was
necessary; that initial failure was not evidence about production admission.

Oracle source review found no card-boundary lock spanning the recovering
accessor's begin-transition and Jump cleanup. Retained Starts make
`durable_call_is_live` false, but this routes the sibling into ordinary replay
claiming rather than blocking it. Cursor transactions serialize individual claims
and cleanup; there is no pending-Jump obligation joining those transactions.

**Do not generalize this into “Live must have no retained Starts.”** Ordinary
Live/replay overlap is intentional: late owners must adopt retained Starts rather
than duplicate them. The concern is a claim or live repair crossing an unfinished
recovery operation that intends to delete its history.

The proposed transition invariant is: once recovery selects a Jump region for a
replay target/generation, affected claims and live repair cannot cross that
boundary until the Jump is committed and registered. Competing settlers must
respect the same obligation. Failure or cancellation cannot expose it as a
successful transition. Unaffected retained history must remain replayable.

If the full-path test confirms the hazard, give this obligation a shared owner in
the existing replay-transition machinery. Merely moving `publish_live` to the
caller-local `PendingReplayToLive::finish` is insufficient: claims and competing
settlers also need to respect it, including handles acquired before the boundary.
Do not hold the cursor or card-boundary mutex across storage/reconstruction waits,
put a Store/replay wait inside the status actor, or suppress divergence.

Before finalizing this part of the fix, use the real accessor and Worker commit
path with a gate after `begin_switch_to_live` and before Jump append/commit.
Exercise the P3 timer's `P3MonotonicClockNow` identity, observe claim and terminal
indices, let cleanup and sibling completion run, then restart again. A successful
continuation must not leave surviving terminals/delivery markers whose Start was
deleted. Include cleanup-first and pre-prepared-sibling cases. Existing
`tool_streaming::run_single_store_http_atomic_probe` and
`TestWorkerExecutor::gate_next_replay_access_admission` provide the closest
fixture and gate patterns; their current BeforeScope/AfterScope hooks do not
pause inside the required cleanup window. This remains a verification gate, not
a claimed reproduction.

## 6. Proposed central invariant and ownership

**Current recommended direction, pending design approval:**

> After an accepted status-actor commit completes its projection phase,
> authoritative operations see a valid projection covering that transaction or
> the owner's explicit failure/unavailability. A committed invalidation never
> depends on producer progress or a future reader to start repair. Projection
> validity never grants permission to continue a resident execution transition.

```text
Required commit milestone
    ├── early acknowledgement / eligible successful response
    └── actor-owned projection phase
            ├── incremental fold succeeds
            ├── invalid baseline → reconstruct using valid baseline
            └── reconstruction fails → explicit owner failure/unavailability
```

- The actor owns reconstruction after invalidation, even if its caller vanishes.
- Preserve the early commit receipt. Do not make ordinary successful responses
  wait for reconstruction or introduce a status read into result publication.
- Never admit work using the retained invalid projection.
- Repair errors must not fabricate retry state, silently strand invocation
  waiters, or recursively enter a failure handler requiring the same bad status.
- Preserve fencing: an old shard owner must not perform new writes or report
  successful execution admission after ownership is refused.
- Respect actor lock discipline: no acquiring lifecycle locks or awaiting
  Store-driven work inside the status queue.

### Fixed projection horizon and history source

Each actor transaction must establish a fixed oplog index through which its
projection is authoritative. Derive that horizon from acknowledged coverage:
the existing pre-commit index sample when the commit flushes the buffer, the
commit receipt, and any explicit committed/handoff watermark needed for receipt
gaps. Never treat a later raw `current_oplog_index()` as proof of commitment.
An ephemeral `DurableOnly` commit is a no-op; its pre-commit buffered tip does not
advance acknowledged coverage.

Use the same fixed horizon for region discovery, baseline validation, and all
incremental or reconstructing folds. Do not let each candidate select a new tip.
The existing `try_fold_status_from` obtains the latest persisted service index
itself, so calling it unchanged is insufficient for the resident actor contract.
Factor its existing fold into a bounded-reader path; preserve the cold persisted
reconstruction caller's separate source of history.

For resident ephemeral agents, `Deferred` acknowledges ordered writer handoff,
not physical persistence. Read the acknowledged prefix through the opened oplog,
whose `read_exact` merges resident handed-off/buffered history and archive data;
cap reads at the fixed horizon so later unacknowledged entries are excluded.
Do not force an additional commit merely to make this reconstruction readable.
Preserve the early receipt, and keep ordinary authoritative reads non-committing.

Acceptance tests must hold the ephemeral writer behind handoff, append a newer
unacknowledged sentinel, and verify reconstruction includes the acknowledged
prefix but not the sentinel. Cover receipt gaps, durable commits, and ephemeral
`DurableOnly` separately. This is in addition to the simple read-does-not-commit
test; a read can be non-committing and still fold the wrong prefix.

### Agreed storage-error policy

Unavailable or corrupt authoritative storage follows the existing oplog-storage
failure policy. Do not introduce projection-specific retries, fallback to stale
status, or a new worker-only recovery policy. Missing/invalid optional baselines
remain optimization misses; inability to read authoritative history is not.

This is not a promise that storage failure cannot terminate the process.
`services/oplog/primary.rs::retry_storage_op_fenceable` retries transient errors
using the existing retry configuration and panics on exhaustion or permanent
failure. Fencing is deliberately different: healthy storage refuses an old shard
owner, which retires that agent rather than aborting the executor. Preserve that
distinction. Match the authoritative read/payload path to the established policy
without accidentally treating an optional checkpoint-cache failure as a fatal
oplog failure. Do not copy indeterminate-write reconciliation into read retries.

The assertion being removed is about a recoverable derived-state condition,
not the system's deliberately fail-stop policy for unusable authoritative data.

Earlier proposals are superseded: neither replacing every assertion with today's
`attached_status()` nor requiring every Jump caller to await reconstruction is
the selected direction. Lazy retry of unavailable projection may remain a central
recovery policy, but not the primary owner of invalidation repair.

## 7. New proposal: retain invocation-start and snapshot baselines

The user proposes remembering status at invocation start, and possibly at the
last snapshot, so Jump/retry reconstruction can fold a short suffix instead of
re-reading the worker's full history. Include this optimization in the design.

**Agreed priority:** integrate invocation-start retention into the existing
baseline/checkpoint mechanism if practical. Prefer keeping it, but correctness
must not depend on it; if exact capture requires disproportionate complexity,
document that finding and omit the optimization rather than inventing a second
checkpoint system. A separate retained snapshot slot remains optional.

### Existing machinery to reuse

`worker/status_checkpointer.rs` already persists a separate clean status
checkpoint at snapshot, idle, and eligible mid-invocation boundaries. Snapshot
checkpoints bypass the idle throttle. Mid-invocation checkpoints respect open
rollback scopes and the exposed oplog-marker watermark. Checkpoints are optional,
best-effort, and incarnation-scoped; ephemeral agents do not persist them.

`calculate_last_known_status_with_checkpoint_reader` tries a cached baseline,
then lazily reads a clean checkpoint, then falls back to a full fold. Actor
reattachment currently supplies no valid live baseline.

### Proposed extension

- Retain a bounded actor-owned `Arc<AgentStatusRecord>` at the outer invocation
  boundary. Prefer the valid projection immediately before folding the new
  `AgentInvocationStarted`; replaying that entry then reconstructs invocation
  identity normally. Confirm this capture point against actual batching.
- Retain a separate latest snapshot-aligned projection if useful. The existing
  persisted checkpoint may later advance past the snapshot, so distinguish
  “latest clean checkpoint” from “last snapshot baseline.” Reuse existing
  checkpoint events rather than adding a second snapshot protocol.
- Use immutable references, not a deep clone on every invocation. Retaining an
  `Arc` is cheap but retains the underlying record and can affect copy-on-write
  costs; measure memory and fold cost rather than claiming zero overhead.
- Keep at most the chosen bounded set, not a history per invocation or nested
  tool call. Define the boundary as the outer owner invocation, not every host
  call or entity body.
- During repair, try usable in-memory candidates newest-first, then the existing
  persisted checkpoint, then full reconstruction. Avoid a checkpoint storage
  read on successful resident-baseline repair.
- Reuse the authoritative fold and baseline-validation logic for every candidate.
  **The baseline index merely being outside the Jump/revert is insufficient.**
  Newly invalidated history already incorporated into the baseline also rejects
  it. Do not weaken `baseline_is_invalidated`.
- A rejected baseline is an optimization miss; a real storage/payload error is
  not an excuse to silently substitute incomplete data.
- The optimization is disposable. Restart without these references must still
  recover through existing checkpoints/full history. Do not add per-invocation
  persistent writes or a new oplog format just to retain a baseline.

### Capture and lifetime questions to resolve

1. A commit receipt may contain preceding host records, the invocation Start,
   and later records. Capture the exact prefix, not the actor's arbitrary current
   snapshot. Can the fold expose that boundary without extra storage or copying?
2. Preserve the original baseline across retries of the same invocation. Do not
   overwrite it with a projection already containing the abandoned attempt.
3. Decide whether replay/cold reconstruction should repopulate these references
   while folding history, or initially rely on persisted checkpoints. Correctness
   must not depend on repopulation.
4. Snapshot alignment means a status projection at the relevant committed log
   boundary, not the guest-memory snapshot itself. Confirm the exact capture
   index, skipped-region semantics, and snapshot-update behavior.
5. Drop/reject references on incompatible incarnation/history changes. A revert
   spanning invocation start must fall back to an older valid baseline. A retained
   pointer is never authority merely because it came from the same worker object.
6. Bound memory retention and account for large invocation-result structures.
   Benchmark long-lived workers, repeated retries, and frequent snapshots.

### Initial implementation scope

Keep one actor-owned invocation-start candidate, captured inside folding at the
exact prefix before the outer invocation Start. Preserve it across retries and
validate it against the reconstruction horizon's regions. Do not add a separate
resident snapshot slot or repopulate candidates during cold reconstruction in
the initial implementation; existing persisted checkpoints remain the fallback.
Those additions are optional follow-ups, not prerequisites for this fix.

Capture must preserve the fold's batch semantics. In particular,
`update_status_with_new_entries_internal` distinguishes intermediate folding from
final oplog-processor checkpoint pruning. Do not implement capture by repeatedly
calling the public finalizing fold on pieces of a receipt. Preserve finalization
once at the intended publication boundary and test empty as well as split folds.
If exact capture cannot be implemented without disproportionate complexity,
use the user's allowed omission of this optimization and explain the cost.

## 8. New question: remove the failing status getter entirely

The user asks whether we can remove the getter that can fail, whether by panic
or `Result`, so future callers cannot recreate the problem.

**Remove the detached-status getter and caller-visible attachment choice.**
After actor-owned repair, callers should not choose among “assert attached,”
“repair attached,” and “try attached.” Delete `NonDetachedStatus` and the
corresponding worker method rather than keeping aliases. Do not expose a public
raw accessor as an easy escape hatch.

However, distinguish two promises:

1. **No ordinary getter can fail because status happened to be detached.** This
   is a plausible API goal: the actor resolves invalidation before dependent work.
2. **Obtaining authoritative state always succeeds.** This is impossible without
   an additional policy for storage failure, deletion, actor retirement, or
   shutdown. Returning stale state, inventing defaults, hanging forever, or moving
   the panic elsewhere does not satisfy it.

Agreed API direction:

- A single fallible authoritative operation hides detachment completely but
  retains the infrastructure/lifecycle outcomes appropriate to its existing
  owner. Detachment is never a caller-visible error or API choice. Apply the
  storage policy in section 6 rather than designing a new failure policy here.
- Status-dependent read/decide/append operations already owned by the actor stay
  there. Other callers can obtain a valid immutable projection after the actor's
  ordered projection barrier. Moving every status-dependent decision into the
  actor is not a requirement.

Prefer removing the attachment-specific API either way. Do not introduce a new
capability/lease framework unless actual call-site requirements justify it.
An immutable snapshot is valid for its recorded prefix, not a guarantee that no
later commit or history change has occurred. Read/decide/append operations such
as admission must remain atomic or generation-validated inside the actor.

**Settled:** eliminate detachment as a caller concern, not all standalone status
retrieval or all infrastructure failures. There must be no replacement
`try_non_detached_status`, assertion, or public raw escape hatch. Normal derived
state invalidation must never select the fatal storage-error path.

## 9. Affected caller inventory

Paths below are under `golem-worker-executor/src/` unless stated otherwise.

### All eleven asserted reads

| Location at inspected revision | Purpose / required contract |
| --- | --- |
| `worker/invocation_loop.rs:890` | Current invocation identity for interrupt during delayed retry. |
| `worker/invocation_loop.rs:1041` | Current invocation identity while unloaded waiting for a permit. |
| `worker/invocation_loop.rs:1868` | Pending work/update/snapshot decisions for permit acquisition. |
| `worker/invocation_loop.rs:1890` | Failure checking and next-work selection. Requires separate runtime readiness. |
| `worker/invocation_loop.rs:2132` | Remaining invocations for scheduler fairness after completion. |
| `worker/invocation_loop.rs:2175` | Periodic snapshot decision after completion. |
| `worker/mod.rs:3637` | Clear persisted recovery error after active startup succeeds. |
| `worker/mod.rs:3681` | Derive previous recovery error/retry position when recording failure. |
| `durable_host/mod.rs:5581` | Retry policy state before recording invocation failure. |
| `durable_host/mod.rs:5700` | Status after failure commit; decide terminal waiter handling. |
| `durable_host/call_coordinator.rs:571` | Pending-card-event baseline plus suffix scan. |

None requires treating a detached old record as authoritative. Some require a
separate proof that the runtime transition succeeded before acting on valid data.

### Existing attached, fallible, and observational consumers

| Surface | Consumers to update or audit |
| --- | --- |
| `worker/mod.rs` | Preparation/startup; metadata; quiescence; external-tool component revision; automatic-update admission; pending-invocation listing; versioned invocation admission; filesystem admission; wallet/card operations; stream status/recovery; export-fork status/reservation; revert; result lookup; unload queue resolution. |
| `worker/lifecycle.rs` | Metadata-dependent lifecycle operations. |
| `worker/durable_stream_slots.rs` | Session failure checks and component revision. |
| `durable_host/durability.rs`, `concurrent/call.rs`, `http/inline_retry.rs`, `p3/http/send.rs`, `wasm_rpc/mod.rs` | Retry-state reads from independent host operations. |
| `durable_host/mod.rs`, `golem/v1x.rs` | Permission boundary state and guest metadata. |
| `grpc/mod.rs`, `grpc/invocation_session/mod.rs`, `grpc/invocation_session/external_tool.rs` | Cancellation, invocation/update admission, external-tool metadata. |
| `services/rpc.rs`, `services/worker_fork/export.rs` | Streaming failure checks and export-fork status. |
| `Worker::get_latest_metadata` | Retirement-tolerant observation and persisted-state fallback. Preserve absence versus error semantics. |
| `golem-worker-executor-test-utils/src/lib.rs` | Export-fork admission and attached-status inspection helpers. |
| `tests/worker_initialization.rs`, `tests/filesystem_inspection.rs`, `tests/tool_streaming/` | FIFO freshness, pending-work observation, lifecycle/fingerprint tests. |

### Explicit reattachment boundaries

Audit `commit_replay_jumps`, post-replay wallet synchronization, concurrent scope
recovery, `append_revert`, and instance creation. Remove redundant projection
repair only after separating their commit barriers and runtime cache resets.

### Raw publication readers

Audit invocation success/failure publication, pending-waiter failure, admission
generation prechecks, stream consumer-journal lookups, stream-history gates and
topology horizons, oplog forwarding/plugins, flusher, and checkpointer.

Do not mechanically insert actor waits: successful completion deliberately uses
the early receipt, and oplog operations called by the actor must not await the
actor recursively. Each raw consumer needs a documented lag-tolerant or ordered
authoritative contract.

## 10. Performance requirements

- Attached authoritative reads: no added commit, storage read, full status copy,
  or reconstruction. Preserve one queue interaction where that is already used.
- Ordinary successful folding: no extra oplog scan. Baseline capture must not
  add per-invocation persistent writes.
- Early successful response: unchanged persistence contract and no new status
  wait. Later same-agent jobs may queue behind exceptional reconstruction.
- Resident Jump repair: fold from the nearest valid retained baseline when
  available; measure entries read and retained memory, not just elapsed time.
- Other agents have separate actors but share storage/CPU; avoid making repeated
  reconstruction failures a tight retry loop.

## 11. Verification plan

Use existing test-r conventions, `testing` and `understanding-durable-execution`
skills, scoped executor guidance, and deterministic gates rather than sleeps.

1. **Attached fast path:** counting storage dependency proves no reconstruction,
   empty commit, or extra storage read.
2. **Actor-owned invalidation repair:** commit a Jump, then cancel/fail its caller
   before replay registration. Projection work completes independently.
3. **Two failure-handler reads:** force invalidation between the reads; verify
   correct retry/terminal notification and no process panic or stale admission.
4. **Repair failure:** distinguish transient retry, exhausted/permanent
   authoritative-storage failure, optional checkpoint failure, and fencing.
   Match existing retry/fail-stop or retirement outcomes; never use stale status
   or introduce a recursive failure loop. Do not demand successful in-process
   waiter recovery after a deliberately process-fatal storage failure. Preserve
   existing region-aware fold fallback before declaring an error unrecoverable:
   a later revert can remove an offending entry. Use appropriate isolated test
   infrastructure for process-fatal behavior, not executor unit-test subprocesses.
5. **Non-committing read:** buffer a sentinel entry and prove projection access
   does not commit it. Preserve durable and ephemeral commit guarantees.
6. **Early acknowledgement:** hold projection processing behind a gate; commit
   acknowledgement completes while authoritative dependent work waits.
7. **Invocation baseline:** asymmetric state before and during the invocation;
   Jump discards part of the invocation. Result equals an independent full fold,
   and reads begin after the retained valid baseline rather than at entry one.
8. **Baseline invalidation boundaries:** test baseline inside a region, after a
   newly skipped earlier region, before a region, and repeated retries. The
   second case catches the incorrect “index outside region is enough” rule.
9. **Snapshot and fallback:** snapshot baseline survives a later Jump; revert
   invalidates invocation baseline; newest candidate invalid but older one valid;
   all candidates invalid/missing; cold restart without resident references.
10. **Exact capture and lifetime:** batched Start plus surrounding records,
    retries of one invocation, consecutive invocations, incarnation replacement,
    and snapshot updates. Check bounded memory retention.
11. **Runtime readiness:** pause primary Live publication versus Jump cleanup;
    attempt sibling host work. Establish whether a transition fix is required.
12. **Side effects:** authority synchronization, transfer dedupe, result caches
    across Jump/revert/restart, recovery-index publication, archive scheduling,
    and retirement observation.
13. **HTTP reproduction:** held POST raced with long durable wait, crash after
    arrival, release, verify another agent remains usable; GET and tool controls.
    Distinguish remaining replay failure from successful containment of the abort.

## 12. Scope, open decisions, and implementation handoff

The storage-error policy, API goal, and preference for an invocation-start
optimization are settled in sections 6–8. Before finalizing implementation, settle:

- The concrete routing into the existing storage/lifecycle policy, including
  already acknowledged invocations and queued waiters; no new policy is needed.
- Exact invocation/snapshot capture boundaries, bounded retention, and candidate
  selection using existing fold validation.
- Whether the primary Live-before-Jump-cleanup test demonstrates a separate
  transition defect and what minimal owner should fix it.
- Consistent publication side effects and the permitted raw-reader contracts.

Do not change POST idempotence policy, weaken replay checks, suppress errors,
change skipped-region selection without proof, or perform compatibility work.
Do not expand into removal of all unrelated process-fatal policies.

Implementation should use the smallest coherent status-owner/API change, update
all necessary in-tree callers, and keep replay-transition changes distinguishable
from projection changes. Update the executor walkthrough and durable-execution
guidance when behavior changes, as required by scoped repository instructions.

Run focused tests and formatting/checks; report precise reproduction limits and
performance evidence. Remove temporary diagnostic code. No push, PR, deployment,
or release is authorized by this document.

## 13. Ordered implementation handoff draft

The decisions above are sufficient to prepare implementation. The full-path
transition test is still needed before fixing the representation of that part of
the plan. Keep the following units distinguishable in review; do not implement
the optional baseline optimization before establishing projection correctness.

1. **Finish the transition evidence.** Build the gated real-accessor test from
   section 5.5 using existing executor fixtures and targeted component builds.
   Inspect both claim and terminal indices and replay the resulting history.
   Preserve ordinary retained-Start adoption controls. If another production
   guard prevents the suspected schedule, identify it explicitly and omit the
   proposed transition change. If the hazard is confirmed, turn the desired
   no-crossing invariant into a regression test and specify shared ownership,
   competing settlers, cancellation, and already-issued handles before coding.

2. **Specify projection contracts with tests.** In the existing state-actor and
   status tests, force invalidation between failure-handler observations and
   cancel the Jump producer after its commit receipt. Assert actor-owned repair
   and correct status values, not just absence of panic. Add a buffered sentinel
   proving authoritative reads do not commit; gate projection processing to
   prove early successful responses retain their existing acknowledgement point.
   Count storage reads/commits on the attached fast path.

3. **Centralize projection maintenance.** Change `StatusState`'s commit/fold
   phase to complete incremental folding or reconstruct invalidated status
   before dependent actor jobs run. Preserve the early receipt and cancellation
   independence. Introduce the fixed-horizon bounded history reader from section
   6 and reuse checkpoint/full-fold rules; do not mix in the new optimization.
   Route genuine authoritative-history errors according to
   section 6. Verify publication, authority generations, recovery-index updates,
   cache flushing, and archive scheduling. Keep hydrated-result reset at the
   appropriate runtime boundary rather than making every repair reset results.

4. **Remove the attachment-specific API.** Delete `NonDetachedStatus` and the
   asserted worker accessor. Replace authoritative callers from section 9 with
   the non-committing actor-ordered operation; do not retain aliases. Keep atomic
   admission inside the actor and stop ignoring reconstruction failures in
   `AppendInvocationIfVersion`. Audit raw consumers individually: lag-tolerant
   observers need an explicit validity contract; commit/result publication and
   actor-internal callbacks must not gain recursive actor waits. Search for the
   removed symbols and assertion text and compile all affected consumers.

5. **Fix the replay boundary if step 1 confirms it.** Implement the smallest
   shared pending-recovery obligation within existing replay ownership. Cover
   affected claims, live repair, competing settlers, cancellation/failure, and
   replay-target growth. Retain unrelated claims' progress and non-primary
   local-tail semantics. Run the failing full-path test through completion and
   another restart. Projection repair must remain independent of this transition
   finishing. Do not redefine skipped regions or HTTP idempotence as a shortcut.

6. **Add the invocation-start optimization.** Extend the existing fold/baseline
   machinery to capture the exact valid prefix before the outer invocation's
   Start, even within a multi-entry receipt. Retain a bounded immutable candidate
   and preserve it across retries of the same invocation. Try valid resident
   candidates before fetching a persisted checkpoint; use existing invalidation
   rules and full-fold fallback. Start with the persisted snapshot checkpoint
   mechanism; add a separate resident snapshot slot only if it is demonstrably
   useful. Do not add per-invocation persistent writes. Compare reconstructed
   values with independently folded asymmetric histories and count entries read.
   Verify retention and cloning costs rather than assuming an Arc eliminates
   copies. If this cannot stay small, omit it with a concrete explanation.

7. **Run combined verification and update guidance.** Run the targeted tests in
   section 11, then affected state-actor/status/replay suites and the real
   POST/wait recovery scenario. Check both durable and ephemeral early receipts,
   ordinary completion, retirement/fencing, snapshot/revert, and result caches.
   Run scoped formatting and compile checks; broaden only for changed shared
   contracts. Update executor documentation required by scoped guidance once
   behavior changes. Report fast-path storage counts, Jump reconstruction work,
   measured retention cost, and any reproduction limitation. Do not declare the
   incident fixed solely because the detached assertion is gone.
