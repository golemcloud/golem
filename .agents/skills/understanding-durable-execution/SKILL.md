---
name: understanding-durable-execution
description: "Explains how the Golem worker executor implements durable execution: oplog recording and deterministic replay, durable host calls and guest completion delivery, the replay-to-live transition, the invocation queue and results, durable RPC exactly-once, streaming invocations and durable streams, tool invocations and entity bodies, snapshots and updates, and interruption, suspension, and reconstruction. Use when changing, reviewing, or debugging golem-worker-executor code or tests that involve replay, oplog entries, crash recovery, idempotency keys, or worker lifecycle."
---

# Understanding Durable Execution

The worker executor runs WASM agents whose progress survives the loss of the process running
them. Read this guide alongside `golem-worker-executor/src/` and `tests/`; when code
and guide disagree, the code wins and the guide needs a fix. Combined verification of
the instrumentation removal and adapted tests is pending. The scoped `AGENTS.md` files under
`golem-worker-executor/` state the mandatory rules; this skill explains the mechanisms.

Deeper material lives in `reference/`: `timelines.md` (worked oplog timelines), `crash-matrix.md`
(what recovery does for each crash window), `testing-patterns.md` (tests that fail under a wrong
model), `streams.md` (durable streams and streaming invocations), `tools.md` (tool invocations
and entity bodies), `retries.md` (in-function versus trap-based retries), and
`filesystem-inspection.md` (exact-path live reads, shared scheduling and generation-pinned production).

## Three axioms

1. **Replay is deterministic by construction.** The guest is a deterministic function of its
   guest-observable host inputs. The executor records every nondeterministic host interaction in
   the oplog and feeds the recorded result back during replay. If a replaying guest asks for a
   different host call than the one recorded next, the executor has a bug (missing recording,
   wrong reconstruction, wrong completion association). It is never "normal nondeterminism".
   The *host* side is not deterministic — completions are tokio futures and finish in varying
   order — but replay reproduces the guest-visible completion order from the recorded
   `CompletionDelivered` markers, so the guest sees the same inputs in the same order.
   Owner: `durable_host/replay_state/claims.rs` (a missing `Start` match while replay is active
   is a strict divergence error, except for a crash prefix whose remaining history belongs
   to other owners; see "Entry ownership". Operator-reverted entries are excluded). The axiom is about
   replaying the *same* execution contract; replaying old history against a changed component
   during an `Automatic` update may legitimately diverge, and that is reported as `FailedUpdate`,
   not tolerated.

2. **The resident runtime is disposable.** The Wasmtime `Store`, the worker task, the executor
   process, sockets, channels and subscriptions can vanish while work is pending. Suspension,
   eviction, restart and crash must all be recoverable the same way, on the same executor: throw
   the instance away, build a new `Store`, replay the oplog, continue. Lifecycle hints (`Suspend`,
   `Interrupted`, `Restart`) change status and scheduling policy, not the recovery mechanism.
   Losing the shard does not reconstruct in place: a durable agent's oplog asserts this
   executor's shard epoch whenever it appends or deletes entries in storage, and once another
   executor holds the shard, the write is refused (`OplogError::Fenced`) instead of accepted. The agent is then *retired for
   its lost shard* (`InterruptKind::ShardLost`): that generation is stopped, dropped from this executor and never
   restarted, and the executor that now holds the shard builds a new `Store` and replays — the
   new owner, or this executor again if the shard came back to it at a higher epoch. See
   "Resharding, revocation and the oplog epoch fence" below and `crash-matrix.md`.
   Owner: `worker/invocation_loop.rs::run` (outer loop: create instance → recover → run →
   suspend/retry) and `durable_host/mod.rs::prepare_instance`.

3. **Durable agent RPC is exactly-once at the logical callee-invocation level.** Dispatch attempts
   (replay, transport retry, incomplete-call repair) share a durable key. An executing target persists one
   invocation and result; read-only cache hits/followers persist neither. Attempts are not executions. Owner:
   `durable_host/rpc/mod.rs`
   (`IdempotencyKeyPosition::derive(start_index)`), `worker/mod.rs::enqueue_worker_invocation_with_effect`
   (`lookup_invocation_result` dedupe before appending `PendingAgentInvocation`).

```diagram
┌──────────────┐ host call ┌──────────────────┐ append ┌──────────────┐
│  live guest  │──────────▶│ durable_host fn  │───────▶│    oplog     │
│  (Store #1)  │◀──────────│ Start…End…Deliver│        │ (persistent) │
└──────────────┘  result   └──────────────────┘        └──────┬───────┘
        ✕ Store #1 disappears (suspend / evict / crash / restart)  │
┌──────────────┐ host call ┌──────────────────┐  read   │
│ replay guest │──────────▶│ claim Start,     │◀────────┘
│  (Store #2)  │◀──────────│ resolve End,     │
└──────────────┘ recorded  │ deliver at marker│
                           └────────┬─────────┘
                    cursor exhausted│ + reconstruction validated
                                    ▼
                           publish Live → new work appends again
```

## Durable versus resident state

| Durable (authoritative) | Resident (disposable, derived) |
|---|---|
| Oplog entries and their payloads (`golem-common/src/base_model/oplog/mod.rs`) | Wasmtime `Store`, instance, linear memory |
| `PendingAgentInvocation` / `AgentInvocationStarted` / `AgentInvocationFinished` | `Worker.queue: VecDeque<QueuedWorkerInvocation>`, event subscriptions |
| Idempotency keys and recorded invocation results | `hydrated_invocation_results` cache, read-only cache |
| Durable-call `Start`/`End`/`Cancelled` and `CompletionDelivered`/`CompletionDiscarded` markers | `DurableCallSession`, `ReplayableOneshot`, spawned store tasks |
| `PendingUpdate` / `SuccessfulUpdate` / `FailedUpdate`, `Snapshot` hints, snapshot blobs | In-flight update decision, loaded snapshot bytes |
| Validated operator revert/fork cuts and retained history | Replay cursor and runtime handles |
| Agent status **folded from the oplog** (`worker/status.rs`) | Status cache / checkpoints (baselines only) |
| Persisted promises and scheduler actions (`WakeupScheduler`), `RunningWorkers` recovery index | Tokio timers, in-memory wake channels |

Anything in the right column may be rebuilt from the left column at any time; a design that needs
something from the right column to survive a restart is wrong. Sockets and other OS resources are
recreated, not preserved (`durable_host/sockets`, `durable_host/http`); the application protocol
must tolerate reconnect or explicitly handle loss. Replayed websocket handles use their connect-time
reconstruction policy under a per-handle coordination gate, so concurrent calls cannot reconnect
one handle twice or replace a terminal session-loss outcome (see
"Concurrency and guest completion delivery"). Durable liveness also needs *rediscovery*: a timed
wake is persisted as a scheduler action before automatic suspension (`worker/suspension.rs::RuntimeStore::drive`,
`WakeupScheduler::sleep_until`), and the shard-keyed `RunningWorkers` index is updated
synchronously so a crash/reshard can enumerate workers with pending work
(`worker/status_flusher.rs`). The status blob cache is an asynchronously flushed baseline only.
Status, clean-checkpoint, invocation-result-index, and durable-stream-index cache namespaces include
the current `Create.instance_id` (`AgentFingerprint`). Readers select them only after resolving the
authoritative `Create` entry, while resident workers carry that fingerprint directly. A delayed
writer from a deleted incarnation therefore cannot mix fields into its replacement's derived state.
These namespaces have a renewable expiry and are rebuilt from the oplog after expiry or cache loss;
metadata compare-and-mutate operations make each multi-field publication atomic. The flat cached
agent mode only chooses which oplog namespace to probe first and never proves existence or identity.
`RunningWorkers` entries also include the fingerprint, so shard recovery admits only the recorded
incarnation and removes only an exact stale member. A member is stale when its agent has no oplog,
or when the oplog of the agent belongs to another incarnation (`stale_reason` in
`services/worker.rs`). The recovery scan (`WorkerService::get_running_workers_in_shards(on_stale)`)
and the restart after it (`WorkerService::remove_if_stale`) request `delete_all_snapshots` for the
dead incarnation of each stale member before they remove the member. A removal that fails leaves
the member, and the next scan requests the delete again. A delete request that a full clean-up
queue refuses is counted as a leaked clean-up: the scan still removes the member, and no later
scan asks again.

Two persistence steps matter for every crash window: **append** puts an entry in the oplog
buffer; **commit** makes it recoverable (`commit_oplog_and_update_state(CommitLevel)`). The
buffer is a performance trade-off (no storage round trip per append). `CommitLevel`
(`services/oplog/mod.rs`) says how strict a commit is: `Always` waits for durable storage;
`Deferred` still waits for durable oplogs but lets an ephemeral oplog return after ordered writer
handoff (bounded queue backpressure may wait); `DurableOnly` waits only for durable agents and
remains a no-op for ephemeral agents. Explicit protocol barriers and invocation acceptance still
use their existing storage guarantees; this is not a global weakening of commit semantics.

Successful invocation completion has a narrower contract. It appends
`AgentInvocationFinished`, then waits for the state actor's commit receipt before notifying
waiters. Durable completion uses `Always`, so that receipt follows storage. Ephemeral completion
uses `Deferred`, so its receipt proves ordered writer handoff, not storage. Neither mode waits for
the status fold: the actor retains that fold after acknowledging completion. A later actor-FIFO
status read that requires freshness waits behind the queued fold; the separately persisted status
cache remains asynchronous. Admission and authority jobs continue to await their complete folds,
and the `RunningWorkers` recovery index remains synchronously flushed by the status actor.
The unchanged synchronous `Create` write preserves an ephemeral agent's identity before execution;
after executor loss, that identity reconstructs an observation-only owner, never a fresh execution.

A commit can also be *refused*. A durable agent's primary oplog, opened while this executor holds
the agent's shard, asserts that shard epoch whenever it appends or deletes entries in storage — an
explicit commit, a threshold flush, a deletion — inside the storage transaction. Storage refuses the write with
`OplogError::Fenced` unless the record it holds names that epoch: a newer epoch means another
executor took over, and no record at all refuses too. An append only buffers, so it meets the check when the buffer is committed.
`commit_oplog_and_update_state` and `add_and_commit_oplog` surface the refusal rather than
swallowing it, so a refused `PendingAgentInvocation` commit is not acknowledged as accepted and a
refused `AgentInvocationFinished` commit is not published to waiters. The first refusal latches:
every later add or commit on that handle is refused locally, without reaching storage, and the
agent is retired for its lost shard instead of retried. The refusal travels as a typed error
(`OplogError::Fenced`, `StreamStoreError::Fenced`, `SessionError::Fenced`,
`WorkerExecutorError::OplogFenced`) to whoever acts on it; nothing re-reads the latch to
reinterpret a failure that already happened.

The shared direct and accessor durable-call start paths commit `WriteRemote` with
`CommitLevel::DurableOnly` after appending the actual `Start` and before returning the session
that permits the effect. This preserves preceding guest observations, including random inputs
used in a guest-supplied idempotency key. A refused commit prevents dispatch. The accessor path
releases the owner-wallet boundary before awaiting the commit. Completion still commits the `End`.
Snapshot calls append nothing; the barrier does not force storage for ephemeral agents.
The fence is checked at the commit, not atomically with the external effect: a shard can still
move after a successful commit, so the receiver must deduplicate retries. Socket sends,
DemandStream parents, and RDBMS protocol owners retain their explicit barriers. Oplogs that
assert no epoch are never refused: a handle opened before this executor has an assignment, and
fork stages and their publication. The archive transfer is fenced like the primary oplog (see
"Resharding, revocation and the oplog epoch fence" below), and so is an ephemeral agent's oplog,
which writes only through the compressed archive levels. The blob archive layer is fenced
through its manifest in indexed storage, since blob storage has no conditional write. One write
stays outside the fence: blob uploads of large payloads.

`worker/state_actor.rs::commit_and_update_state` samples the appended tip before its explicit
commit and ignores receipt entries already folded into the published status. Primary/ephemeral
threshold flushes and replica waits commit outside an explicit commit. After such a commit the
primary oplog calls its auto-commit listener (`Oplog::set_auto_commit_listener`, held weakly),
which queues one coalesced `StatusJob::FoldAutoCommits`. That job commits nothing: it folds the
entries returned by `CommitLevel::ReportOnly`, so `status.oplog_idx` follows the committed index
during a long invocation, while a buffered append below the threshold stays invisible until a
commit. The primary oplog keeps the entries of its latest auto commits in memory until that
report, which normally follows at once, up to `oplog.max_retained_auto_commit_batches` batches;
beyond it the report reads the oldest unreported entries back from storage. An ephemeral oplog
calls no listener and reports nothing for `ReportOnly`: ephemeral agents fold at the explicit
commit that ends each invocation. The job wakes the invocation loop only when a field other than the index changed; a
caller of `add_and_commit_oplog_internal` sends its own wakeup after every successful commit,
because the fold of a threshold commit can take its entry first. As the status follows the
oplog, each status-cache write catches up the stream-session and invocation-result indexes over
only the entries since the previous write. Even an empty receipt may still hide a committed
suffix. Ephemeral threshold flushes hand batches to a bounded asynchronous
writer; read snapshots include both handed-off entries and the buffered tail after the persisted
writer watermark. Up to 32 batch receipts are retained for status folding. If that cap creates a
receipt gap, exceptional catch-up forces storage only after the completion receipt has already
acknowledged the caller, then reads the missing range. Otherwise, unless the remaining receipt is
exactly the contiguous suffix after the last published index, the status actor catches up with
`status::try_fold_status_from`: committed storage is read in bounded chunks, external
`StreamSession` payloads are hydrated, and the result is published once. This avoids retaining an
unbounded auto-flushed tail and adds neither oplog entries nor a protocol change. Gap recovery
conservatively invalidates authority snapshots after the fold. This is status reconstruction, not
replay tolerance.

## Component map

| Area | Files | Responsibility |
|---|---|---|
| Oplog model | `golem-common/src/base_model/oplog/{mod.rs,oplog_macro.rs}` | Entry kinds, `is_hint()`, Start/End pairing rules |
| Replay cursor | `durable_host/replay_state/{mod.rs,cursor.rs,claims.rs}` | Single cursor lock, speculative read vs commit, identity claims, Replaying→Settling→Live |
| Durable calls | `durable_host/concurrent/{mod.rs,call.rs,delivery.rs,replay.rs}` | `DurableCallSession` state machine, drop policies, completion delivery tokens |
| Durability guard | `durable_host/durability.rs` | `begin_durable_function`/`end_durable_function`, `DurableFunctionType`, in-function retry |
| Host context | `durable_host/mod.rs`, `durable_host/recovery.rs` | `prepare_instance`, `replay`, `switch_to_live`, `PendingReplayToLive`, `IdempotencyKeyPosition`, snapshot boundary checks |
| Tail work | `durable_host/tail_work.rs` | Keeps store tasks running until no durable work is active before `AgentInvocationFinished` |
| RPC | `durable_host/rpc/mod.rs` | Key derivation, first dispatch vs `MayExist`, replay claims, ephemeral phantom identity |
| Worker | `worker/{mod.rs,invocation_loop.rs,lifecycle.rs,instance/mod.rs,status.rs}` | Queue persistence, dedupe, interrupt/resume/update decisions, eviction, retry decisions |
| Owner wallet | `worker/owner_wallet.rs`, `worker/instance/mod.rs` | Generation-bound live wallet, recorded replay baseline and shared cursor installation |
| Cut points | `worker/cut_point.rs` | Validates operator cuts; calls recover from the exact retained prefix |
| Durable streams | `durable_host/{durable_stream/mod.rs,durable_session/mod.rs,stream_session.rs,stream_bus.rs,stream_transport.rs,schema_value_stream.rs}` | Producer-oplog stream records, consumer session journal, exactly-once item delivery, protocol terminals |
| Tool invocations | `durable_host/tool/{mod.rs,streams.rs,boundary.rs,operation/mod.rs,attachment.rs}`, `durable_host/entity.rs`, `worker/{entity_slot.rs,owner_lane.rs,entity_invocation.rs,instance/mod.rs}` | Per-admission authorization, inherited invocation scope, pinned middleware plans, typed boundaries, owner-oplog entity bodies, shared replay state, operation settlement |

## Worker lifecycle and reconstruction

`OwnerExecution` installs the replay cursor and live wallet together in `OwnerReplayGeneration`.
Startup seals old append tasks, takes the card boundary, commits the horizon and selects the wallet
baseline before `install_replay_generation_locked` joins prior owner cleanup and begins the new
tool/deferred-admission generation. That installer is the single reset point. The earlier resident
attempt counter does not reset owner execution. After wallet installation succeeds, the loop marks
the owner as started so the next resident receives a new resource-release observation scope,
including when later context construction fails. Lifecycle markers do not use that bookkeeping.

`Worker` (`worker/mod.rs`) receives invocations, persists them, and processes them in order. The
run loop (`worker/invocation_loop.rs::run`) is:

```diagram
┌─────────────────────────────────────────────────────────────────────┐
│ outer loop (one iteration = one instance attempt)                   │
│  monthly admission ──▶ create Store + instance ──▶ prepare_instance │
│    durable agent:  handle PendingUpdate ▸ try snapshot ▸ replay       │
│    ephemeral:      constructor-only replay, append Restart          │
│  ──▶ inner loop: pop durable queue, run invocation, persist Finished │
│  ──▶ instance ends (idle / interrupt / trap / suspend)               │
│  ──▶ RetryDecision: Immediate | Delayed | ReacquirePermits | None | TryStop │
└─────────────────────────────────────────────────────────────────────┘
```

Only `prepare_instance` returning `Ok(None)` completes filesystem replay and reconstruction,
activates the resident generation, and enters the inner loop. `Ok(Some(decision))` is an aborted
preparation, including `Some(RetryDecision::None)`. `PreparedAgent::finish` seals that reconstructing
filesystem without drainage and retains the runtime for the ordinary teardown and retry tail.
The unload continuation drops the Store before it drains the sealed filesystem, closes the execution
window, releases the permit, and deletes the filesystem. A P2 output stream can retain a pending
`FilesystemCall` after interruption cancels only its borrowed readiness future; draining replay
with that Store still alive would wait on a call that only Store destruction can detach.
The original retry decision, later interrupt arbitration, ephemeral fail-stop conversion, and
cleanup-failure handling remain in the common tail.

`DurableWorkerCtx::replay` (`durable_host/recovery.rs`) loops `get_oplog_entry_agent_invocation_started`, replays
each recorded invocation in `InvocationMode::Replay`, and when there is no further
`AgentInvocationStarted` it switches to live. Ephemeral owners replay at most one recorded
`AgentInitialization`, and only when their resolved owner is a typed agent. Component-baseline
tool owners instantiate the deployed component but never queue or replay an agent constructor.
Constructor-only replay covers restarts before the accepted invocation ran. An ephemeral agent
never reconstructs in process once its Store has run: `on_invocation_failure` turns traps that
would restart it into terminal failures and notifies the caller. Any other `Immediate`, `Delayed`,
or `ReacquirePermits` decision reaching the outer loop becomes a terminal interruption plus archive,
without replacing an already published outcome. An active `Suspend` also becomes a terminal
`EphemeralCannotSuspend` failure after interrupt acknowledgement and monthly-exhaustion conversion,
so a lowered memory limit or failed monthly monitor cannot leave a method suffix for constructor-only
replay. Monthly capacity exhaustion retains its specific monthly error, and a monthly-policy outage
keeps its `Monthly enforcement unavailable` reason. Idle and pre-method suspension remain safe:
ordinary ephemeral agents keep their outer-loop `TryStop` unchanged;
ephemeral external-tool owners also treat it as terminal. Startup retries before a Store exists
remain unchanged. A started method never runs again live.
Replay starts from the chosen snapshot baseline
(see Snapshots and updates), not necessarily from `OplogIndex::INITIAL`. A new replay generation
scans the replayed range for recovery metadata (delivery markers, guest stream observations and
tool attachment indexes) before it resolves any call. A snapshot start scans only the entries
after the snapshot (`replay_state/cursor.rs::recovery_metadata_floor`): no durable construct
spans a snapshot, so nothing after it refers to metadata before it. A start without a snapshot
replays the whole history and scans it. Interruption kinds
(`Worker::set_interrupting`): `Interrupt` stays interrupted, `Restart` is a simulated crash with
automatic recovery, `Suspend` unloads and resumes on demand — each of these three reconstructs on
this same executor. `ShardLost` does not: it means this executor lost the agent's shard (a
revoked/reassigned shard, or an oplog write refused on the shard epoch), and instead of
reconstructing, that generation is retired — stopped without writing its status, dropped from
this executor and never restarted — and the executor that now holds the shard reconstructs it.

Eviction (`EvictionClass::{LoadedIdle, WarmRunnable}`) never unloads a
worker that is executing or holds non-durable in-memory work. Ephemeral agents are fail-stop:
`reconstructed_ephemeral` rebuilds only for observation and result lookup, "but the instance must
never be started again" (`worker/mod.rs`, `INACTIVE_EPHEMERAL_AGENT_ERROR`).

### Monthly admission, interruption, and recovery

Inherit Worker acceptance, typed signals, invocation-loop cleanup and ordinary reconstruction;
record independent defects and unverified combinations separately from the bounded delivery claim.

Monthly policy uses the shared account `AtomicResourceEntry` in `services/resource_limits.rs`.
Registry revisions arrive through the existing refresh path; stale refreshes must not restore
older policy. Admission in `worker/invocation_loop.rs` checks the latest cached capacity before
instance creation, replay resumption, and invocation. Reconstruction must pass those checks too;
a resume or queued invocation cannot bypass an exhausted applicable cap.

`WorkAvailable` and `InternalStatusChanged` are wake notifications, not work reservations. Before
reacquiring an idle concurrent-agent permit, the loop checks resident requests, pending interrupts,
detached status, durable pending work and snapshot policy. A notification whose work already drained
does not reopen a billing window. Detached status still wakes ordinary repair before work selection.
The loaded compute-admission test queues two real readiness requests while the actual Finished
append is withheld by the oplog dependency. An awaited filesystem update joins the earlier
notifications on the same command queue. Check queued-work results and accounting;
this no longer measures the number of execution-window acquisitions.

During guest execution, `Context::ensure_fuel`, the `FuelManagement` implementation in
`workerctx/default.rs`, calls `ensure_monthly_resource_capacity` to check enabled live compute,
then monthly memory and the agent's matching storage class. `worker/instance/mod.rs` calls it from the
existing Wasmtime epoch callback, and `worker/invocation.rs` checks before guest calls. Compute
and memory exhaustion apply account-wide; durable storage applies only to durable agents and
ephemeral storage only to ephemeral agents. Durable exhaustion requests suspension and unload
with `RetryDecision::TryStop`. Ephemeral exhaustion fails the invocation without retry, using
`EphemeralFuelExhausted` for compute or `EphemeralCannotSuspend` for memory/storage. Production
hard-limit execution does not borrow local ephemeral overdraft. Enforcement infrastructure
failure is distinct from exhausted capacity and must not be reported as an ordinary cap hit.

Capacity can return after a UTC month reset, a grant or plan increase above current usage, or
application of an `allowOverage` revision. Subsequent demand uses the same admission and
reconstruction path, not a second recovery system. Mode changes reach running agents through
refresh within five minutes. Idle refresh eligibility reserves one batch interval for the next
scheduled tick, so an account initialized just after a tick does not wait an extra interval.
The batch caches desired per-agent filesystem limits without awaiting a resident invocation's
filesystem-command reply. Each account has one latest-value queue and a serialized consumer that
uses the existing target callback. The Worker invocation loop still owns quota application and its
reply. A pending physical update cannot stop later monthly-policy refreshes or other accounts.
Queued newer limits cannot be overwritten in the cache by an older completion. Failures stay logged
and a later ordinary refresh can retry the same limit. Batch completion does not prove physical
quota application; explicit awaited filesystem-update callers retain their completion/error contract.
Usage attributed to an applied `allowOverage` revision remains eligible for excess billing until
`hardLimit` applies. Registry currently allocates the allowance in revision order rather than exact
cross-executor accrual order during overlapping refreshes. That MVP limitation is tracked in
[GOL-848](https://linear.app/golem-cloud/issue/GOL-848/preserve-chronological-allowance-allocation-across-overlapping-policy).

Memory and storage exports retain measured hard-limit overshoot instead of clipping it to capacity.
Fractional byte-time remains exact across settlements and policy/month cutovers. Overshoot totals
are diagnostics, not a second capacity debit. Billing eligibility is separate from measured usage.
Registry limit requests carry the deployment's existing meter flags; disabled runtime dimensions
skip usage reads and allowance calculations. All-off reads resolve per-agent policy without creating
monthly billing revisions or baselines, and aggregate display does not create runtime billing state.

Registry replies carry delivery acknowledgement separately from monthly-policy availability. A
committed usage update can be acknowledged even when the next policy read fails. The account entry
marks enforcement unavailable without installing the fallback's mode, period or capacity. Admission,
guest checks and the existing monitor report infrastructure failure rather than quota exhaustion.
The entry retains the latest observed policy revision as a floor, so an older reply cannot restore
capacity. Unavailable entries refresh on the next existing batch tick; a valid policy restores
admission without resending acknowledged usage. All-off entries have no monthly availability gate.
Guest-entry, post-instantiation and epoch checks share `worker/invocation.rs::resource_capacity_error`.
Only typed resource exhaustion becomes durable suspension. Infrastructure errors retain their
`AgentError` and retry-policy classification; the epoch boundary preserves raw interruption for a
real suspension.

Fuel keeps the existing prepaid/refund meter. Production Wasmtime call hooks sample the physical
gauge at guest/host boundaries. `FuelUsage` keeps a separate metered gauge that decreases only
while that Store executes live guest code inside an actual guest-body scope. Physical fuel remains
visible in invocation diagnostics. Core instantiation, completed replay and snapshot loading do not
borrow or charge customer fuel. Snapshot saving keeps its existing live accounting.
A remaining prepaid reservation can retain its borrow
revision and month across a transition, with a default remaining batch of 10,000 fuel. Failed Registry
delivery can also lose captured usage; ambiguous additive updates are not blindly resent. Delivery
and customer-credit safety are tracked in
[GOL-847](https://linear.app/golem-cloud/issue/GOL-847/recover-resource-usage-safely-after-registry-delivery-failures).

Disabled meters omit their monthly accounting, caps, overage, and billing. Filesystem metering
controls both storage meters but their amounts and usage remain separate. Per-agent memory and
managed-XFS quota enforcement remain independent; unmanaged storage has no finite per-agent quota.

The single-binary configuration rejects enabled per-agent storage policy without managed XFS,
independently of filesystem billing. Split-service executors reject a finite Registry storage limit
on unaccounted storage instead of silently resolving it to Unlimited. Startup validates this before
filesystem creation. A live limit update uses the same resolver and the existing seal/unload path
on failure. Unlimited policy remains available on unmanaged storage; Registry's finite policy is a
deployment requirement, not permission for an unsupported backend to omit enforcement.

A live invocation still holds its allocated linear memory and filesystem stock while waiting for
host I/O. Its live allocations accrue byte-time during these permit-held waits even when no Wasm
executes. The memory meter measures allocated linear-memory bytes, not the configured maximum.
The parked guest itself burns no fuel.

Reconstruction keeps physical admission, the concurrent-agent permit, allocation observations,
per-agent limits and cleanup ownership. Materialization and replay do not start customer byte-time.
`GuestResourceUsageGuard` covers exported guest calls, native bodies and stream drains. Successful
`PendingReplayToLive::finish_at` activates usage only inside such a body. A primary replay tail outside
the body therefore does not bill final filesystem verification. Completed entity replay remains
unbillable; an incomplete entity activates its own memory after its local live admission succeeds,
even while the primary is still replaying. Its activation does not bill primary replay memory.

A filesystem-capable live body activates the generation's current storage stock. An incapable
entity does not. That live stock remains billable through cleanup and permit release; separate blob
stock accounting is unchanged. Entity activation shares the owner's memory-transition lock with
window closure, so a late activation cannot resume a closed or dormant window.

The concurrent-agent permit bounds the billing window but does not itself make replay billable.
Permit release settles and pauses metering
before publishing reclaimable `LoadedIdle` or `WarmRunnable` state. This ends the billable window by
design; it does not mean every cached allocation is physically freed immediately. Released-permit
durable sleep, reclaimable cache time and unloaded time do not accrue memory or storage byte-time.

`worker/monthly_limits/mod.rs` attaches one monitor to an enabled, permit-held execution window. It checks
local capacity on a normal 30-second cadence and on applied Registry updates, settling owner-scoped
byte-time first and reading published fuel reservations without locking the Store. Actual changes
to billable memory also wake the monitor through the memory meter's rate watch. The monitor
subscribes before reading the rate and retains changes across its read/wait gap. Rate changes do
not publish a policy revision. Before each wait,
it settles that usage and wakes earlier when exact remaining monthly memory byte-nanoseconds divided
by the primary runtime's billable allocated linear-memory bytes predicts exhaustion sooner. The
horizon rounds up to a nanosecond; zero allocation, exhausted or inapplicable memory caps retain the
normal cadence, including rejected-stop retries. This owner-rate estimate is not a global account
prediction or a universal interruption deadline. It neither scans other
account owners nor changes Registry refresh cadence. All-off windows have no monthly monitor or
capacity subscription. Window closure invalidates its target and joins the monitor. During unload,
the filesystem drains before its final allocation read. The execution
window then settles and releases its permit before filesystem deletion, as in the existing unload
path. Failed deletion retains its filesystem reservation and explicit repair obligation, not the
closed billing window or execution permit. Repair verifies physical cleanup without reopening or
resettling the closed window. Cleanup failures still prevent startup on that Worker. Physical cleanup
receipts remain unfinished through failed deletion and complete only after verified repair.
Ordinary window close owns the final allocation sample and its existing deadline. There is no
separate frozen-allocation phase. Final-read retry and customer-favoring timeout behavior stay as
in main. A fault published during active-window enforcement remains failed health through close
and verified filesystem deletion; a transient final read alone does not create that retained fault.
Ordinary startup rollback after a window opens uses the same unload path. This does not cover
independent startup panic or cancellation: the filesystem and execution-window Drop fallbacks
have separate physical tasks, and the panic notification does not join them.

Initial idle settlement precedes startup readiness. Monitor panics, lost delivery and settlement
failures survive successful filesystem deletion as `CleanupFailed`; they
neither request immediate reconstruction nor acquire a replacement permit. The monitor treats a
closed lifecycle channel or lost send as an infrastructure error, retains that health, and requests
a cooperative Worker stop. Filesystem observation timeout or failure publishes retained window
health to that same monitor. Missing storage measurement is unavailable, not a zero sample or quota
exhaustion. The existing read remains owned and final allocation/drain work is still joined before
physical release. Known live memory continues accruing during that wait; unavailable storage intervals
do not fabricate measurements. A late successful read does not erase the window's failure.
If status work then panics because the actor has stopped, the invocation
loop retains its resident agent across the borrowed inner-loop failure and runs ordinary unload
without submitting more actor work. Tests in `tests/api/interruption/lifecycle/health.rs` cover initial
settlement, later successful invocations and pending silent TCP. Other panic paths still use the outer
panic boundary.

The monitor has no test-only panic selector or panic token on its target. Native health tests supply
a panicking adapter through the existing weak owner-usage-flusher interface. Initial settlement,
selected-result recording and already-published success need distinct proof boundaries; see
[idle health with distinct success boundaries](reference/testing-patterns.md#idle-health-with-distinct-success-boundaries).
Cleanup, startup refusal and the original error remain strict without changing the monitor's await
graph. The actor case holds the actual Start commit of a later live RPC after observing the
successful allocation result and its real idle release; no idle-close control remains. Monthly monitoring
uses the same ordinary Tokio timer in production and test builds.

Actor-loss tests fail a real positive GrowMemory append through the existing oplog
fixture. Counter.allocate_memory supplies guest allocation; the TCP case uses its
running invocation. The existing entity-fence log can establish stop-path entry,
not physical drainage. Check the lost-actor error, physical deletion, retained failed
health, startup refusal and original invocation records. Execution-lifetime tracing
does not supply generation or permit-acquisition counts. Initial idle precedes guest
execution, so a guest-growth fault cannot establish that initial boundary.

The settlement-loss selector is removed. It previously completed settlement
successfully, then dropped the reply through Worker and window flags. The ordinary ObserverLost
fallback remains. A colocated test faults the actual spawned close task through its existing
MeteringClock dependency, requires the exact error and no successful settlement, and then requires
the same real FIFO contender to acquire capacity. It adds no runtime selector or Worker interface.
Missing acknowledgement alone does not prove that settlement failed or authorize settling again;
this test's known clock fault occurs before settlement.
The native fake reply-drop cells retire with their producer; the native health helper retains its
other exact cleanup and startup assertions. This is not native settlement-task panic coverage.
Earlier library, native, lint, production and browser results are historical;
combined verification after instrumentation removal is pending. The post-success
test uses the real RPC commit boundary described in the linked testing patterns.

A temporary Registry policy outage is an execution failure, not a disposal failure. The monitor
requests the same cooperative stop and retains this error separately from cleanup health. After
successful settlement and physical disposal, the ordinary unloaded-startup path reports the
infrastructure error. A valid policy refresh allows the same cached durable Worker to restart through
ordinary admission and reconstruction. Failed cleanup still prevents startup; policy recovery cannot
erase it. Ephemeral agents remain fail-stop.

Monitor health belongs to the existing monthly window target, as failure data rather than a stop
task. Its first cleanup failure remains authoritative; a policy outage cannot replace it. A failed
monitor join records that same health even after the target closes. Operational policy failure is
consumed separately when the unloaded loop reports it to current waiters. A healthy settlement
therefore does not turn a policy outage into failed cleanup, and policy recovery cannot erase a
real monitor failure. Native filesystem proof/profile gaps remain separate from
monitor-health checks.

Interruption uses main's `InterruptEstablishment`. Queueing moves the tail in the same locked step
that accepts the request. An independently spawned task waits for its predecessor, begins the owner
entity fence, notifies the primary Store, and drains the fenced entities. Requester cancellation does
not cancel this task. Each queued request retains its own establishment, including a Restart followed
by a terminal request. The queue can replace Restart before it is taken, but an earlier establishment
may already have signalled Restart. There is no frozen elected cause or shared publication leader.

The invocation loop takes the exact queued request and waits for its establishment. At an idle permit
wait it drops acquisition before that wait. It keeps deferred commands and filesystem-limit replies. A taken
no-body stop retains its unwritten lifecycle marker through disposal, and the unloaded-stop handler
writes it once. It preserves completed results. If a terminal Interrupt stops a live ephemeral owner
after constructor success but before its accepted method starts, it notifies that pending key through
`store_invocation_failure`. It records no method Started or Finished and leaves constructor success
unchanged. A Restart can leave the same loop waiting without a loaded Store.

Startup uses main's establishment-tail/current-pointer and idle-queue check before reopening entity
admission. The wallet installer remains the sole owner-generation reset. The resident counter records
monthly target identity and marker provenance; it does not fence an earlier driver or reset tool state.
Window close and owner retirement use their existing monitor, metering, filesystem and cleanup joins.
There is no stop-task vector, release successor, admission seal or publication transaction. Settlement
and permit release do not wait on a generic signal-publication phase. A real entered effect or
filesystem drain can still delay their ordinary cleanup owner.

Colocated tests exercise the real queue, establishment predecessor, ready-permit cancellation,
EntityFence drainage, window close and monitor-health operations. Tests for frozen publication,
leader/follower progress and seal/successor ordering retire with those producers. Native tests retain
effects, invocation identity, committed prefixes, exact results and terminals, and physical cleanup.
Their receipt comes from ordinary `ExecutionStatus::Interrupting`, observed in test-side support
while provider or oplog work remains held. No private publication receipt remains on Worker.
Monthly acceptance has no test observation event or collector. Colocated tests exercise
the actual owner/window guard and acceptance result. Native tests apply capacity changes
and check execution status, typed outcomes, accounting, effects and committed history;
they do not infer an exact rejection or private window identity from missing events.

Real pending outcome appends and account-capacity contention exercise teardown without
a private pre-fence pause. Completed-history replay tests hold the exact stored invocation
input at the existing oplog dependency. Its Started/Finished pair and uncached payload
reference come from committed history; no replay flag or cursor is changed. A dependency
hold is not a guest-instruction or native-syscall Pending observation.

Tail tests withhold actual HTTP response bytes and check incomplete chunks, interruption,
accounting, committed prefixes and recovery. No root-completion or resource-producer
observation replaces the deleted instrumentation. Exact active-task counts and private
root/settlement ordering are not native integration guarantees. See
[root completion and live tail ownership](reference/testing-patterns.md#root-completion-and-live-tail-ownership).

An interrupt during initial `WaitingForPermit` queues the ordinary request and signals Loading
subscribers without cancelling the waiting task. Once permits arrive, the invocation loop handles
the request before creating a Store; its receipt does not promise completion before permit grant.
Restart and Jump retain the same start attempt. Deletion and owner retirement still drop the
waiting worker and abort its task. Initial waiting has no monthly execution window; monthly
acceptance requires a Running owner with a held permit. Warm permit reacquisition remains
interruptible in the invocation loop.
If core initialization stops before a prepared Store exists, the invocation-loop-owned Worker
stop acknowledges the published interruption under the lifecycle lock. No Store callback remains
to send that receipt. The real initializer tests in `tests/api/interruption/lifecycle/admission.rs` verify
cleanup and one receipt, durable Suspend and reconstruction, and an ephemeral typed resource
error recorded as Recovery before any guest invocation starts.

The monitor submits a proposal to the existing Worker state actor. Acceptance revalidates the cached
owner, startup attempt, held permit, exact active window, policy status and revision, period, fuel
generation and current exhaustion. The private Worker slot and exact window identity reject targets
from replaced executions.
It queues the same ordinary interruption
and advances the establishment tail before reporting acceptance. Capacity and request-state locks
are released before asynchronous establishment. The invocation loop remains the sole primary-Store
owner. Entity failure selection stays with the existing operation owner; a competing trap or
infrastructure failure does not become a quota error or a fabricated stop-election failure.

For the outer-Running acceptance guard, separate body/native claims and applied capacity versus
physical filesystem completion, read [monthly acceptance proofs](reference/testing-patterns.md#monthly-acceptance-guard-and-applied-capacity).

Ordinary nonterminal unload closes tool/entity admission and drains physical ownership without
electing a synthetic lifecycle failure. The operation owner retains that closed admission separately
from its first selected Trap, Infrastructure or real Lifecycle cause. A later stop during unloaded
OOM backoff can therefore elect its exact accepted kind without clearing the old physical fence,
cleanup or selected invocation outcome. The operation owner keeps its first selected failure.

Monthly acceptance rejects further proposals once a terminal stop is claimed, under the existing
interrupt lock and capacity transaction. The same active window can keep accounting during retirement, but
later ticks or applied updates cannot replace the accepted monthly kind/reason or queue another
terminal demand. This also preserves a claimed user stop. Generic terminal queuing still permits
later deletion. `tests/api/interruption/host/p2/tcp/input/repeated_stop.rs` holds a real outcome writer after
selection, checks tick and changed-resource update rejection, then verifies one terminal, physical
cleanup, durable continuation and ephemeral archival.

Failure classification uses the ordinary `on_invocation_failure` callback. The invocation boundary
already distinguishes a deadline from a typed lifecycle interruption. Failure recording does not
wait for private stop publication, consume a queued lifecycle request, or replace a new failure
candidate with that request. Monthly ephemeral errors map only the exact matching typed interruption;
an independent guest error, timeout or unmatched interruption keeps its boundary classification.
No separate origin enum or origin-specific callback remains. Cleanup errors remain separate from
an unrelated invocation error.
The invocation deadline uses the same configured Tokio timer with and without `test-utils`. There
is no Worker watch channel or forced-expiry setter. Native selection tests leave TCP input silent,
observe the timer's actual typed interrupt and hold the ordinary outcome append before requesting
a later quota stop. Their other cases let quota or success finish first. They require the exact
five-second timeout text and complete committed raw prefix. Prior validation is historical,
not verification of the instrumentation removal.
Before recording a typed interruption, the ordinary failure callback acknowledges only its exact
matching, already-observed terminal request. A different kind or an unobserved queued request is
left untouched. This prevents later monthly observations from replacing an observed stop without
reintroducing publication waiting or changing a timeout's cause.

The monthly body test applies the ordinary callback's pure monthly mapper while holding the actual
stop guard. A deadline remains unchanged and the queued stop retains its own exact cause.
Admission has no test-only pause. Real Worker selection tests retain actual timer expiry, storage
append backpressure, effect/key/raw-prefix assertions and ordinary reconstruction. Their queued-work
window is not a pre-admission timing claim. The retained startup, health and permit-wait changes have
concrete monthly admission or cleanup dependencies, not a stronger quota/completion ordering rule.

After guest success, the invocation loop uses the ordinary `Interrupts::claim_pending_terminal`
check before success finalization. A queued terminal waits for its own establishment and takes the
existing failure path, as in main. Once that boundary permits success, its recorder
does not recheck or consume a later stop or turn the result into a quota error. Finalization still
drains required durable work and commits `AgentInvocationFinished` before notifying waiters.
Colocated tests exercise the actual terminal check; native callback/persistence tests retain the
effect, key, committed-prefix, terminal, cleanup and reconstruction guarantees.

There is no `SelectedInvocationOutcome`, private writer receipt, writer-loss verdict or Worker result
slot. The ordinary invocation loop and callbacks own result writing. Stop signalling may overlap a
pending terminal append; invocation result notification still follows the existing commit receipt.
Durable success uses `Always`; ephemeral success uses `Deferred` without waiting for status folding.
A refused write publishes no result, and ShardLost retirement still joins physical cleanup before
removing the exact old generation. No replacement result arbiter or writer registry is installed.

Invocation exit reads FIFO-folded status before recording a retry interruption. Only Running or
Retrying still needs a failure callback. Failed, automatically suspended or completed work must not
run that callback again because a later stop arrived. Native guest-failure and deadline cases
require exact single invocation outcomes, not another quota error. A later unloaded stop can append
its ordinary Suspend hint after the automatic callback's Suspend; it does not run that callback or
replace an invocation result. A stop already consumed at invocation exit does not become a second
unloaded demand; the pending-Suspend append case still checks one hint and one receipt.

`tests/api/fenced_outcome.rs` keeps its exact refused-prefix, physical-release, removal and regrant
checks through the real oplog dependency. Selection and host completion cases retain result/effect/
key/history and ordinary reconstruction, but no longer require raw stop signalling to wait for the
callback's commit. The idle-after-success observer and private pending/committed-writer projections
are removed with their protocol. Setup uses actual invocation completion and execution-permit release.
The general stop protocol is removed. Monthly
monitoring uses ordinary timers; test-side observations do not own result or cleanup transitions.

The default 10 ms epoch increment still does not wake pending host futures. Real silent-TCP tests
cover monthly-memory detection by both the local tick and applied Registry update, cooperative
socket closure, durable reconstruction and ephemeral resource failure. The P2 TCP input
`subscribe().block()` tests in `tests/api/interruption/host/p2/tcp/poll.rs` cover memory, prepaid compute and
mode-matching managed-XFS storage in both modes. They verify physical unload and permit release
before peer readiness, then repair the original poll Start for durable agents. Storage cases write
and sync a real allocation and run in the privileged managed-XFS suite. These tests do not establish a
bound for other host waits; preserve the permit-owned billing window.

P2 TCP `start_connect` re-executes natively on reconstruction. A completed connect poll must drive
the new native connect future before returning its recorded readiness, otherwise synchronous
`finish_connect` can return `WouldBlock` and diverge. `TcpConnectReplay` in
`durable_host/sockets/tcp.rs` classifies only socket pollables with an unfinished connect.
`durable_host/io/poll.rs` revalidates their recorded-ready `poll` or `ready` result without changing
the oplog. Only the native TCP readiness wait selects the existing typed interrupt signal, after
completed replay has closed the durable handle. Replay resolution, the recorded terminal and
file-readiness revalidation stay outside that race. `NativeIoWait` uses the same `InterruptSources`
subscription as `DurableWorkerCtx::create_interrupt_signal`: it subscribes before checking invocation
and tail-work deadlines and settled owner failure, then also observes entity cancellation. A deadline
can leave `ExecutionStatus` Running, so its latch is required for a late subscriber.
`tests/api/interruption/host/p2/tcp/poll.rs` keeps the TCP peer silent after connect
reconstruction, then applies a second monthly stop. It retains the original connect poll and ready
terminals and incomplete input Start, checks observable cleanup and accounting,
and recovers the same invocation key. These observations do not prove the native poll state
or private window identity. Its immediate post-close flush
does not establish an elapsed billing interval. The colocated production-window test
`closed_owner_window_does_not_accrue_over_known_metering_intervals` advances the existing MeteringClock
after physical permit release, retains allocated memory and the same owner/meter, and asserts no
memory or mode-specific storage accrual and no filesystem resampling over known intervals.
Peer withholding does not control TCP connect readiness, so
this integrated case is not a connect-timeout witness. Failed native reconnect outcomes remain a raw-P2 replay
limitation; readiness does not guarantee the original successful connection result.

Successful pollable drops clear classification. Native socket deletion requires all pollable
children to be dropped first, so socket cleanup need not scan unrelated subscriptions. This does
not revalidate HTTP, UDP or TCP input pollables. Exported P2 `pollable.ready` remains a nonblocking
live probe, as verified by `tests/api/p2_ready.rs`.

### Primary DB/KV completion before stop

An accepted stop does not cancel already-entered primary DB/KV backend work. A completed success
or final error keeps its required durable completion and cleanup under their existing owners before
guest execution observes the typed stop. Eligible retry-delay interruption remains unchanged. It can leave
`Start` incomplete, without a fabricated `End` or `Cancelled`. Not every entered import must produce
a terminal record. The current call is not the whole guest invocation or open transaction. An
unstarted commit is not required, but an already-entered commit or rollback keeps its required
finalization.

`tests/api/interruption/host/db_kv.rs` witnesses serialized KV set and scripted PostgreSQL execute
in both agent modes, plus an entered durable commit with awaited cleanup. The tests hold the
provider through stop acceptance, then verify its effect and completion before physical release.
Enabled byte-time continues while the concurrent-agent permit is held and stops through its
existing window closure. Durable recovery passes admission and reconstructs the same invocation
key without repeating recorded completed effects. Ephemeral failure remains terminal; a fresh
invocation can use the released capacity.

A provider, transaction finalizer or pre-release cleanup that never finishes can retain the runtime
and permit indefinitely. Later filesystem deletion or repair does not reopen a closed window.
Enabled metering and accounting continue until the existing window closes. Only excess
under the applied `allowOverage` revision is billable. Earlier `hardLimit` usage never becomes billable
retrospectively. These witnesses observe accounting deltas, not overage charges. They do not prove
every SQL/KV operation, pending entity-provider survival, concurrent P3 sibling completion, stop
before every next import, or external exactly-once effects. They assert lifecycle, accounting and
effects, not release-metric attribution.

### Observing physical release

When interpreting stop latency, unfinished cleanup or retained failure, the existing Prometheus
registry reports owner facts; metrics do not own interruption, cleanup, billing or recovery.
Acceptance and signal publication are not physical release. Observe the end of primary execution,
actual Store destruction, reservation return, filesystem drain/deletion and permit Drop separately.
Billing closes before the existing permit release. Store destruction does not prove OS page
reclamation, and a retained MemoryGrant keeps its receipt and end-to-end interval unfinished.

Duration histograms contain only completed endpoints. Unfinished counts and oldest ages retain
pending or failed receipts; collection advances age without another timer, including with billing
disabled. A deadline or dropped observer does not complete a receipt. `released_with_failure`
records physical completion with retained failed health, not successful cleanup. Repair does not
erase failure history. Stage series count receipts, while `end_to_end` counts intervals; do not
sum them as a worker count. The host-exit histogram covers only entered `p3_tcp_receive` drivers,
not DB/KV calls. Histogram boundaries are not release deadlines, and the metrics define no
global quota-detection timestamp or universal release bound.

Metric labels aggregate process-local observations without account or invocation IDs. A shared
process's cause/stage delta alone cannot attribute an event to a particular invocation when other
work can update the same series. Use an existing isolated registry attached to real owners for
attributable owner tests; keep that evidence distinct from real-Worker DB/KV composition.

### Native wait verification

Native waits have no dedicated tracing wrapper, window/permit tracing receipts or
test collector. Keep functional `NativeIoWait` cancellation and the operational
permit-held eviction flag. Existing release Prometheus metrics and invocation-result,
scheduler and entity-fence logging remain; they are not a replacement observation protocol.

Use dependency/peer boundaries, execution APIs, results, oplog records and accounting
for integration tests. An open Start, quiet peer or guest promise does not prove exact
native Pending, resource creation, reader-lock ordering or a private window identity.
Test precise polling and ownership on existing colocated primitives. Preserve main's
preexisting clock gate; do not add instrumentation to recover removed assertions.
Read [native wait verification boundaries](../../../golem-worker-executor/native-wait-tracing.md)
and [native wait observations](reference/testing-patterns.md#native-wait-observations-without-worker-hooks).

### Blobstore and filesystem guest observation

Live blobstore reads in `durable_host/blobstore/{mod,container}.rs` select the composed
`create_interrupt_signal()`. Provider completion has priority when both read branches are ready.
A read interruption abandons only the incomplete durable call and propagates the typed trap.
An entered DefaultBlobStoreService mutation transfers its original environment lock, storage Arc,
quota entry and owned inputs to a service-owned operation. Caller Drop detaches its await rather
than cancelling storage or settlement. The original accounting code settles the known result before
releasing the original lock. Success keeps its measured stock; a returned failure uses the existing
reservation refund. Drop never assumes that a remote effect did not happen. All mutation methods
use this ownership. A cooperative primary host call still awaits the result, retains creation's
required timestamp lookup and performs mandatory End before observing the stop. An entity runner
can cancel its whole body through `await_component_entity_body`; that Start may stay incomplete,
but service work still owns its quota settlement. The service task has no Worker/Store/oplog and
writes no late End. No second lifecycle or reconstruction path is added.

If the operation is lost without a result, the environment owner retains its
actual guard and typed TaskLost failure. Queued mutators are notified; queued and later
calls fail Unavailable before entering storage or quota admission. The host traps this infrastructure
failure before retry/result serialization or End. No credit or unlock is fabricated.
Detached task loss remains observable through mutation_failure. Production uses panic=abort:
a panic terminates the process, with no in-process recovery guarantee.
Healthy inactive owners can be pruned; failed owners keep the explicit obligation. Unknown work
requires operator reconciliation/service replacement. This local guard is not process-loss or
distributed exactly-once protection, and it does not repair partial-backend-error accounting.
A genuinely pending backend stays pending and can delay primary unload indefinitely.

Retry-delay interruption remains unchanged and can leave Start incomplete. Metrics, resource
registration and accessor delivery stay outside the read select. Buffered values and materialized
object names add no backend wait.

P2/P3 filesystem descriptor calls select guest observation of each admitted operation. Open selects
its attributes and open calls separately. Stat interruption escapes as a typed trap before error
serialization; completed stat history and replay timestamp restoration remain authoritative. Direct
P2 file read/skip, flush readiness and completed-record file readiness/read revalidation use the
existing file maps. Blocking splice observes known file outputs with file or TCP inputs, as described
below. Live multi-poll retains its existing Worker subscriber. Unknown stream pairs keep their paths.

P3 file prefetch, subsequent producer demand and result-future observation use the composed signal.
An interrupted read traps instead of reporting EOF. Streaming chunk effects and task completion are
not selected. `FilesystemCall` Drop keeps already-started work and its generation lease under the
filesystem module until completion. Guest observation can therefore stop while flush or write still
runs; unload must drain that work before deletion and actual permit release. Known-local synchronous
syscalls cannot be preempted inside a future poll, and started blocking tasks may delay drain. Cleanup
failures and unknown writers remain observable, not successful release.

`tests/api/interruption/host/blob_filesystem.rs` verifies monthly memory in both agent modes through
a real blob provider read. Its container-exists dependency stays Pending through physical release.
Durable cases retain and complete the original Start under the same invocation key after capacity
returns; ephemeral cases persist the typed monthly failure. The keyed pre-poll filesystem gate and
its synthetic stat cells are removed. They never established Pending of the metadata operation.
The lifecycle test
`interrupted_guest_observation_retains_started_call_and_drain` holds a started scripted flush and
proves deletion waits after typed observation interruption. These are separate observation and
physical-drain witnesses, not a native-XFS stall or universal release bound. Keep that evidence split
instead of building a new filesystem backend or suspension point to recreate the discarded claim.

`services/blob_store.rs` tests explicitly poll the actual provider-await helper and
DefaultBlobStoreService into backend Pending after positive stock reservation. They deliver the
typed stop with the backend still withheld, then check the environment lock, exact successful stock
and returned-failure refunds. This excludes a provider-ready/stop tie. The primary host cases in
`tests/api/interruption/host/blob_mutations/` separately require mandatory write/copy End, joined
physical cleanup and same-key reconstruction without another backend effect. They release the
backend after stop acceptance; the service tests establish the precise cancellation window.

Caller-loss tests compose the actual EntitySlot fence and component cancellation helper with the
real service/backend. They require reservation and lock ownership through cancellation, exact known
success/failure settlement and stored bytes. Panic/task-loss tests require retained guard identity,
no refund and typed rejection of queued/later mutators. Full-host unavailable cases separately require
an incomplete host Start, outer invocation failure and no fabricated backend End or guest Result Err.
Primitive composition is not a complete Wasmtime-entity/owner-oplog cancellation witness.

`tests/tool_streaming/durable_entity_blob.rs` supplies that separate witness through the actual
`ComponentEntityRunner`. It cancels the owner while a reserved write is pending, joins physical
entity/owner cleanup, and requires the original provider, environment lock and quota to survive.
Late service settlement writes no owner-oplog terminal. Ordinary same-key repair can put the same
object/content again without charging its stock twice; completed cold replay performs no new put.
The completion-first control retains normal End records, with no delivery/discard markers for the
host-internal entity root or serialized blob write. Exact raw records,
hydrated payloads and public projection checks are described in
[entity blob mutation after caller loss](reference/testing-patterns.md#entity-blob-mutation-after-caller-loss).
This does not resolve unknown process-loss or partial-backend outcomes into external exactly-once effects.

### Non-network frontend stream observation

P3 captured stdout/stderr chunks and result futures in `durable_host/p3/cli.rs` observe the
composed signal only while awaiting an acknowledgement or task result. A ready acknowledgement
or result wins first. The emitting task still owns log emission, acknowledgements and completion;
consumer Drop still queues its buffered remainder. Standard input remains disabled.

`stream_transport.rs` live input demand selects source receive against the same composed signal.
Interruption traps with `InterruptKind`, not EOF or a stream terminal. Schema wrap/unwrap and the
native-tool byte frontend pass the signal from their owning context. Pending item publication can
stop guest observation, but the consumer keeps its already-consumed publication future for existing
Drop completion. Terminal publication remains mandatory. A saturated primary queue can therefore
keep retained publication pending after the guest stops. Projection keeps its existing relay
lifecycle and Store-closure ownership; a typed durable-source interruption aborts the relay without
publishing an error terminal.

Durable schema and native-tool stdin share `DurableInputProducer::poll_value`. Receive admission
captures the composed signal after replay admission. Only the first live `reader.next()` selects
it. Once a source event wins, packed-batch collection, consumer-journal commit and nested mapping
work finish before delivery. Recorded consumer ordinals and offsets remain authoritative; local
`DurableInputError` keeps interruption separate from `SessionError`. Drop cancellation and
attachment fencing retain their existing owners.

Module tests observe actual quiet receive, bounded publication and CLI acknowledgement/result
Pending. They verify typed interruption, retained consumed output and final buffering, ready-first
behavior, nonidentity projection, and source-selected journal completion through a later stop.
Durable journal reload continues under the same session key without an invented terminal.
The keyed durable-source test observer and registry are removed. Actual receive-body tests own the
first-Pending, ordinal and Drop checks. The native frontend case retains the accepted session,
exact monthly cause, physical release, unchanged prefix and ordinary same-key continuation.
For these separate claims and the test-only result-wait startup boundary, read
[durable-source read proofs](reference/testing-patterns.md#durable-source-reads-without-a-runtime-observer).

Raw P2 TCP `blocking_read` and `blocking_skip` in `durable_host/io/streams.rs` use `NativeIoWait`
with the composed interruption sources around only the native input future. Successful `finish_connect` and
`accept` classify the returned input stream; successful native stream drop removes that rep.
File streams use the separate filesystem observation path above; unknown streams keep their existing behavior. Native stream cancellation,
resource deletion and invocation-result commit remain outside the select.

Raw TCP and reconstructed connect keep their existing unbiased selects. Native-wait tests use
real WASI pollables and streams with independently assembled
production sources. They cover deadline/entity cancellation before subscription and during actual
Pending, typed errors, native completion, and resource ownership. They do not establish whole-Worker
deadline/entity behavior or successful network reconnection. Durable replay resolution stays
outside the native interruption race. P2 HTTP is unsupported and has no stream retry adapter.

These raw TCP calls have **no durable host Start or recorded read/skip result**. At a pending
stop, the connect poll is already complete and the method remains unfinished. Reconstruction
reconnects and re-executes input under the retained invocation key, not a repaired stream Start.
Even completed historical raw input executes again when a fresh invocation reconstructs an
unloaded Worker. Deterministic peer data is required to reproduce the recorded invocation result;
this does not provide exactly-once TCP consumption.

`tests/api/interruption/host/p2/tcp/input/mod.rs` exercises each export with memory, prepaid compute and matching
managed-XFS storage in both modes through a controlled TCP peer and execution APIs.
Tests keep the peer silent through monthly stop, physical unload and permit release. Durable
cases retain 2 Started/1 Finished, then reach 2/2 after reconnect and one-byte input. The separate
completion tests let native input finish, hold the actual selected terminal append, then accept
a monthly stop and join its establishment while the outcome append remains held. Releasing that
append preserves the selected result; notification follows its normal commit receipt. The tests do
not require commit before stop publication. They also reconstruct completed history and count the
repeated native input before fresh work. Other TCP output and
UDP waits are separate operations, not covered by these input tests.

Raw P2 TCP `output.blocking_splice(&input, 1)` is a distinct export whose native provider first
checks output capacity, then waits for input and finally writes and flushes output. Successful
TCP `finish_connect` and `accept` register input and output reps independently; successful stream
drop removes only that stream's classification. When both reps are registered raw TCP streams,
the native splice selects the existing composed `create_interrupt_signal()` through
`interruptible_tcp_input`. Input and output need not share a socket. Filesystem readiness,
unknown outputs, durable settlement and resource deletion stay outside
this select. Raw read/skip use the same composed subscription.

A registered TCP input can also feed a known filesystem output. That pairing uses the existing
filesystem observation helper for output readiness and the native splice. Cancellation stops guest
observation; any entered file operation keeps its filesystem owner and generation lease until
ordinary drain completes. File completion keeps its existing ready-first behavior, while TCP/TCP
selection remains unbiased. Unknown or unclassified pairs keep their existing paths.

The mixed-pair regression uses actual guest TCP and file streams with a silent peer, not a Worker
gate. Both modes reproduced accepted Suspend with the socket, Store and permit still live before
the missing classification was fixed. It retains exact stopped history and the complete committed
prefix in the actual agent mode. Durable continuation and the fresh ephemeral probe must produce
exactly the spliced byte in the file, with matching byte count and file size. The native splice
itself still has no durable Start; surrounding filesystem calls keep their ordinary records. This
does not add transactional byte effects or an external exactly-once guarantee.

`tests/api/interruption/host/p2/tcp/input/mod.rs` and `api/interruption/host/p2/tcp/input/cross_socket/mod.rs`
use real same-socket and cross-socket peers. The precise writable-output/blocked-input
proof belongs to the actual native-body test below, not an inferred internal phase.
Each now has memory, prepaid-compute and matching managed-XFS storage cases in both agent modes.
They check socket closure and joined Store/permit/window cleanup before any input byte, durable
same-key reconnection or ephemeral terminal/probe, and completion-first cached success. The
cross-socket tests require both sockets to close and later count a byte consumed on A and emitted
on B. Memory accounting uses a drained seed and isolated target account interval, not a per-worker
billing key or an exact parked-duration charge. Storage cases use native managed-XFS measurement.

TCP/TCP splice constructs the same `NativeIoWait` as raw read/skip. Colocated tests call the actual
WASI `HostOutputStream::blocking_splice` on separate input/output resources. Writable memory output
and quiet native input establish actual Pending without a Worker gate.
`native_splice_interrupts_actual_pending_without_output_effect` checks actual output capacity in
that test body, then requires no output effect after interruption. The tests publish lifecycle,
invocation/tail-work deadline or entity cancellation before construction or during that Pending,
require the typed trap and no output effect, then delete both real table resources. A completion-first
case transfers exactly one byte before a later stop. Raw selection remains unbiased; no ready/stop
tie priority is claimed. This is native source-to-wait evidence, not whole-Worker deadline/entity
coverage or real socket teardown. Peer-backed matrices check socket closure,
observable cleanup and same-key effects, not native Pending. Their immediate post-close flush is not an elapsed
billing interval; the deterministic closed-owner-window test above proves that separate guarantee.

Like raw read/skip, splice has no durable host Start or recorded byte result. Completed guest
history can consume and emit TCP bytes again; only a same-key result-cache lookup stays offline.
These tests do **not** prove output back-pressure, post-consumption flush cancellation,
transactional byte effects, accepted/half-dropped socket combinations or external exactly-once.

P3 HTTP monthly tests in `tests/api/interruption/host/p3/http/` withhold response headers or the
body suffix at the peer. Recovery retains the original send, consume-body parent and chunk Starts.
It verifies an already-delivered prefix before appending the suffix. There is no synthetic batched
scope or recovery Jump. The assertions require exact output, HTTP key, invocation count, terminals,
delivery markers and the full committed raw prefix. P2 HTTP producers and their dedicated tests
were removed with that unsupported API.

Raw P2/P3 TCP and UDP fixtures explicitly enable `agent_sockets.enabled` in their executor setup.
The production default remains disabled. Host HTTP, WebSocket and RDBMS clients do not depend on
that setting. A fixture without this opt-in can return access-denied before reaching the intended
native wait. Check the actual error rather than assuming the intended wait was reached.

P3 UDP `receive` in `durable_host/p3/sockets/udp/mod.rs` selects the same typed signal only
around the native live receive. `run_read_access` leaves an interrupted
`P3SocketsTypesUdpSocketReceive` Start incomplete; its call-owned trap context preserves the
`InterruptKind` root cause. Replay resolution, End persistence, accessor delivery and native
bind/drop remain outside the select. The call remains `ReadRemote` and repairs its original Start.

UDP tests use fixture-controlled endpoints, results, oplog records and accounting.
A receive Start does not prove that a socket was created, bound or polled Pending.
Port reuse requires an independently known endpoint; UDP has no EOF. Completion-first
controls verify End and delivery, then preserve the selected invocation result through
commit and notification. These tests do not establish exactly-once datagram consumption
across the receive-before-End crash window. Connected UDP reconstruction and incomplete
batched sends remain separate, unresolved contracts.

WebSocket initial `connect` in `durable_host/websocket/client.rs` opens its `WriteRemote`
Start before acquiring the executor-wide connection-pool permit. The live pool acquisition
selects the existing typed interrupt signal without a Store lock. An interrupted acquisition
abandons the call for a trap, leaving its Start incomplete; it does not append `Cancelled`.
Only successful acquisition transfers the permit to the live socket. The subsequent handshake
has its own interrupt select; neither call completion nor resource-table registration is raced
with this pool wait. `tests/api/interruption/host/websocket/pool.rs` holds the sole pool slot with a
completed, loaded-idle Worker whose monthly permit and monitor have been released. A second
Worker parks on initial connect with no peer connection or receive Start. Six resource/mode
cases stop and physically unload that target before evicting the holder. Durable reconstruction
repairs the original connect Start and invocation key after the slot is freed; ephemeral failure
remains terminal. Peer handshakes and frames are counted as attempts, not external exactly-once
effects.

The initial `connect_async` handshake begins after a free pool permit is acquired.
`tests/api/interruption/host/websocket/handshake.rs` uses an in-process TCP peer
that reads the Upgrade request and holds back the response. Resource/mode cases stop the Worker,
close the half-open socket and return both permits while 101 remains withheld. Durable replay
repairs the original Connect Start with a new handshake; ephemeral failure is typed and a fresh
Worker probes the released pool. A completion-first control lets 101 arrive before exhaustion.
The handshake already selected the typed signal; these tests did not expose a production RED.

Completed durable Connect replay instead restores a `Replay` connection handle without a TCP
connection. A fresh Receive or Close opens its own Start, then acquires a pool slot before
reconnecting through the shared accessor helper. Store access ends before reconnect coordination,
permit acquisition and handshake waits. `tests/api/interruption/host/websocket/reconnect_pool.rs`
holds the only slot with another loaded-idle Worker. This establishes pool contention,
not an observed Pending poll. Both operations select the composed typed signal. The holder stays loaded until the
stopped target physically unloads. After holder eviction and capacity grant, each original Start
gains End then CompletionDelivered, with one new handshake and no historical replay traffic. `tests/api/interruption/host/websocket/reconnect_handshake.rs` instead starts with the pool free
and withholds the third TCP Upgrade response after receiving the reconnect request.
The typed stop closes that socket and frees the slot before the peer supplies
101; recovery repairs the same operation Start on a fourth TCP connection. Receive and Close
have durable memory, prepaid compute and managed-XFS durable-storage cases and completion-first
controls for each wait. Both post-permit handshakes use the composed interrupt signal.
Completed-connect replay is not available to ephemeral agents, which cannot resume or invoke a
reconstructed incarnation. Entity-local cancellation, a native ready/stop tie, timed receive,
reader/writer locks and send/close transport back-pressure remain unproved by these matrices.

A timed WebSocket receive is another distinct frame read: `live.reader.lock().await` completes
before `reader.next()` is wrapped by the timeout and raced against the composed typed signal.
`tests/api/interruption/host/websocket/timed_receive.rs` withholds the peer frame after a
valid Upgrade and checks the timed Receive Start and invocation result. This does not
identify an exact frame-future poll. The guest's deadline remains an ordinary guest timer. The
resource/mode cases check stop and physical socket/Store/permit/window release with the frame
still withheld, durable same-key repair of the original timed Start and ephemeral typed failure
with a fresh Worker probe. A separate one-second timeout-first control returns `None` before
any monthly exhaustion, then preserves its cached result. The existing frame select already
interrupted; no production RED or timeout/stop tie is claimed. The reader mutex acquisition
**precedes** that select, so these tests do not cover lock contention. Storage cases run against
real allocation measurement in the privileged managed-XFS suite.

A second timed receive on the **same** guest WebSocket can reach the reader mutex while an
untimed receive holds it in its native frame read. `tests/api/interruption/host/websocket/reader_lock.rs`
uses a promise created by that same guest invocation before either receive. The timed branch
awaits it before entering receive-with-timeout. The test verifies the committed create
response and completes that promise through the normal API. No Worker ordering gate or
observer remains. Guest import, promise completion or Start order does not establish
lock ownership or a Pending lock poll. Resource/mode cases
exercise **stop-only** physical cleanup with both frames withheld. The first
interruptible read releases its guard on an owner monthly stop, so the missing select around
the second lock await has no demonstrated normal-Worker RED. The timed deadline begins
after lock acquisition in the implementation; native tests no longer observe that private
ordering. An earlier durable two-read continuation failed
because replay accessors independently reconnected one saved handle through a one-slot pool.
The per-handle reconnect gate now serializes reconstruction: queued accessors recheck Live or
Terminal and do not acquire a second permit. Ordinary interruption/reconstruction tests in
`tests/websocket.rs` cover this fix. The monthly reader-lock cases remain stop-only and do not
prove same-key two-read continuation after monthly suspension. Entity-local cancellation and
the writer mutex are distinct.

### Resident lifecycle unit and identities

The primary Store and every current entity Store are one resident lifecycle unit. The ordinary
establishment task begins the matching owner's entity fence, signals the primary Store and drains
fenced entity bodies. Invocation-loop teardown uses the same entity owner before physical unload.
Startup waits for the current establishment tail and checks the idle request state before reopening
entity admission. The wallet installer owns the one tool/replay-generation reset. Restart and suspend
discard resident Stores; reconstruction recreates entities from the owner's oplog rather than carrying
their resident state forward. No generic stop-progress registry or frozen-cause election remains.

Three identities at this boundary are deliberately different:

- `AgentFingerprint` is the persistent `Create` identity and namespaces durable derived state.
- The cached `Arc<Worker>` identifies the concrete owner shell in `ActiveAgents`; stale shell A
  can remove or fence only itself, never replacement B cached under the same `AgentId`.
- `ActiveAgent::entity_fence_generation` identifies a resident entity-admission epoch within that
  shell. Reopening admission succeeds only for the generation that was fenced.

An oplog shutdown handle is a fourth, narrower capability: it targets the captured open-oplog
generation. It is not a worker identity or entity-fence generation, and ordinary worker retirement
may reuse the same open oplog. Executor shutdown fences every registered oplog generation before
cancelling executor-owned task execution, abandons transient entity work without semantic
finalization, joins Store destruction and retained callbacks, then joins those captured oplog
layers. These distinctions prevent an old owner from removing, reopening, or writing lifecycle
state for its replacement without adding a second recovery path.

The production shutdown token closes construction admission, fences oplogs and cancels worker
tasks through the `InvocationLoops` watcher before gRPC stops. `server.rs` waits for tracked
shutdown work within `SHUTDOWN_GRACE`, then stops the server join set. This interval can therefore
refuse requests even though the transport remains open. `wait_for_exit()` also stops worker roots
and joins cleanup; it is an active drain, not a passive wait. Concurrent drain callers serialize,
and a failed drain keeps registrations for retry.

On executor shutdown, the active-agent cache task removes its cached workers regardless of TTL.
Each worker owns the service graph that owns the cache, so leaving the entries cached would
retain the graph and its storage pools after task cancellation. This drops cache references only;
it does not record an interruption or delete persisted agents. In-process replacement still waits
for the previous executor's invocation loops and oplog generations to drain.

Explicit interruption retires the cached owner and fences its replacement startup. Automatic
shard-assignment recovery leaves `Interrupted` workers stopped, even with queued invocations or
updates; an executor restart or shard move is not a request to resume them. If the worker
has already unloaded (for example during OOM backoff), the retiring owner must commit an unclaimed
terminal interrupt and notify invocation waiters before removal; no Store remains to do it. A
terminal interrupt already claimed by the invocation loop is not recorded again, and a completed
or failed invocation is not overwritten. A claim retains its exact request, so teardown cannot
requeue that same cause. The ordinary callback owns an automatic terminal, even when a later stop
signals while its append is pending. No driver captures, joins or claims a private result writer.
FIFO-folded status prevents invocation exit from recording that outcome a second time. Permit-wait
and pre-owner-generation stops retain ordinary startup finalization and acknowledgement; they
cannot reuse a previous owner's marker just because the shared counter is unchanged.
A distinct terminal following a completed Restart keeps its own queue
demand while the original publication stays Restart. Normal loop stop or joined owner retirement
commits the unclaimed status marker once, including an outer-loop concurrent-permit wait with no
new Store or permit. An unloaded Suspend or Interrupt writes its ordinary lifecycle hint without
deduplicating against an earlier Suspend. A later pre-owner attempt also writes its own hint even
if the resident counter is unchanged. These hints do not replace committed invocation results or
rewrite the earlier prefix. See [the Suspend timeline](reference/timelines.md#10a-unloaded-stops-write-ordinary-lifecycle-hints).
Loop stop never claims a terminal merely to discard it. Tests:
`tests/scalability.rs::interrupt_during_oom_backoff_is_durable_before_restart` and
`tests/resource_limits/mod.rs::concurrent_agent_limit_restarted_waiter_stops_without_permit`.

`recover_immediately` selects `Restart` for Running, Suspended and Retrying workers. It never
turns a simulated crash of a parked worker into a permanent interruption. If no invocation loop
remains, the existing promise, scheduler or permit wakeup starts reconstruction; the queued
restart does not fail the invocation waiter or append `Interrupted`.

When a trap chooses `RetryDecision::Delayed`, the invocation loop snapshots the finite command
prefix already queued before teardown. Duplicate `WorkAvailable` edges in that prefix collapse to
one resident-work hint; commands arriving after the snapshot remain queued and can shorten the
delay. This preserves lifecycle-control semantics without treating every edge notification as
independent work. Replay always runs to the oplog tail during instance preparation; there is no
paused replay and no command that resumes one.

Environment and application deletion invalidate component metadata, environment state and agent
type caches before awaiting owner retirement. New metadata lookups then observe deletion instead
of admitting requests against a retiring cached owner.

External metadata observation uses a fallible FIFO status read that repairs a detached projection
before returning it. Unreadable history returns its cause rather than a stale status; once history
reads again, the next observation reconstructs the status without starting the worker. Cleanup
keeps its non-repairing FIFO read so an unreadable oplog cannot prevent runtime teardown. If the
actor has stopped, the lookup takes the cold oplog lifecycle guard, verifies retirement, joins
writer completion, and reconstructs from persisted metadata and the oplog. A failed deletion can
therefore leave a cached but stopped worker observable until removal is retried. Missing storage
means absence; failed reconstruction means an error, never a stale cached status or permission to
restart work.

Ephemeral response leases delay only normal archival, not Store unloading or explicit retirement.
The shared gRPC owner lookup acquires the lease before reading session metadata or accepting work.
If normal archival already fenced the owner, lookup joins archival through cache removal, then
resolves an observation-only owner from storage. Archive failure or cancellation rejects the lookup;
it never grants access to the old poisoned producer or restarts the ephemeral invocation.

Normal archival and explicit interruption share `Worker::quiesce_for_owner_retirement` under
the owner-cleanup lock: stop execution, drain stream retirement and lifecycle/forwarding work,
commit, then stop status writers. Only explicit interruption records a pending terminal interrupt.
Archival moves the oplog before removing the cached worker. The open oplog generation and its
forwarding wrapper may be reused by the next worker, so ordinary retirement does not close their
task admission or forwarding. Deletion claims ownership under the same owner-cleanup lock, then
drains the resident stream producer before running maintenance on a private producer. Failed
maintenance is drained before a retry; only deletion closes the oplog generation permanently.
Archive transfers append and verify the destination before dropping the source. Archive storage
failures therefore fail only that maintenance attempt: they are logged, never fail the agent,
the authoritative source remains intact, and threshold, scheduled or sweep maintenance retries
with per-agent backoff. A scheduled retry remains valid when later commits advance the oplog tip.
An oplog read that does not give the requested entries (an archive chunk that does not decode, a
blob or indexed storage failure of an archive level, a gap) is never treated as absent data. It
returns `UnreadableOplog` (`services/oplog/reader.rs`: the agent id and the `OplogReadError` with
its source, such as `archive layer 1`), and the read paths of replay, status folding and recovery
carry it up. The read error is classified: a storage that did not answer after its retries
(`OplogReadError::SourceUnavailable`, from `OplogArchiveError::Unavailable`: an indexed storage
`Transient` or `Indeterminate` error, or a blob error that `BlobFailure` calls transient or
refused) becomes `RecoveryRequired`, which the start and the invocation retry with backoff and
shard-assignment recovery retries by failing the assignment. Corrupt data, a gap, an invalid range
or any other storage error becomes `WorkerExecutorError::OplogFailed`. Every rule below that names
`OplogFailed` is about that terminal kind; the same read through storage that did not answer
gives `RecoveryRequired` instead. `OplogFailed` fails that agent only, fail-stop: the start ends
through `StartProblem::OplogFailed`, which passes the error in every column of the
outcome table (no skipped or rejected record, no failed update), and the recovery path records it
as a terminal `Error` entry, so the agent is `Failed` with the cause. Shard-assignment recovery
(`get_running_workers_in_shards`, `recovered_status`, `recovered_restart`) logs and skips an agent
whose oplog failed instead of failing the assignment, so the other agents recover. Nothing deletes
or rewrites the unreadable data. The `try_*` reads of `Oplog` and `OplogService` stay the
maintenance reads: they return every storage failure as text for the transfer and the sweep to
retry.

Opening an oplog can fail (`OplogService::open`/`create`, `OplogConstructor::create_oplog`): an
archive level whose last index does not read, or whose owning-epoch record fails after its
retries, fails the open, and the open-oplog cache keeps nothing. The open fails with
`OplogFailed`, or with `RecoveryRequired` when the storage did not answer. A stream session index
rebuilt over an unreadable oplog returns `SessionError::Unreadable`, which keeps the
`UnreadableOplog` up to the boundary, where it becomes `OplogFailed` or `RecoveryRequired` by its
kind. An invocation session attachment records a failed session terminal (`fail_protocol`) only
for a violation of the session protocol (`grpc/invocation_session/mod.rs::end_attachment_on_failure`).
When a read of the session history meets storage that did not answer (including a stream payload
download), the attachment ends without a terminal frame, and the client reattaches from its
durable cursors. A fence or an oplog that failed for good goes out as its worker error and
records nothing. An ephemeral oplog whose writer task cannot store a batch in the first
archive layer latches `OplogError::ArchiveWrite` like the fence: every later add and commit is
refused, so the agent stops. Every caller that handles a fence handles both as `OplogRefusal`
(`services/oplog/mod.rs`): the status actor answers its receipts with the refusal, and a write
that cannot report it (`Worker::add_to_oplog_or_retire`, the log policy, the oplog processor
checkpoint) leaves it to the next write that can. Only the fence records a retirement: a failed
terminal entry of an interrupt that is not a fence fails the waiters with its cause, not as a lost
shard. A commit whose status the status task must rebuild from an oplog that does not read (when
the incremental fold of the commit fails) is refused with
`OplogRefusal::Unreadable`, so the start fails with `OplogFailed` (or `RecoveryRequired` when the
storage did not answer); unlike the fence and the archive
write, it is not latched on the oplog, and the caller that receives it fails the agent. The
status stays detached until a later fold or rebuild succeeds. A caller that took the early receipt
of `commit_oplog_before_status_update` does not wait for the refusal, so the owner finds it
itself: the invocation loop chooses work only from `Worker::get_repaired_status`, which rebuilds a
detached status first, and a rebuild that fails again is recorded like a failed recovery and
ends in infrastructure recovery or in a stop with `OplogFailed` (`unreadable_oplog_outcome`). A
pending invocation whose entry or payload does not hydrate takes the same path:
`hydrate_pending_invocation` classifies the failure as `RecoveryRequired` or `OplogFailed`, and the
loop does not retry it immediately.
Status rebuilds also classify retained `StreamSession` payload reads through
`OplogServiceOps::download_payload`: a backend failure gives `RecoveryRequired`, while
a missing reference or undecodable payload gives `OplogFailed` for that agent, with the entry
index and cause. Reverted payloads do not participate in the rebuild. Regions that invalidate
even the empty status baseline are corrupt history; they do not mean the worker is absent.
`AppendInvocationIfVersion` also rebuilds a detached status before it checks the key. A detached
status can still name an invocation whose completion was committed and published as current, so
the hydrated result cache keeps the keys of committed completions that the status has not folded
yet, apart from the evictable payloads, and the terminal stop (`fail_pending_invocations`) does not
fail those keys. When
`on_invocation_failure` cannot record the failure of an invocation, the refusal takes the
classification of its agent error (`terminal_entry_refusal` in `durable_host/mod.rs`). An oplog
that failed for good fails the invocation with its cause. A storage that did not answer returns
`InvocationFailureDecision::RecoveryRequired`: the invocation stays pending, and the agent goes to
infrastructure recovery. When a latched archive write keeps the terminal entry of an ephemeral
invocation out of the oplog, the terminal stop gives the waiters `OplogFailed` instead of the
ordinary end of an ephemeral agent. Any other `OplogError` on those paths stays
fail-stop. Entries committed with `CommitLevel::Deferred` before the failure
were already acknowledged and are missing from the archived history; an ephemeral agent is never
resumed from that history, so the agent stops instead of the process. The storage failures of the
primary oplog are outside this rule and still stop the executor.
A write the shard-epoch fence refuses is not a storage failure: it ends the transfer as
`OplogError::Fenced`, latches the oplog's fence and is never retried, because the oplog now
belongs to the shard's new owner (see "Resharding, revocation and the oplog epoch fence").

Producer-targeted attachment controls (consumer-side finalization, activation, refresh) reach the
producer's executor through `Rpc::control_durable_stream_attachment`, which returns
`DurableStreamRemoteError` like slot reads. A producer that is recovering, fenced, or being deleted
answers `Unavailable`; the caller retries with backoff instead of failing. A producer that no longer
exists answers not-found, and consumer-side finalization treats that as success because the
producer's own deletion cascade already dropped the attachment. Consumer deletion bounds each
producer finalization with a timeout so a producer that never becomes reachable fails the deletion
attempt instead of hanging it; the attempt can be retried. Producer deletion never waits for
consumer cooperation.

Cold acquisition reserves one unresolved `Worker` in `ActiveAgents`. `initialize_with` owns one
shared attempt independently of request cancellation. `finish_construction` prepares resolved data
privately; failure drains and joins attempt-owned work before returning to `Unresolved`, without
deleting persisted data. Existing waiters receive that attempt's error; later explicit demand
retries by reloading persisted identity and pending initialization. Local success publishes the
resolved data and `Unloaded` state. Remote topology recovery and dependent finished-session recovery
run together in the post-publication reconciler, preserving the deletion gate: attachment RPCs can
acquire mutually referring cold workers on different executors, so awaiting them before publication
would create a cycle. Local readiness does not authorize a merely prepared stream attachment. The
reconciler folds only through the committed `last_known_status.oplog_idx`. Once its topology cache
has no unfinished recovery, it parks on committed stream-state notifications even while attachments
are active. First producer load wakes a worker with no prior stream history; committed terminals
wake deferred session completion after the status fold. Only failed or unfinished recovery arms a
retry deadline. Stale producer attachments are reconciled lazily, including before deletion decides
which dependencies remain. A source read repairs a missing producer activation only after checking
the exact committed Active consumer topology; healthy reads do not enter the mutation lane.
Each reconciler uses a child of the executor shutdown token, so graph shutdown stops new recovery
passes and wakes parked reconcilers. Explicit owner retirement, revert, and
deletion cancel and join the reconciler's in-flight pass; TTL cache retirement does not itself cancel
or join the reconciler.
Tests: `tests/worker_initialization.rs` exercises shared failure, real actor completion, cancellation,
existing-only acquisition, reciprocal cold topologies, and reconciler graph shutdown.

Lifecycle operations acquire the cached or persisted `Worker` through an existing-only path, so
interrupt, delete, resume, update, revert, and plugin changes never create an absent agent.
`ensure_existing_with_fingerprint` resolves identity and metadata, not execution readiness. A resolved
Worker with failed cleanup remains accessible to ordinary public Delete so the owning filesystem
generation can repair disposal. Startup and readiness still reject that retained failure. Delete
is owned by that worker: concurrent callers share its retained attempt result, a later call retries
only unfinished cleanup stages after failure, and successful cleanup retires active-worker and
open-oplog cache entries only for the generation being deleted. A stale `Arc<Worker>` therefore
cannot continue deletion against, or evict cache state belonging to, a replacement with the same
`AgentId`. The `DurableStateRemoved` stage calls `WorkerService::remove` with a closure that
requests `delete_all_snapshots` for the deleting incarnation right after the oplog delete, also when
the stored identity is gone or belongs to another incarnation. A fenced delete requests nothing.
The store's `delete_all` waits for the store work of the incarnation that began before it. A
delete of `delete_all` that got no answer can still land after it answered, so nothing writes a
scope after a delete of all its snapshots; each incarnation and each fork stage has its own
fingerprint, so nothing needs to.

The bounded unload result and final cleanup completion are separate facts. An unload timeout
permanently fails that deletion attempt, while module-owned cleanup continues. A later explicit
delete joins the retained completion, or retries a failed filesystem deletion through the owning
filesystem generation. Durable storage and cache authority remain fenced until verified cleanup
succeeds; old attempt handles retain their original error. Cleanup with no verifiable
owning-component repair remains a failure: successful filesystem deletion cannot erase an
unverified metering settlement, including `ObserverLost` during startup rollback.

For a resident owner, retained module-owned unload destroys the primary runtime, drains the
filesystem, settles final metering and releases the scheduler permit, then waits for the current
`OwnerToolOperations` registry to become empty before deleting the filesystem and reporting
successful unload. `wait_owner_settled` observes registry settlement; it does not join every
supervisor. Entity Store destruction and lane drainage remain separate, necessary barriers.
`begin_generation` still rejects active operations rather than clearing them. Do not put this wait
in `EntityFence::drain`: its caller can hold an operation and would wait on itself. The existing
unload deadline observer may report `CleanupFailed` while retained final cleanup keeps waiting;
an early metering error can also report before this wait. Neither error permits successful
reconstruction. Startup cleanup without a resident owner skips the registry barrier.

Before awaiting startup filesystem cleanup, drop any filesystem context, worker context or Store
that owns open nodes. Cleanup drains those nodes; retaining one across the drain waits on itself.

Create, open, archival, fork-source reads, and deletion share the logical oplog's exclusive cold
lifecycle guard. Fork reads persisted source history without constructing an absent source;
the complete hidden stage is published atomically into an absent target under its lifecycle guard.
Source and target guards are never held together, and publication releases the target guard before
resuming the child. Before the publication, a `ForkCopy` ticket copies the filesystem snapshots of
the source into the repository of the stage (`AgentSnapshots::agent(target, stage_id)`), and
`Copied::publish` gives the `StagePublication` that `OplogService::publish_staged` requires.
Attempts of one request share one copy. The fork cancels every update that the copied prefix
leaves open: one `FailedUpdate` per pending update in queue order, then one per manual-update
invocation that no `PendingUpdate` paired, with that invocation as its attempt index. The baseline of a fork is the record of the last
successful snapshot-based or snapshot-assisted update in the copied prefix. A fork refuses a
missing baseline after it reconciled; an export fork checks the source first. With filesystem snapshots disabled, the copy
copies nothing, so a baseline that names a filesystem snapshot is missing, and the fork is refused
the same way unless the reconciliation finds the target. A copy catches up with the source in rounds,
and each round copies its packs before its index files, so a pack that a prune removed ends the
copy before an index file that names it lands. Cancellation or failure may leave an unreachable
hidden stage; cleanup removes that stage's index and its filesystem snapshots, and never target
payloads or canonical state. Archival is routed through the
existing worker owner. A scheduled archive releases the
guard once its transfer is queued, while the oplog sweep holds it until the transfer finishes. No
lifecycle lock is taken for individual stream items, oplog reads, or replay steps.

`Oplog::stop_and_wait` closes admission and joins work associated with the actual open oplog
generation, including tasks belonging to older worker shells removed from the active cache.
Transport roots are cancelled and their children joined without waiting for client IO. Invocation
loops on this ordinary oplog-stop path are joined, not cancelled: their final commits, state
destruction, and panic cleanup must finish. Executor shutdown is different: it first fences the
captured oplog generations, then cancels executor-owned invocation and entity tasks as described
above, and joins their destruction before closing those generations.
Already-spawned metadata loads and attachment queries finish independently, so a suspended Store
cannot retain their locks; registration occurs once per spawned task, never on cached no-spawn
queries. Attachment queries may spawn every time. Worker-state actors register once at construction
and drain lifecycle jobs before status jobs and the status flusher, including on ordinary worker
drop. Final retirement also joins oplog actors, payload uploads, archive transfers, and monitors.
An error is reported only after all owned work finishes. Deletion records that completion separately
so an explicit retry can remove storage without reusing a retained stop error; the original attempt
keeps its error. A later cold acquisition reloads persisted state rather than inheriting a stopped
generation's error.

A failure while creating or preparing the instance is durable health state, not only a resident-worker
error. The invocation loop commits `Error { kind: Recovery, .. }` before unloading and preserves the
underlying classification. Infrastructure failures do not advance the agent's semantic retry policy:
they remain `Retrying` without a limit and retry on the next demand (invoke, resume, scheduler
activation, or shard reassignment), rather than keeping an executor resident for a scheduled retry.
Invalid components, exports, snapshot baselines, replay divergence, and other permanent failures are terminal.
An authoritative manual-update or promoted snapshot-assisted baseline that cannot be loaded is
terminal, because recovery has no compatible replay fallback. A payload download failure is
terminal for the manual-update baseline too; for the promoted snapshot-assisted baseline it passes
to the recovery path, which retries. Under a pending plain automatic update the baseline load
has the purpose `AutomaticUpdate`: a store failure of the payload retries (`RecoveryRequired`), and
a missing or undecodable payload fails the update with `UPDATE_REPLAY_FAILED`. The
ordinary invocation trap path commits `Error { kind: Invocation, .. }`. The status fold exposes the
kind with the failed/retrying status, so metadata and invocation admission agree after unload or
reassignment. A later startup appends `RecoverySucceeded` only when it fully completes
`prepare_instance` and an unresolved recovery error exists. Routine suspend/recovery writes no
success marker. Structured metadata reports the underlying `Failed`/`Retrying` status and
`last_error_kind: Recovery`; the human CLI table labels terminal recovery failures `Unavailable`.
A queued update still starts a terminally failed worker but does not cosmetically change that
durable health status until recovery succeeds.

Resuming an interrupted **active durable invocation** appends and commits the timestamp-only
`Resumed` hint while the instance lock still proves the worker is unloaded. This happens only
after reading the worker's memory requirement succeeds and before changing the resident state to
`WaitingForPermit`. The status fold therefore changes `Interrupted` back to `Running` immediately,
even if permit admission is still blocked (for example while the interrupted invocation is parked
in a pending p3 wait). `Resumed` does not enqueue an invocation: the existing
`current_idempotency_key` identifies the invocation that reconstruction continues. A `Restart`
does not use this marker and retains its normal `Idle`/automatic-recovery semantics; ephemeral
agents retain their clean fail-stop lifecycle and never append it.

### Resharding, revocation and the oplog epoch fence

Two triggers retire an agent for its lost shard rather than reconstructing it here, and both go
through `Worker::interrupt_and_retire(InterruptKind::ShardLost)`, the one stop-and-drop path an
owner leaves by:

- **Assignment change.** A `RevokeShards` push (`grpc/mod.rs::revoke_shards_internal`), or any
  delivered assignment — an `AssignShards` push, the set returned at registration, or a lease renewal that
  corrects it, all through `apply_shard_assignment_effects` — that no longer holds the agent's
  shard or holds it at a higher epoch than the agent's oplog was opened at (another executor may
  have written to it meanwhile); a late check after construction uses it too. The reason names the
  trigger, not the condition. `apply_shard_assignment_effects` then calls the *other*
  `on_shard_assignment_changed` (`durable_host/mod.rs`, the `WorkerCtx` hook) to recover agents on
  shards held now, the opposite direction from a retirement. An agent retired for an epoch bump is
  reopened on this executor at the new epoch: by that recovery if it was in the running-workers
  index, otherwise by its next invocation. The recovery waits for a live lease.
- **Oplog epoch fence.** A durable agent's primary oplog asserts the epoch it was opened at
  whenever it appends or deletes entries, inside the storage transaction, on every indexed-storage backend (a Lua
  script on Redis, `FOR UPDATE` on Postgres, the single-connection write pool on SQLite, a held
  entry lock in memory; `storage/indexed/*.rs`, surfaced through `services/oplog/primary.rs`). The
  shard manager mints a new, higher epoch for each new owner, including a restarted executor
  process at registration, so a write from an executor that has lost the shard is refused rather
  than written (`OplogError::Fenced` / `OplogFence`, carrying the asserted and, when known, the
  stored epoch). This protects an assignment change this executor has not yet heard about, and a
  revoked lease it is still trying to renew. Once one write is refused the fence *latches*: every
  later add or commit on that handle is refused without a second round trip to storage. A write
  path that meets the refusal under the worker lifecycle lock records the kind in the owner
  retirement synchronously (`Worker::record_retirement`); the stop follows once that lock is
  released, and `stop_internal` hands the generation to `interrupt_and_retire(ShardLost)`, the
  one retirement that fails the waiters and drops it.
- **Archive transfer.** Opening the layered oplog records the owner's epoch on every archive
  level's own key (`services/oplog/fenced_stream.rs::FencedIndexedStream::opened`, used by the
  compressed levels and by the blob layer's manifest) before the archive watermark is read, so
  an older owner's transfer either landed before the record,
  and the watermark covers it, or is refused after it. Each level asserts the epoch on its
  appends, trims and delete-when-empty, and the primary oplog asserts it on the trim that follows
  archiving (the `expected_epoch` of `IndexedStorage::drop_prefix`). A refused step ends the transfer
  (`multilayer.rs::BackgroundTransfer::run`): an append the storage turned away is not verified,
  so the new owner's history does not trip fail-stop validation, and a source whose entries were
  not archived is not trimmed. The refusal latches on the archive handle, `Oplog::fence` reports
  it for the whole layered oplog, and `archive` stops asking for more work. An open refused at the
  primary oplog, or at an ephemeral oplog's first level, records nothing on the remaining levels:
  the handle is finished and never writes them. An emptied level is
  removed with `delete_empty_with_epoch`, which keeps its epoch record: the owner keeps writing
  the level, and an older owner is still refused. A fully archived ephemeral oplog's emptied
  levels keep their records too: removing one would leave nothing that remembers the newest
  owner, and any older handle could claim the level again. Deleting the agent removes the
  records with it, so a transfer still in flight on an older owner cannot write the deleted
  agent's archive back. An ephemeral oplog's writer task latches a refused batch, and the next add or commit fails with
  it; the open-oplog cache replaces an ephemeral handle for an opener at a newer epoch, as it
  replaces a primary one.
- **Blob archive layer.** Blob storage has no conditional write, so a blob level's chunks exist
  only as far as its manifest lists them: a per-agent key in indexed storage
  (`IndexedStorageNamespace::BlobOplogManifest`) holding, per chunk, the object name and entry
  count, fenced like a compressed level (`services/oplog/blob.rs`). An append stores the chunk
  under an object name no other write uses, then appends its manifest entry. The rule for the
  object is that it is deleted only when it is not listed and cannot become listed, which holds
  in two cases. In the first, the storage answers the first attempt of the append that the
  index is already held (`IndexedStorageError::Conflict`, which every backend reports for this
  namespace as it does for a primary oplog insert): nothing was stored and nothing is on its
  way, and the append counts as stored when the listed chunk holds the same entries, as it does
  when a repeated transfer finds the chunk it listed before. In the second, the entry is refused
  and the manifest does not list the object: no entry asserting the old epoch can be stored
  after a refusal. A refused entry whose object the manifest lists keeps it, because an append
  repeated after a lost reply is refused on its second run although its first run stored the
  entry. A manifest append that fails in any other way keeps the object, because the entry may
  still land, even under an index another chunk holds now, once a trim has freed it; that
  includes a held index reported to a repeated attempt, since the attempt before it may have
  been sent. Such an append is checked against the manifest: if this attempt's entry landed, or
  another attempt listed a chunk holding the same entries, it counts as stored, and otherwise it
  is a maintenance failure to retry. On Redis a manifest append is sent once, as a primary oplog
  insert is, so that a lost reply is reported as a failure and not answered by a second run.
  Reads, lengths and scans go through the manifest, never a directory listing. A trim removes
  the manifest entries first and deletes their objects after, and never deletes the agent's
  directory, which is recursive and unfenced. An object no manifest lists is left behind by a
  crash after its upload, by a crash after its trim, and by a manifest append that failed
  without an answer and whose entry never landed; nothing reads it, and deleting the agent
  removes it.

Recording a `ShardLost` retirement cancels `owner_retirement_requested`, so every owner write gate
refuses at once, fences the durable stream producer, and stops the `AgentStatusFlusher` and
`StatusCheckpointer`: those are unfenced key-value writes that belong to the new owner, and a stale
one could overwrite its status or drop the row its crash recovery relies on. The retirement then
stops the agent, drops it from this executor's `ActiveAgents`, and answers its waiters and callers
with `ShardingNotReady` (`Worker::retirement_error`) rather than an in-place restart or a cached
result, so the worker service refreshes its routing table and retries on the owner. Entries still buffered are committed only if
storage still accepts this executor's epoch, so the oplog holds nothing the new owner has not
seen. An invocation still pending in this executor's queue when it gives up is failed the same
way: with a retriable error and no cached result, never with a result the queue happened to
already hold, so a client retry runs it exactly once, on the owner. A deletion refused by the fence
fails with the routing miss and stays cached in `Deleting` with the stages it completed, so a retry
here is refused at entry and repeats no remote effect; the generation is evicted once the shard has
left this executor. Stream cleanup confirms the epoch before it reaches any other agent, because a
re-run (after a restart, or on another executor) finds its fenced records already written. `WorkerService::remove` confirms the epoch (`OplogService::assert_owning_epoch`)
before removing anything, and deletes the oplog after everything it can rebuild. A revert commits
its `Revert` entry with `commit_oplog_and_update_state`, so a refused commit gives `OplogFenced` and
deletes no filesystem snapshot; the caller retries on the new owner. See `crash-matrix.md` for the fence's
failure modes and `services/active_agents/mod.rs` for the sweep that retires agents on an
assignment change.

## Oplog model

Entries are positional or hints (`OplogEntry::is_hint()`). Replay consumes positional entries in
order and skips hints. Key kinds:

- `Start { parent_start_index, observational_owner, request, span_started }` paired with
  `End { start_index, response, span_finished, span_attributes }` or
  `Cancelled { start_index, partial, span_finished }`. A call is identified by
  its `Start` index; `End`/`Cancelled` is its *terminal*. A terminal is *visible* when it lies
  below the replay target and outside an operator `Revert`-skipped region
  (`has_visible_terminal` / `visible_terminal_record`, `replay_state/cursor.rs`). RDBMS transaction
  protocols retain request-less `<scope:transaction>` Start/End pairs. A P3 HTTP response body
  uses its actual `consume-body` call as its parent. Neither creates a generic batched scope;
  nesting uses `parent_start_index`, and invocation ids distinguish concurrent calls.
- `AgentInvocationStarted` / `AgentInvocationFinished` bracket one invocation;
  `PendingAgentInvocation` (hint) is the durable queue entry.
- `CompletionDelivered` / `CompletionDiscarded` (hints, always after their `End`) record the
  guest-observation boundary of an accessor completion.
- `HostStreamFrame` (hint): a frame of a host-owned stream (e.g. a p3 HTTP request body) attached
  to its owning call by `parent_start_index`; consumers find frames by scanning, interrupted
  recordings need no closing entry.
- Operator `Revert` metadata marks deleted history without relocating entries.
- Remote transaction phase entries (`BeginRemoteTransaction`, `PreCommitRemoteTransaction`,
  `PreRollbackRemoteTransaction`, `CommittedRemoteTransaction`, `RolledBackRemoteTransaction`)
  are `hint: true`, not call terminals. The raw begin entry requires `original_begin_index`:
  the owning transaction scope's `Start`. The scope's durable `End` response is
  `HostResponse::GolemRdbmsTransactionTerminal { outcome }`; phase hints do not replace it.
- `PendingUpdate`, `SuccessfulUpdate`, `FailedUpdate`, `Snapshot` (hint).
- Lifecycle hints: `Suspend`, `Error`, `RecoverySucceeded`, `Interrupted`, `Resumed`, `Exited`,
  `Restart`.

Hints are skipped by `skip_forward` (the physical cursor moves past them, but
`last_replayed_non_hint_index` does not), take part in no `Start`/terminal pairing, and never
satisfy a claim. Operator cuts do not relocate retained entries; call identity remains its own `Start`.

Span lifecycle is metadata on ordinary durable operations, not a second positional protocol.
There are no standalone `StartSpan`, `FinishSpan`, or `SetSpanAttribute` entries. Replay first
claims the operation by its invocation id, function name and owner, then the owning host path
restores `SpanStarted` and applies `SpanFinished` or successful attribute updates. Span ids,
timestamps, parents, and links never select a claim, and the generic cursor performs no
span-specific terminal-tail consumption. Long-lived resource and guest spans therefore open on
one operation and close later; a span lifetime never keeps the opening durable call open. An
explicit `span.finish` closes through a short local operation. Releasing a host resource whose
only durable content is its span (`wasm-rpc`, `span`, and HTTP response drops) or an in-memory reservation
(`cache::vacancy` drop) is not a durable call: a garbage-collected guest, or a heap restored from
a snapshot, releases handles at points that differ between runs, so such a release must not take
an invocation id or a replay position. It deletes the table entry and finishes the span in
memory; a live Store with a recorded opening also appends a `SpanClosed` hint naming the opening
`Start`. Replay never claims the hint. HTTP closure also checks whether the send terminal already
closed the span. A drop that terminates a real durable call (an un-awaited RPC/cache future, a
cache fill, an open transaction) keeps its recorded terminal. A replayed drop must not await that
terminal on the guest thread: the guest may drop the handle earlier than recorded, before an
entry only the guest can consume, or in an earlier invocation. A runtime-owned task awaits the
resolution without holding the guest. Only passive sibling-history waits release tail activity;
queued and running cursor work remains active through invocation settlement. It validates the operation's
terminal kind; a cancellation cannot consume a successful result. If the recorded run never
terminated it, continuation waits for the natural replay tail, then performs the live disposal
under the original call. Durable input cancellation likewise waits outside cleanup drains and
producer drop polling. An exact source terminal also satisfies input disposal, including a
terminal supplied by session finalization without a consumer cancellation intent. Inspection
consumes no guest read. Missing cancellation requires the normal published-live transition and
attachment admission; producer mutation admission alone does not authorize replay continuation.
The Store applies pending replay events before forwarding cancellation. Admission failures return
through the cleanup queue with their original trap context. Completed entity replay still rejects
an unrecorded effect.

Disposal admission refusal identifies the persisted session, binding and cancellation role, or
the tool attachment. It overrides a terminal supplied by later owner finalization. Since GC need
not run again, reconstruction also restores the refusal at the recorded owner boundary after
earlier real calls and guest observations have replayed. It neither requires another destructor
callback nor permits a different read or call to satisfy retained history.

Refusal readiness uses `AgentStatusRecord.entity_replay_progress`, folded incrementally with
the status. Completed entity boundaries freeze the terminal and last required entry; open scopes
retain the routes needed to continue a later fold. Replay installs horizon-covering evidence
through the FIFO repaired-status path before exposing its generation or an extended target.
Readiness checks read no oplog entries or payloads: they require the matching boundary, cursor
progress through its watermark, no retained Start in the entity subtree, and acknowledged guest
delivery without a latched failure. Missing evidence is not readiness.

GC timing does not permit a different sequence of consumed reads. An idle stream endpoint may
disappear before or after another host call; an awaited JS read retains its native handle until
that read settles. Do not weaken completion validation to accommodate a test that replaces a
recorded read with an explicit cancellation.

Invocation finalization attempts cleanup before closing the parent, but a cleanup failure does
not skip parent closure or child settlement. Failure election and settlement run inside the Store
driver, retaining their futures across secondary Store-task errors. Settlement waits for election;
the Store still drives terminal continuations that a pending election needs. An already committed
entity terminal is published before retained-resource cleanup, so cleanup cannot wait for its own
terminal selection. Its filesystem lane stays held until cleanup succeeds or owner-failure cleanup
drains it. Classify retained error chains before transport conversion; preserve the original body
failure and its retry metadata over secondary cleanup errors. Shard loss remains authoritative
over later cancellation and an earlier ordinary owner failure. A typed snapshot divergence must
remain recognizable after the old generation's deferred admissions have settled.

`AgentInvocationStarted` records the executing context, including the invocation span added by
the invocation loop, for every invocation kind. The live hook passes that context explicitly to
the oplog writer; it must not derive it from `AgentInvocation::into_parts`, which synthesizes a
fresh context for oplog-processor invocations. Replay restores this recorded stack before guest
execution so HTTP, RPC, and guest span parents resolve to the same ids as in the live execution.

## Durable host call lifecycle

Two-step callers use `DurableCallSession::begin` (returning `BegunCall`) → `BegunCall::resolve` →
`ResolvedCall::{Live, Replay}` (`concurrent/call.rs`). Only the `Live` branch performs
live-only authorization and request preparation before `start_live`; `BegunCall::is_live`
is private so callers cannot choose a branch before resolution. `Replay` consumes the recorded
result or repairs an admitted incomplete call without re-authorizing. Resolution may finish a
guarded replay-tail transition and refresh authority capture. Snapshot calls remain unpersisted;
resolving one as `Live` does not publish Store liveness or lift snapshot restrictions.

Accessor and direct (Store-holding) calls build the same `BegunCall` (`DurableCallSession::new_begun`)
and resolve it through the one `BegunCall::resolve_against_history` (`concurrent/call/claims.rs`).
It claims the recorded `Start`, or continues live at the replay tail. When the replay target grew
while the transition was settling (`BeginReplayToLive::ReplayResumed`), it claims again, so the
call replays the `Start` the grown target covers instead of failing. Both paths append the live
`Start` through `append_start`. A call whose request or identity depends on its own `Start` index
(RPC keys, schedule IDs, runtime card IDs, transfer IDs) builds the request in the
`start_live_at_index` callback; there is no separate begin index.

Every nondeterministic host function goes through `begin_durable_function` /
`end_durable_function` (`durability.rs`). Concurrent (p3 accessor) calls and direct
(Store-holding) calls run inside a `DurableCallSession` (`concurrent/call.rs`) with the same
`Start`/`End` shape.

Live (guest-facing accessor path): append `Start` eagerly → run the live action → append `End` (or
`Cancelled`) → hand the result to the guest → append `CompletionDelivered` (or
`CompletionDiscarded` if the guest dropped the completion unread). The serialized direct path
has no markers: its result is delivered when the host function returns. A trap while a call is
in flight leaves `Start` incomplete (`abandon_for_trap`); a trap never writes `Cancelled`.

**Owned starts.** The guest may cancel a pending accessor call at any point, including during its
start (drop-event drain, boundary synchronization, `Start` append or claim, session construction).
So that the oplog does not depend on where the cancellation found the start, every accessor entry
point (`start_access*`) captures the call's identity in the guest's call and runs the start in an
owned store task (`OwnedAccessStart`) when the accessor belongs to a guest-visible subtask. The
task always appends or claims the `Start`; if the guest has dropped the call by then, it drops the
session, and the drop policy records the call as if the guest had dropped it right after its
start. A live call is prepared in the guest's call, so it counts as in flight from then on. The
task holds a tail-work activity and races the worker's interrupt signal. Request builders run in
the task and receive its accessor. Calls made by host-owned tasks and host-internal calls start
inline. The start never waits on guest progress; authority waits inside request builders wait on
card-authority recovery only.

**Host-internal results** use `DurableCallSession::invoke_host_internal_access`: Start/End
remain durable and identity-validated, but End returns the recorded result directly to the host.
There is no guest-terminal observer, delivery token, delivery/discard marker, or markerless
replay-tail gate. A Store-holding reader must never wait for progress whose only producer needs
that Store's event loop. P3 `monotonic-clock::wait-for` records its outer Start first and owns the
internal `now` read as a child. It resolves the outer replay before the child: a cancelled outer
wait parks without issuing the child, while an incomplete outer wait owns cancellation cleanup
before reconstructing the deadline from recorded `now + duration`. A completed outer requires
a recorded completed child. Cancelling after the child's End settles the outer wait without an
internal delivery marker. Only the outer wait has a real guest-delivery boundary.

A `wait-for` that the guest dropped records `Start` and `Cancelled { partial: None }`, with
whatever prefix of the `now` read the host reached in between (nothing, an open Start, or a closed
one). Its claim sets `AccessClaimOptions::host_internal_calls`; when the claimed Start's
replay-visible terminal is that `Cancelled`, the claim registers the Start as a replay-inert
subtree root (the custom-invocation mechanism), so the recorded `now` read is drained whether or
not the replayed host task reaches it. `invoke_host_internal_access` refuses an owner started
without `host_internal_calls`. Tests: `tests/tool_streaming/dropped_waits.rs`.

P3 timers always return `STARTED` before successful guest delivery, even for zero-duration or
expired waits. Both live execution and replay yield before completing the outer timer. `wait-for`
yields after deriving its deadline from the recorded clock read, so the yield does not restart
the duration. Otherwise an inline live timer could record its delivery before a direct P2 call,
while replay returns `STARTED` and enters that Store-holding call before it can deliver the timer.
The direct call would wait behind a guest boundary that its own Store ownership prevents.

Replay: claim the matching `Start` (`StartClaim`: invocation id, function name and kind),
resolve its terminal through `ConcurrentReplayResolver`, then classify with
`classify_replay_resolution` (`concurrent/call.rs`), which is total and shared by guest-delivery paths:

| Recorded | `ReplayedResolution` | Guest handoff |
|---|---|---|
| `End` + `CompletionDelivered` | `Delivered`, `AtMarker` | released exactly at the recorded marker boundary (`ReplayDeliveryBarrier` holds the cursor gate) |
| `End`, no marker (guest-facing accessor) | `Delivered`, `AtReplayTail` | crash after host completion, before observation: withheld until the cursor drains (`await_natural_tail_end`, `replay_state/resolution.rs`), then delivered live-armed; the effect is **not** re-run |
| `Cancelled { partial: Some }` | `Delivered`, `Immediate` | recorded partial response available without a delivery-marker gate; this is not permission to change guest observations |
| `Cancelled { partial: None }` or `End` + `CompletionDiscarded` | `Undelivered` | no value reached the guest; accessor delivery stays pending, while resource disposal may resolve the recorded terminal in a runtime-owned task without blocking guest progress |
| `Start` without terminal | `Incomplete` | see below |

These classifications describe completion delivery, not deterministic resource-release timing.
GC and snapshot restoration can move an unobserved handle's drop. Deferred cancellation must
preserve its original call identity and terminal kind; a missing terminal permits live disposal
only after the natural replay-to-live transition. A recorded partial response is distinct from
an unread cancellation, and actual guest observations must still be reproduced.

**Custom durable invocations** (`golem:durability/durability.begin-custom-durable-invocation`,
`durability.rs::begin_custom_durable_invocation`) wrap a whole guest block as one logical durable
operation. Completed: the recorded root result is returned and the body does not run (nested
entries are consumed as a replay-inert subtree). Incomplete: the block goes live under its
original root `Start` and the **whole body re-runs**, re-recording nested calls as new physical
`Start`s. This is the one ordinary durable path where a completed nested effect legitimately
repeats; the block author owns its idempotency (`reference/timelines.md` §15,
`tests/durability.rs::custom_durability_crash_mid_live_invocation_reexecutes_whole_body`). A root
custom invocation of an entity body records the entity invocation `Start` as its
`parent_start_index` (and claims with it), like every other durable call of that body, so replay
can tell it from a top-level call of the primary; the custom tree itself (invocation ids, child
initiation) keeps using the custom parent.

**Asynchronous custom completion uses the ordinary delivery protocol.** Rust async combinators
await `begin-custom-durable-invocation-async`, which reserves the identity before yielding but
does not install a live execution scope. The borrowing guest wrapper installs that scope around
each body and finish poll, response conversion, and destruction. Once the body completes, `finish-async`
borrows the live resource and persists `End` through an owned append/commit task. Its
`CompletionDelivery` retains the terminal receipt across cancellation: guest cancellation records
`CompletionDiscarded` only after a successful `End`; Store teardown leaves it markerless and
drains accepted persistence. The SDK retains the resource and scope until finish completes or is
cancelled, destroying any undelivered result before dropping that resource.
Completed replay returns the recorded bytes through `AtMarker` or `AtReplayTail` without running
the body. This reproduces outward completion order, not the skipped body's polls or internal
entries. A completed sibling therefore cannot return early and change a later root call's ordinal
while another sibling remains incomplete. Sync combinators retain the direct begin/finish path.

**Non-Rust SDK limitation:** TypeScript, Effect, Scala, MoonBit, and Go custom combinators support
sequential and properly nested execution only. Their scope remains installed across suspension.
An async/blocking body may wait for host calls when no unrelated guest work runs during the wait.
Do not overlap it with sibling continuations, including ordinary host calls: their effects can
become observational entries owned by the wrong custom operation, causing incorrect replay or
invocation failure. Misuse is not reliably detected. Await nested operations; do not detach work.
Stop and join outstanding work before the body exits, including on failure or cancellation. A
fail-fast join does not necessarily wait for surviving siblings, and eventual joining does not
make concurrent custom scopes supported. This restriction does not apply to ordinary built-in
durability outside custom operations or to the Rust poll/drop scope integration.

`Incomplete` (`prepare_incomplete_live_repair`): the handle switches to live completion of the
*existing* `Start` — no second `Start` is appended. The four function types are `ReadLocal`,
`WriteLocal`, `ReadRemote`, and `WriteRemote`; nesting comes only from explicit
`Start.parent_start_index`, never from a type payload or temporally open sibling call.
Top-level incomplete calls repair in place. Ambient entity lineage records ancestry and keeps
ordinary calls' own call-local repair; it grants no permission to explicitly parented protocol
children. An explicit child requires its protocol owner's explicit
`AccessClaimOptions::incomplete_repair_authorized_by_scope` decision, including when the owner
records typed resource loss rather than repeating an effect. An unauthorized child reaching
generic repair is an invariant error naming the function and Start index. `ScopeReplayRecovery`
retains the protocol owner's recovery/readiness decision. There are no extra generic scope
Start/End records; demand streams retain their actual parent call session, and RDBMS retains
its transaction protocol Start/End. Snapshot and checkpoint admission use the in-flight live
call counter, including permits owned by open transaction protocols.

Authorization and protocol readiness precede live publication. Terminal refusal abandons replay
for failure (`authoritative_replay_failure`), not publication over sibling history. Never rewind
or discard another task's history. A typed-loss End belongs to the original Start: HTTP without
resend returns `http-response-incomplete`, RDBMS `resource-lost`, and TCP `connection-broken`.
UDP `ReadRemote` instead re-executes receive after rebind.

**A denied call records its denial.** When the host denies a live call before it runs, the call
writes its `Start` with `denial` and an `End` that holds the pair's denied response
(`BegunCall::start_live_denied`, `DurableCallSession::invoke_deniable_access`, or
`AccessClaimOptions::denial`). The operation never runs. A pair declares its denied response with
`denied = ...` in `host_payload_pairs!` (`DeniablePair::denied`); the response depends only on the
recorded request and the denial, so live execution and recovery produce the same value. Replay of
a denied `Start` without a terminal does not return `Incomplete`: the session appends the denied
`End` and returns `Replayed`. A crash between the two entries therefore cannot run the denied
operation. Call sites keep their replay branch free of authorization.

Accessor permission checks belong to owned call initiation (`AccessPermissionCheck`), after the
claim establishes that the call is new and before its immutable `Start`. Store liveness alone
cannot classify a call: new work can reach the replay tail before live publication, and historical
retained Starts can be claimed after publication. Claimed calls keep their recorded decision;
only new calls authorize, including incomplete entities continuing beside sibling history. HTTP
quota admission follows authorization in the request builder, still before `Start`, and preserves
typed quota traps. Denied calls and historical claims do not consume HTTP quota.

**P3 HTTP recovery is call-local.** `client::send` records `WriteRemote` for all methods
(`p3/http/send.rs`). Every method uses `Cancellable`: a guest drop records `Cancelled` and
replay never resends that cancelled call. Only crash-like loss leaves an incomplete send.
Request-body intake continues while replay resolution waits, so a streamed writer can reach its
next durable call. A trap leaves the Start incomplete, never Cancelled.
A crash before the response headers therefore completes the recorded send
`Start` in place (same index, same derived idempotency key, the deterministic guest re-produces
the request body) and keeps every operation recorded next to it. The response body's
`P3HttpClientConsumeBody` parent chooses owner-authorized re-execution only after request-body
readiness, or `ScopeReplayRecovery::CompleteWithLoss` when no reconstructable resend exists.
On replay its original Start is retained and its recorded chunks replay to the guest. Without a
resend, the live tail ends with `HttpResponseIncomplete` under that original Start. With a resend,
the first live demand continues the body (`response_body.rs::resume_recorded_response`): nothing delivered means
a fresh re-issue of the recorded request; a delivered prefix means a re-send with
`Range: bytes=<delivered>-`, accepted as a `206` continuing at that offset or as a full response
whose prefix is byte-identical to the delivered bytes (a blake3 digest accumulated over delivered
chunks, on both the replay and the live path). A request that carries a guest-set `Range`
header is re-sent unchanged and any successful response is accepted only through the same
prefix verification. Anything else ends the body with
`ErrorCode::HttpResponseIncomplete` after the prefix the guest observed: a different response is
never spliced after bytes the guest already saw, and the outcome is retry-exempt. The same
prefix verification applies to the in-session `Range` resume after a transient mid-stream
failure. External peers must deduplicate ambiguous writes. Preview 2 HTTP is not supported;
Rust uses `golem_rust::http`.
Tests: `tests/http/concurrent_recovery.rs`.

Host completion time and guest observation time are different facts. Only the second is a guest
input, and only the second must recur exactly; the first may vary between runs.

Fork and revert retain the exact inclusive prefix. A cut between `Start` and its terminal uses
ordinary incomplete-call recovery; a cut between an accessor `End` and its delivery/discard
marker uses `AtReplayTail`. No outcome beyond the cut is inherited. Revert leaves deleted entries
physically present, so completion-marker discovery excludes deleted regions before checking
uniqueness. A replacement marker for a retained `End` then survives the next reconstruction;
two visible markers for one `Start` remain corruption. Remote-transaction
outcome cuts are still rejected. Test: `reverted_completion_marker_can_be_replaced_and_reconstructed`.

## Replay cursor, claims and strictness

`ReplayCursor` (`replay_state/cursor.rs`) is the single chokepoint: `try_get_oplog_entry` reads
speculatively, `commit_consumed_entry` commits, `move_replay_idx` advances and emits
`ReplayFinished` exactly once. Concurrent accessor host calls may append `Start` entries in
scheduling order that replay does not reproduce, so `claim_start_matching` claims the *first
unclaimed matching* `Start` between cursor and target. A claim with an invocation id is a lookup:
the cursor indexes the recorded `Start`s by invocation id (`start_invocation_ids`, built when the
replay target is set and kept in step with it), and the claim takes the first unclaimed entry
before the next delivery boundary. Only claims without an id scan the oplog. Either way the
lookup routes concurrent completions to the right awaiter. It does not license the guest
to make different calls; when no matching `Start` exists, replay fails with a divergence error
(after any active entity body that owns the cursor head has consumed it; see below).

**Invocation ids.** Scan-ahead alone cannot tell apart concurrent calls with the same function:
their host tasks reach the claim in an order that depends on host scheduling (a replayed 600 s
watchdog `wait-for` could take the record of a completed 300 ms `wait-for` and fire at once). So
every `Start` carries the invocation id of the guest call that recorded it
(`Start.invocation_id`), and a claim (`StartClaim`, `replay_state/claims.rs`) requires the id, the
function name and the kind (call or scope). Request payloads and scope names do not identify
calls. The id (`DurableInvocationId`) is an anchor and an ordinal: the anchor is the oplog index of
the current agent invocation (`AgentInvocationStarted`), entity invocation (`Start`) or parent
custom durable invocation (`Start`), and the ordinal counts the calls numbered under that anchor.
Root custom durable invocations share the counter with ordinary calls.

**Custom execution scopes.** Attribution follows the context installed on the current guest
task, not the lifetime of a live custom resource. `begin_custom_durable_invocation` installs a
scope. The synchronous `exchange_custom_invocation_context` captures without changing it for
`None`, or consumes and installs a snapshot for `Some`, returning the actual previous context.
An empty snapshot clears attribution. Snapshots are resident-only, journal nothing, and do not
keep scopes alive. Before installation the host validates the invocation anchor and every scope
registration and live resource; a stale ancestor invalidates the whole snapshot.

Rust `Durability::new` prepares the request; async runners observe and begin on first poll.
Their wrapper saves the outgoing context and restores the dynamic caller after each poll and
after destruction. Body futures, including completed-body resources, are destroyed before
finish; cancellation destroys descendants before the enclosing live resource and writes no End.
Closing a resource absent from the current stack is normal and leaves that stack unchanged.
Finish validates active children and response decoding before removing the resource or scope.
Nested work must complete or be dropped before its parent finishes. Detached work does not
automatically acquire the scope. Other SDKs have synchronized bindings but do not yet propagate
custom scopes across concurrent continuations; do not infer a cross-SDK concurrency fix.
The synchronous exchange binding (`store | trappable`, not async) is essential: an async import
would capture host-task context too early for the per-poll switch.

- A guest import reserves its id (`GuestImportNumbering::reserve_import_identity`,
  `durability/invocation_id`) at its first durable call, while it is still the guest's newest
  import; a later reservation is refused. The Store's call hook (`CallHook::CallingHost`) decides
  the anchor from the calling guest task's custom invocation scope, and the runtime observer
  (`RuntimeStore::observe`, `worker/suspension`) registers the import, both inside the guest's
  call. Wasmtime reports every import, including polls and custom-context exchanges whose count
  depends on scheduling, so only reserving imports take an ordinal. A host function that `await`s
  before its first durable call reserves at entry (`reserve_invocation_id_before_await`). All
  durable calls of one import share its id. Imports inside a custom invocation body are numbered
  under that invocation's `Start`, so they do not shift the ordinals of their root siblings. A new
  root restarts the ordinals.
- A task that an import spawns is not a guest import: it must receive the id explicitly
  (`InvocationIdSource::Reserved`). A durable call from an unidentified activity fails. Host
  tasks that no guest import issued, such as a tool call a client started, record no id
  (`InvocationIdSource::HostTask`).
- A direct (Store-holding) call takes the identity of the running import
  (`direct_call_invocation_id`, `entity_context.rs`). Store-holding host code that runs between
  guest calls takes the next ordinal under the root.
- A call recorded inside an open durable scope records the id of the call that opened the scope,
  and a host-internal child records the id of its owner. Span-only resource cleanup is a hint
  referencing its opening Start, not another call carrying an invocation id.
- A tool invocation records the id of the guest import that dispatched it, on both the accepted
  entity invocation `Start` and the rejection `Start`.
- Calls observed by a custom invocation, calls while snapshotting, and calls outside an
  invocation record no id. Replay never claims observed calls; calls without an id are claimed in
  oplog order.

**Entry ownership.** A reader that drives the cursor without owning the entry at its head — a
positional marker read, a direct call awaiting its own `End`, a sibling's terminal drain — is
entitled to nothing it did not record. Kind and owner are validated before consumption:

- A durable-call `Start` nobody has claimed is never handed to a positional reader and is never
  parked on either: its owner may be a concurrent host task that needs the very Store the parked
  reader holds (an entity body waiting for admission, an accessor task behind a Store-holding
  direct call). The cursor commits past it and **retains** it (`CursorState::retained_starts`,
  in oplog order, together with its terminal once reached). The owner's later identity claim
  checks retained `Start`s before the head (`claim_retained_start` in `claim_start_matching`); a
  `CompletionDelivered` marker at the head may belong to a
  retained `Start`, so retained-first ordering is required, not an optimisation.
- A delivery barrier blocks physical advancement, not adoption of already-retained history.
  `with_claim_tx_admission` first polls one gate waiter, then uses the restricted `RetainedClaims`
  capability under the state mutex while that gate is unavailable. Using retained history means
  claiming a `Start` the reader has already passed, registering its resolver, and publishing its
  claimed position in `last_replayed_non_hint_index`. It does not move the physical reader.
  Advancing the reader means reading and committing the next entries through a gate-owning
  `CursorTx`. On a retained miss the claim keeps the same waiter and rechecks retained state on
  progress; only a gated transaction may classify absence.
  This preserves the cleanup fence's queue ordering.
  Resolution likewise prefers a ready gate and completes the ordinary drain, but can return an
  already-routed host result when the gate remains held. Host readiness is not guest delivery:
  the actual observer still waits for its exact marker. The marker-consuming owned operation
  constructs the `ReplayDeliveryBarrier` before returning, so discarding its unobserved result
  latches failure and releases the gate rather than silently acknowledging delivery.
  The owned resolution task also keeps its receiver and `HistoricalReconstruction` guard when
  the caller cancels. The guard cannot settle reconstruction while a surviving transaction still
  uses it. Cancellation does not authorize generation replacement: the surviving operation must
  finish and reconstruction fences must settle. A cursor-operation fence alone does not prove
  reconstruction has settled.
- Retaining is not consuming. Retained `Start`s and their attached terminals are committed
  through `commit_retained_entry`, which does not advance `last_replayed_non_hint_index`; the
  position is published by `publish_claimed_position` only when the owner claims the `Start`.
  A durable call that derives its position from that index (`begin_function` without a scope:
  the retry point) therefore observes the same value the live run
  saw as the oplog tip before its own `Start` was appended, not a position advanced by entries
  no caller has consumed. The same holds for replay events: a `CardDerived` or `CardInstalled`
  hint trailing a retained `Start` or its terminal is a side effect of that call, and its owner
  looks it up when it replays the terminal (`pending_card_derivation`). Publishing it while the
  `Start` is merely retained would let an intervening authority boundary apply it first, so
  events recorded during a retained commit are deferred on the `RetainedStart`
  (`deferred_events`) and published by `publish_deferred_events` when the `Start` leaves the
  retained map: claimed, adopted into a custom subtree, folded into the abandoned tolerance, or
  released at the live invocation end (`release_retained_starts_at_live_invocation_end`).
- Recovery repairs an eligible incomplete call at its own `Start`, retaining neighboring
  completed calls. It does not discard a suffix to restart a group of guest effects.
- Positional reads through the shared cursor are attributed per Store (`get_oplog_entry(scope)`,
  `OplogEntry::entity_attribution()`): the primary agent consumes only entries with no entity
  parent, an entity body only entries recorded under its own invocation `Start`. A read that
  finds another Store's entry at the head parks only while that Store can still consume it (the
  primary while the cursor replays; an entity body whose `Start` is claimed, retained, or
  scan-ahead claimed). Otherwise `check_parked_positional_read` reports the head as divergence
  instead of hanging replay; the invocation-boundary reader never parks on another Store.
- A `Start` claim that finds no match decides the missing `Start` only when no active entity
  body encloses the entry at the cursor head. A tool call claims its entity `Start` inline, but
  the cursor drains the body's first entries only when the spawned supervisor is first polled. A
  clock call of the guest that claims in that gap stops at the body's unclaimed `Start`.
  `head_owner` (`cursor.rs`) names the owner of the head entry and is shared with
  `check_parked_positional_read`: the parent of a nested `Start`, or the entity attribution of
  any other entry. `nearest_active_body` (a pure function) resolves the nearest active body that
  encloses the head, and the one that encloses the claim's parent: it walks parents and stops at
  the first active body, so a scope of a nested entity resolves to that entity even after the
  outer body settled. The parents come from state the cursor already holds: each resolver
  awaiter records the `parent_start_index` of its claimed `Start` at registration, and retained
  `Start`s keep their entries. The missing-claim path reads no oplog entry; a custom invocation
  claim takes the head from its own exact-id scan. The pure rule
  `missing_start_waits_for` then decides: the claim waits for the head's body, unless that body
  issued the claim or the claim's parent was appended live. A call of an entity nested inside
  the head's body still waits, because that entity is another Store.
  A waiting claim is `Blocked(BlockedOn::ActiveBody)` and runs again on cursor progress or a
  change of the active-body set. The rule applies to ordinary, request-matching, scope and custom
  invocation claims. A body that settles without consuming the head and a claim issued by the
  body that encloses the head stay strict divergence at once. The path where a claim finds its
  `Start` does not change.
- An ordinary claim that still finds no match (`CursorTx::decide_missing_start`) is a reader
  that must make progress. A crash prefix can hold the `Start` of one concurrent guest call and
  not the `Start` of its sibling, because ids are reserved in guest call order but `Start`s are
  appended in host scheduling order, so the sibling's unclaimed `Start` at the head is not
  divergence. The claim treats each record at the head as its owner would once it claimed its
  own records (`sibling_record_at_head`), so the verdict does not depend on how far the sibling
  got: it retains a top-level `Start` with an invocation id, a `Start` under a retained or
  claimed `Start` with the same invocation id (the call of a scope, the child of a host call, a
  tool invocation of the dispatching import), and a record in the subtree of a retained custom
  invocation (adopted when that invocation is claimed). It waits at a record of the body of a retained
  entity invocation, as at an active body's entry or a delivery boundary. It then decides
  against the entry the cursor stops at: the replay target is `ReplayEnded`, and anything else
  (including the call of a settled body, or a foreign id under an ordinary call) is
  divergence. An unclaimed `Start` with the claim's own invocation id was recorded by the
  claiming guest call itself and is divergence before the cursor moves. This identity check
  also precedes `ReplayEnded` and `StoreAlreadyLive`: cursor exhaustion can leave unclaimed
  retained calls, and neither exhaustion nor Store liveness permits replacing their function,
  type, parent, or observational owner. Tests:
  `replay_state/tests.rs::retained_invocation_identity_is_checked_before_live_continuation`,
  `replay_state/tests.rs::missing_start_claim_*`,
  `tests/websocket.rs::websocket_report_loss_recovers_a_crash_prefix_with_one_sibling_start`.
- RPC connection reconstruction also validates the requested remote agent against the recorded
  constructor request before restoring its span or resource. Matching a call's invocation id
  and function does not authorize a different RPC target. The snapshot negative controls in
  `tests/rpc/gc_timed_cleanup.rs` require rejection and full replay without duplicate effects.
- Retained `Start`s that survive to the invocation boundary fold into the abandoned-record
  tolerance (`AbandonedStarts`); only `can_drain` kinds are retained at all. When a live primary
  invocation finishes, retained `Start`s that are closed by a recorded `End`/`Cancelled` are
  released with a warning; an unclosed one is a divergence error
  (`release_retained_starts_at_live_invocation_end`), so `AgentInvocationFinished` is never
  appended after an open `Start`. A settled entity body that returned without claiming a retained
  descendant is a structural divergence (`ensure_body_claimed_retained_descendants`, `entity.rs`).
- While unclaimed retained `Start`s exist, a call arriving after the live transition may still be
  their replayed owner, so durable-call admission uses `durable_call_is_live()` (stricter than
  `is_live()`): such calls claim first and append a fresh `Start` only when replay reports none
  remains (`claim_start_for_store`, which also returns `StoreAlreadyLive` for an incomplete entity
  that continued live locally). The primary runtime treats a cursor that became live between its
  liveness check and the claim as `ReplayEnded`, not divergence. Custom (guest manual) durability
  follows the same per-Store admission: `begin_custom_durable_invocation` claims its `Start` by
  invocation id through `claim_custom_start_for_store` and continues to live with the Store's own `ReplayToLiveRole`
  (`Primary` only for the primary agent runtime), so a completed-replay entity body can neither
  settle the primary fence nor fall through to fresh execution. Positional readers,
  authorization and snapshot decisions keep using `is_live()`.
- Per-call quotas (`check_and_increment_{http,rpc}_call_count`, `record_monthly_{http,rpc}_call`)
  are charged before the call's `Start` is claimed or appended, so a refused call leaves no oplog
  entry. They use `durable_call_is_fresh(QuotaClass)`: `QuotaClass::of_recorded_call` is the
  single mapping from a recorded `HostFunctionName` to the HTTP or RPC quota it is charged
  against, the cursor publishes per-class counts of its unclaimed retained `Start`s
  (`retains_unclaimed_start_charged_to`), and only a call charged to a class that still has a
  retained `Start` — which may be that `Start`'s late owner — is exempt; retained `Start`s of
  other classes (or of none) do not suppress the charge. The charging sites name their class
  (HTTP for P3 HTTP, MCP dispatch and external durable streams; RPC for `golem:rpc` invokes).

Tests: `replay_state/tests.rs` (`positional_reader_waits_for_a_retained_entity_start_to_be_claimed`,
`interleaved_positional_markers_are_consumed_only_by_the_recording_store`,
`request_matching_claim_adopts_retained_start_behind_its_own_delivery_marker`,
`retained_start_publishes_the_non_hint_position_only_when_claimed`,
`retained_start_names_are_published_per_call_kind`,
`closed_retained_starts_are_released_at_live_invocation_end`,
`unclosed_retained_starts_are_rejected_at_live_invocation_end`),
`tests/tool_streaming.rs` (
`direct_call_waits_without_blocking_body_admission_*`,
`incomplete_custom_durability_waits_for_overlapping_completed_reconstruction`).

Tolerance machinery (poll-ID stabilisation, response reordering, synthesized readiness, "skip
unmatched entries") hides the first real bug and must not be added. Retaining an unclaimed
`Start` is not tolerance: the entry stays claimable only by its identity-validated owner and is
reported if nobody claims it.

## Replay-to-live

Positional operations (retry-policy entries) use
`get_oplog_entry_or_continue_live` and `prepare_live_continuation_at_replay_tail` in
`durable_host/mod.rs`. The positional read waits out reserved completion-delivery gates and
distinguishes an entry from `ReplayEnded`. Continuation is allowed only for the primary agent
at `ReplayEnded` or an incomplete entity;
a `ReplayingCompleted` entity is rejected. The helper returns `None` when already live or after
the guarded transition finishes, and retries replay on `ReplayResumed`. A wrong recorded entry
is still a mismatch. Transaction-protocol reads remain strict.

Three different facts are involved, each with its own owner:

1. **Cursor exhaustion** — the recorded history has been consumed. An observation only.
2. **Shared primary publication** — `transition_phase` moves Replaying → Settling → Live
   (`replay_state/cursor.rs`). `ReplayState::switch_to_live` with `ReplayToLiveRole::PrimaryAgent`
   runs `begin_primary_settling`, then `finish_settling_to_live` loops until every
   `HistoricalReconstruction` fence has validated and `CursorTx::finish_primary_settling`
   publishes Live (or reports `ReplayResumed` when new history appeared).
3. **Entity-local liveness** — `store_is_live(mode, local_live_tail, shared_live_published)`
   (`durable_host/mod.rs`): the primary Store is live iff (2) has published; a
   `ReplayingCompleted` entity Store is never live; a `ReplayingIncomplete`/`Live` entity Store is
   live iff its own `local_live_tail` is set by `PendingReplayToLive::finish_at` (`#[must_use]`).
   For `ReplayToLiveRole::NonPrimary`, `switch_to_live` skips (2): the entity Store goes live for
   its own invocation scope (after attachment admission) without waiting for the primary.

Tests: `entity_store_liveness_is_scoped_to_its_invocation_mode`,
`pending_replay_to_live_is_fail_closed_until_finished`,
`replay_state/tests.rs::switch_to_live_wakes_parked_awaiter_as_incomplete`,
`replay_state/tests.rs::primary_publication_requires_completed_reconstruction_validation`,
`tests/tool_streaming/mod.rs::incomplete_custom_durability_waits_for_completed_reconstruction`.

Consequences: a host function must not perform a live effect because "the cursor is at the end";
it must observe `is_live` from the durable context. A markerless `End` is delivered once the
cursor drains (`await_natural_tail_end`) with a live-armed delivery token
(`CompletionDelivery::prepare_delivery`), which makes "crash after End, before delivery" safe
without re-executing.

## Invocation queue and results
`enqueue_worker_invocation_with_effect` (`worker/mod.rs`): dedupe via `lookup_invocation_result`
(anything other than `LookupResult::New` returns without appending), then append
`PendingAgentInvocation` and commit before the caller learns the invocation was accepted. The
invocation loop appends `AgentInvocationStarted`, runs the guest, and
`on_agent_invocation_success` (`durable_host/mod.rs`) appends `AgentInvocationFinished` with the
result and waits for the commit receipt before waiters are notified. Durable agents use
`CommitLevel::Always` (storage first); ephemeral agents use `CommitLevel::Deferred` (ordered writer
handoff, without waiting for storage). Completion does not await the status fold; freshness-sensitive
reads queued on the same state actor wait behind it. Failures go through `on_invocation_failure`.

The live loop hydrates an ordinary invocation from its committed Pending entry into an
executor-local `HydratedInvocation`. Started reuses that exact payload reference, including its
cached serialized bytes, instead of serializing and uploading the payload again. Lowering uses
a separate invocation clone and cannot replace the retained original payload. This does not
reuse the Pending context or wallet pins: the start hook checks current authority, records the
executing context and current wallet pin, and commits Started with `CommitLevel::Always` before
guest execution. `ManualUpdate` takes its separate snapshot path and does not pass its Pending
payload to this hook. Fork rehomes both references into the target owner; archive and revert do
not individually delete a referenced payload blob.

During replay the recorded result is compared with the recomputed one
(`replay_equivalent`); a mismatch is an `unexpected_oplog_entry` determinism error. Tail work
(`durable_host/tail_work.rs`) keeps the store loop running until no spawned task is still
*active*, so a task's positional `Start`/`End` never lands after `AgentInvocationFinished`
(tasks parked at guest-driven safe park points may span invocations). Hints can follow
`Finished`: `invocation_loop.rs::agent_invocation_finished` completes the streaming session
(protocol terminals, `StreamSession { Finished }`) after `on_agent_invocation_success`.

This execution-needing path acquires no capacity during preparation; cancellation cannot split its
durably enqueued acceptance prefix. Read-only hits/followers persist no invocation/result/alias.
Test: `tests/api/mod.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart`. After an
executor restart, an old key returns the recorded result without re-running the guest.

P3 HTTP send arms a Store-owned request leak guard before Start admission, not just before
replay resolution. Guest cancellation can drop the send while its claim waits at the cursor gate.
The guard fences owned cursor operations, deletes and drains the request. This releases the
guest's body/trailer writes without re-issuing the request. Both replay consumption and live execution disarm
the guard when they take request ownership (`durable_host/p3/http/{send,replay}.rs`).

P3 HTTP response frames remain explicit parent/child durable calls, not consumer-journal records.
Data and ordinary EOF settle their child delivery tokens at Wasmtime's per-read consumption
observer. Producer readiness is too early: it can reverse a timer/EOF ordering during replay.
The producer exposes at most the guest read's capacity and retains the frame's suffix; the first
positive consumption records the frame's marker, and later suffix reads add no durable child.
A fresh frame demand always parks, while an already buffered suffix is available inline on both
paths. Explicit cancellation acknowledgements keep their producer handoff boundary before
waiting for parent finalization, avoiding an observer/acknowledgement cycle. Trailers still settle
the parent's token at the future's terminal observer. Tests: `tests/http.rs` cancellation and
offline-replay controls, and `tests/tool_streaming/mod.rs` Git partial-download recovery.

## Durable RPC exactly-once

`RpcTargetAdmission::{Recorded, LiveOnly}` keeps admission separate from call resolution.
Deferred durable activation records or replays its own decision (`Recorded`). Non-deferred
targets return `LiveOnly(PermissionTarget)`; their asynchronous permission check runs only in
`ResolvedCall::Live`, after resolution, preserving operator authorization and decision telemetry.
Denials use `persist_*_denial_from_begun` to record on the already-begun call, not begin another
one. Recorded calls, including incomplete repairs, retain admission without re-authorizing.

1. The caller opens a durable call; its own `Start` index is the call identity. Live
   computes the request when it appends the `Start` (`start_live_at_index`), so the request
   can embed the index; replay reads the index from the claimed `Start`.
2. `IdempotencyKeyPosition::derive(start_index)` (`durable_host/mod.rs`) yields
   a key derived from the caller invocation key and this call's own physical `Start` index.
   Incomplete-call repair reuses that `Start` and key. Streaming RPC also uses the exact `Start`
   index to distinguish concurrent calls. Its session descriptor excludes retry tracing and
   canonicalizes environment ordering while retaining execution-relevant configuration.
3. Caller state that the target may depend on is committed before the first dispatch.
4. The target persists `PendingAgentInvocation` (durable acceptance) and later
   `AgentInvocationFinished`; same-key arrivals attach to that one invocation/result.
5. Replay of a completed call returns the recorded `End`; replay of an incomplete call (`Start`
   without `End`) re-dispatches with the **same** key (`InvocationFreshnessDisposition::MayExist`,
   the default). RPC is a `WriteRemote` call, so repair completes the existing `Start`.
6. Only the first live dispatch to an ephemeral target may use `KnownFresh`
   (`durable_host/rpc/`), after a `DurableOnly` commit makes the caller's `Start` durable.
   Streaming RPC consumes this hint during target preparation, before it builds stream mappings
   with the returned fingerprint; the subsequent invocation uses `MayExist`.
   Every repair, retry, and replay dispatch uses `MayExist`. Ephemeral phantom
   identity is `ephemeral_invocation_phantom_id` = UUIDv5 of the idempotency key — deterministic,
   never random.

### Pending RPC waits and proactive suspension
`OwnerExecution` shares one `OwnerSuspension` authority across the primary, entity and native
participants (`worker/instance/mod.rs`, `worker/suspension.rs`). A current runtime blocked witness
is necessary but insufficient: every admitted participant must be accounted for, with no active
outer poll or unclassified non-root work. Unknown work, preparation and borrowed synchronous
RPC waits veto automatic suspension; raw sync RPC does not automatically suspend.
Owned async RPC activities become eligible after their grace period; the owner rechecks while
other work vetoes. Owned timers (including the narrow P2 timer-only poll/block dispatcher) and
promises provide deadline/activation evidence. This is not general P2 readiness adaptation.
Durable source reads bind to their exact runtime transfer activity and retain the read future.
Only an established source wait can be passive: locally registered external inline input and
its descendants veto suspension; agent-hosted input, invocation output and attached downstream
waits can qualify with a durable timed recheck. Active journaling, publication and settlement
are not passive waits. Root/result return alone is not idle.
`RuntimeStore::drive` persists the earliest timed wake, then revalidates the same activity revision
and eligibility before committing timestamped suspension. Activity changes invalidate stale
evidence. Existing interrupt/retirement precedence and discard/replay reconstruction remain
unchanged; there is no new cleanup or lifecycle protocol. Borrowed waits in
`durable_host/suspendable_wait.rs` observe only readiness and interruption.
See `reference/rpc-suspension.md` for timing and admission details.
Exactly-once describes the *target's logical execution and effect*, not attempts or packets. Tests:
`tests/rpc.rs::counter_resource_test_2_with_restart`, `failed_ephemeral_invocation_retry_does_not_reexecute`,
`ephemeral_rpc_invocations_get_distinct_final_identities`,
`reacquire_permits_restart_preserves_accepted_queued_live_invocation`, and
`tests/api/mod.rs::lost_card_transfer_response_converges_after_source_and_target_restart` (a wrapped
worker proxy drops the response; after both sides restart the proxy has seen *two* identical
attempts while the target oplog holds exactly one `CardTransferred` — attempts ≠ executions).

Ephemeral targets are deliberately fail-stop: a same-key retry does not re-execute accepted
incomplete work, but eventual completion is not guaranteed because the target has no durability
to resume from. An agent calling itself is rejected ("RPC calls to the same agent are not
supported"); an entity body's synchronous call into its owner must pass
`OwnerLane::ensure_synchronous_owner_call` (`WouldDeadlock` otherwise).

## Snapshots and updates

Snapshot save/load runs guest code in a third mode: it neither appends nor consumes durable-call
oplog entries (`DurableCallSession` created with `persisted: false`; `durability_is_suppressed`
is exactly `snapshotting_mode`).

Which history the new instance replays is decided in `Worker` construction (`worker/mod.rs`,
`create_instance`, through `Worker::decide_start`, which loads the persisted rejections and calls
the pure `worker/snapshot_selection.rs::decide_start`). The status keeps two automatic snapshot
records: the last one, `last_automatic_snapshot` (an `AutomaticSnapshot` with its index, time,
revision, filesystem snapshot name and confirmation), and `previous_usable_automatic_snapshot`,
the newest usable one before it. A record is usable when it has no filesystem snapshot name, or
when a `SnapshotConfirmed` entry confirms its name. `select_automatic_snapshot` takes the first of
the two that `passes`: it is usable, has the current revision, is not in the rejected set, is not
unavailable for this start, is not the selected record of a live `FailedUpdate` with
`SnapshotFault::Unavailable`, and has no name when filesystem snapshots are disabled. An ephemeral
agent never has an automatic record: `resolve_agent_properties` gives it
`SnapshotPolicy::Disabled`, because a start from a snapshot record skips the initialization that
the replay of an ephemeral agent needs, and that replay then fails.

`decide_start` reads the head of the update queue and gives one `StartDecision`:

- `PersistStrategy` for a public `Automatic` admission `P` at the head that has no strategy yet.
  The queue filter (`QueueFilter::UnselectedAutomatic`) lets a record pass only when the target is
  newer than the current revision and the record is before every `SnapshotBased` element of the
  queue; `incompatible_before` closes the filter when a live `FailedUpdate` of the same target
  from the same source has `SnapshotFault::Incompatible`. A record that passes gives
  `SnapshotAssistedAutomatic { target, source revision, source start index, snapshot index,
  snapshot revision, filesystem_snapshot }`; no record gives plain `Automatic`, a full replay.
  `create_instance` appends that description as a second `PendingUpdate` with
  `update_attempt_index: Some(P)`, and decides again. The status fold
  (`worker/status/update_queue.rs`) refines the head with that entry instead of adding an update,
  so reconstruction never recomputes the choice.
- `FailHead` for a selected snapshot-assisted head whose source does not hold any more
  (`stale_assisted_head`: the revision or its start index changed, or the target is not newer).
  The start writes the failed update and decides again.
- `Start(StartSelection)` otherwise. `StartSelection.baseline` is one `SelectedBaseline`: the
  record of a snapshot-assisted head (`AssistedPending`), the record of a `SnapshotBased` head
  (`ManualPending`), a periodic record when no update is pending (`Periodic`), the authoritative
  baseline (`AssistedPromoted` or `ManualPromoted`, from `authoritative_snapshot`), or
  `InitialFiles`. A pending plain `Automatic` head uses the authoritative baseline or the initial
  files.

The replay revision of `Periodic` and `AssistedPending` is the revision of the record; for a
snapshot-assisted update that is the source revision. `filesystem_snapshots::plan_start` maps the
baseline to one `StartPlan`: the tree source (`Store(name)`, `InitialFiles`, or `SourceFiles` for
a manual-update record without a name) and the `ReplayBaseline` (the record whose application
snapshot `prepare_instance` loads, the skipped override, the role). The restore and the load come
from that one value, so an application snapshot never loads without the files of the same
record. A record with a name on an executor without filesystem snapshots gives `SnapshotsDisabled`.
`prepare_instance` (`durable_host/mod.rs`) then branches on the pending update:

- `SnapshotBased` — the save hook already ran and the payload is already recorded; the store must
  already be live, and `finalize_pending_snapshot_update` loads it into the new revision. It writes
  `SuccessfulUpdate` only after the load completed.
- `SnapshotAssistedAutomatic` — `try_load_snapshot` loads the application snapshot of the selected
  record `S` into the target, after `materialize` restored its filesystem snapshot. `replay`
  replays the stopped-source tail with the source revision's metadata: the executable metadata,
  `GOLEM_COMPONENT_REVISION`, the config and the initial files. At `ReplayFinished`,
  `prepare_revision_update` (`durable_host/revision_update.rs`) fetches the target metadata and
  applies the initial-file rule from the source to the target, at the update point, as a full
  replay does. Then `SuccessfulUpdate` is written. A later start from the promoted `S` restores
  the same filesystem snapshot and applies the rule at the replayed `SuccessfulUpdate`, in the
  same order.
- `Automatic` — `try_load_snapshot` loads the authoritative baseline, if any, then `replay`
  replays the remaining old history against the new component; success is recorded during that
  replay.
- No pending update — `try_load_snapshot` loads the selected periodic record.

Every start failure goes through `worker/start_outcome.rs::decide(role, head, RawStartError,
lost_shard)`, one table of problem by column (`Periodic`, `ManualPending`, `ManualPromoted`,
`InitialFiles`, `AutomaticPending`, `AssistedPending`, `AssistedPromoted`). It gives a
`StartAction`: `FailUpdate { entry, reject }` with the entry already built from the paired queue
element (attempt index, the snapshot-assisted details of the head, `snapshot_fault`, and the
details text with its stable code), `SkipPeriodic`, `RejectPeriodic`, `Error`, `Retry` or
`ShardLost`. The sites only perform the action; `on_worker_update_failed` takes the built
entry. After `FailUpdate` the start returns `RetryDecision::Immediate` and the outer loop rebuilds
on the source revision. The details have the form `CODE: text: cause`, and the text says what to
do next (`Code::prefix`), for example request the update again, or use a manual snapshot-based
update. The codes are `pub(crate) const` items of `start_outcome`:

- `UPDATE_SNAPSHOT_UNAVAILABLE`: the store lost the filesystem snapshot (`RestoreClass::Lost`) or
  the application snapshot payload of the selected record of an assisted update
  (`SnapshotFault::Unavailable`, and the record is rejected); with a manual-update text, the store
  lost the filesystem snapshot of a pending manual update.
- `UPDATE_SNAPSHOT_INCOMPATIBLE`: the assisted target could not load `S`, or the tail diverged
  (`SnapshotFault::Incompatible`; `S` is not rejected, and the next request for the same target
  from the same source is a full replay).
- `UPDATE_REPLAY_FAILED`: a plain automatic update (full replay) failed in the load or the replay.
- `UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS`, `UPDATE_SNAPSHOT_RESTORE_FAILED` (also for a full
  local disk, `RestoreClass::DiskFull`): the filesystem snapshot of a pending manual or assisted
  update cannot come back on this executor.
- `UPDATE_TARGET_NOT_FOUND`, `UPDATE_TARGET_REFUSED`: the component service has no target
  revision, or refused it (`ComponentServiceRefused { kind }`). `ComponentServiceUnavailable`
  writes no failed update: the recovery path retries the start.
- `UPDATE_BLOB_STORAGE_REFUSED`: at the update point, the blob storage refused the executor a read
  of an initial-file source of the target (`FilesystemError::InitialFileRefused`, from
  `BlobFailure::Refused` of a `BlobRefusedError`: an S3 4xx other than 408, 416 and 429 and other
  than a code that asks for a retry, or `PermissionDenied` of the filesystem backend). The install
  loads every source before its first change, so the agent runs on its source revision. The text
  names the `BlobRefusal`. A transient blob error (`InitialFileUnavailable`) writes no failed
  update and retries as a recovery; outside the update point a refused source is a
  reconstruction error. The guest blob store maps the same refusal to a permanent
  `BlobStoreError::PermissionDenied`.

Before loading a pending target, Worker validates its cached metadata with
`services/component.rs::component_support_error`. Admission uses the same support policy.
An unsupported target fails through `RawStartError::TargetUnsupported(reason)` with
`UPDATE_TARGET_REFUSED`, preserving the reason, attempt index and assisted details without a
snapshot fault or rejection. The current policy rejects WebAssembly threads (shared linear
memory). Admission charges the source revision for an unsupported target, so an oversized
target cannot park the agent before the refusal. After committing the refusal, startup releases
the old memory admission and re-admits the next queued target, or reconstructs the source if no
update remains. This restart does not consume the out-of-memory retry budget.

Component compilation runs in a blocking task. A cancelled join becomes `Interrupted(Restart)`;
a panicked join becomes `Runtime`. `RawStartError::TargetLoad` passes these executor failures
through for every pending update, unlike metadata-fetch failures. Worker converts a passed
target-load `Runtime` to `RecoveryRequired`, so the invocation loop retries with infrastructure
backoff without consuming the invocation retry budget. Interruption keeps its exact kind, and
retirement or a pending terminal interrupt still stops reconstruction. Ordinary parse errors
remain target failures; generic metadata and instantiation errors keep their existing outcomes.

Other tasks whose handles nothing aborts are joined through
`services/shutdown.rs::join_or_park_at_runtime_shutdown`: oplog entry and payload deserialization,
durable call and tool terminal encoding, and the detached stream session index tasks. Only the
shutdown of the runtime cancels such a task, for example during the final status flush sweep, so
a cancelled join parks the caller until the runtime drops it instead of failing it as a storage
error. A panicked join still fails, and a fail-fast read of the primary oplog still panics on a corrupt
entry; an archive read returns `UnreadableOplog` instead.

`worker/filesystem_snapshots.rs::UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS` is the code of a manual update
that cannot take its snapshot on an executor without filesystem snapshots, because the files of the
agent differ from its initial files.

Transient causes write no failed update, and the start retries: for every pending column,
`RestoreClass::Transient`, a reconstruction error, a full quota, an interrupted instantiation and
the load's `Retry`; for an assisted head also `RecoveryRequired` and `Interrupted`, so a frozen
assisted head retries with the same `S`; for a plain automatic head also `RecoveryRequired` (a
store failure of the baseline payload). A replay `Interrupted` error that reaches `decide` fails a plain
automatic update with `UPDATE_REPLAY_FAILED`, and `RecoveryRequired` or `Interrupted` fail a
pending manual update. The load of a pending manual update reports its own `ManualLoadResult`: a
failed load and a guest exit (`Exited`) fail the update without a code, and an interrupted load
(`Interrupted(kind)`) passes its interrupt, writes nothing, and leaves the update pending, so the
next start loads the payload again. A plain automatic head whose authoritative baseline does not restore
(`Disabled`, `Restore(Lost | Fixed | DiskFull)`) takes the cell of that baseline: the start fails
with a visible cause and writes no failed update.

The replay of a start with `SnapshotReplayPurpose` other than `None` is speculative:
`PeriodicRecovery` (after a periodic record), `AssistedUpdate` (after the record of a pending
snapshot-assisted update) and `AutomaticUpdate` (the replay for a pending plain automatic update,
from the authoritative baseline or from the start). A trap while such a replay still replays
recorded entries writes no invocation `Error` for the recorded work. An error trap whose fixed
decision retries, such as `OutOfMemory` (`ReacquirePermits`), keeps that retry
(`DurableWorkerCtx::speculative_retry`). Any other error trap ends the replay with its error, and
the start outcome decides: `UPDATE_REPLAY_FAILED` for `AutomaticUpdate`, a divergence or a plain
failure for `AssistedUpdate`, a rejection or a pass-through for `PeriodicRecovery`. An
`Interrupt` trap takes its fixed decision; the replay writes no `Interrupted` entry and no
`FailedUpdate`. During the startup replay the folded status is the status before the restart.
For an idle agent it stays `Idle`, and `worker/lifecycle.rs::interrupt_decision` ignores an API
interrupt (`Attempted interrupting worker which is idle`): nothing reaches the replay, no
`Interrupted` entry and no `FailedUpdate` are written, and the update completes without a resume
(`tests/hot_update.rs::an_interrupt_during_the_full_replay_of_a_pending_automatic_update_of_an_idle_agent_is_ignored_and_the_update_completes`).
Only a status of `Running`, `Suspended` or `Retrying` lets the interrupt through; then the
invocation loop records the request after the start as at the end of any generation
(`InvocationLoop::record_retry_interrupt_failure`). A lost shard writes nothing, and the new owner
runs the attempt again. The trap path is the same for all three purposes.
The purpose is `None` again when the replay finishes. No retry counter exists. Once
`S` is selected, an attempt never tries another record or a full replay. On success the fold sets
`authoritative_snapshot` to `{ S, SnapshotAssistedAutomatic { filesystem_snapshot } }` and
`component_revision_for_replay` to the source revision. A later start may use a newer periodic
record of the target; when that record is rejected, it falls back to `S`, never to the history
before it. A promoted baseline whose filesystem snapshot is lost, does not restore here or needs
filesystem snapshots on an executor without them, or whose application snapshot does not load,
fails the start with a visible cause; a transient cause retries.

With no pending update, an automatic snapshot load failure or divergent replay suffix rejects the
exact index of that record (`Worker::reject_periodic`, which applies
`SnapshotExclusions::rejecting`) and returns `RetryDecision::Immediate`. The outer loop recreates
the entire Store, component metadata, revision, and plugin context, and selects again: the other
usable record, the authoritative baseline, or a full replay. It never replays pre-migration
history. Only after preparation succeeds, and before readiness is published, is the rejected set
merged into the persisted set under the worker's `AgentFingerprint`
(`Worker::settle_exclusions_after_prepare`, through `WorkerService::reject_periodic_snapshots`).
The set holds exact indexes, so a newer record stays selectable.

An automatic snapshot payload-download failure, or a failed restore of its filesystem snapshot,
instead adds the index to the unavailable set of that startup attempt
(`Worker::mark_periodic_unavailable`), so the retry skips the record without rejecting it; a
successful preparation clears the set (`Worker::settle_exclusions_after_prepare`). A manual-update
snapshot cannot be skipped: its load failure is terminal, wrapped as failure to resume while
retaining the underlying cause.

### Filesystem snapshots

Executor startup creates `.scratch` and `.initial-files` through
`services/agent_filesystem/executor_directories/mod.rs`. The generic `host_directories` module owns
their filesystem operations and does not know about sandbox provisioning or filesystem-specific
quotas. Executor setup first opens and validates `storage_volume::FilesystemVolume`, then creates
the host directories and constructs sandbox provisioning on that shared volume.
On executor shutdown, `AgentFilesystems::track_shutdown` joins invocation loops and snapshot-store shutdown before
`AgentFilesystems::shut_down` removes both directories. The file loader stops new loads and drains
entered downloads and outstanding source leases before removing its cache. `Drop` remains a
best-effort cleanup safety net. That fallback is synchronous: without explicit shutdown, dropping
the last cache lease after its loader can remove the cache directory on the caller's thread.

Filesystem creation and source loads use `services/work_admission` registrations. Closing admission
refuses a new operation immediately and waits for existing registrations without holding a lock.
Source leases retain their registration, so a multi-file load interrupted by shutdown fails its
next source request as `InitialFileLoadFailure::Unavailable` instead of deadlocking behind its
own earlier lease. In-process executor `release()` waits for tracked shutdown cleanup as well as
the service graph. `shutdown_and_wait_for_invocation_loops()` keeps the invocation-drain timeout
separate from its tracked-cleanup grace. Only a successful return proves both completed. It does
not drop the service graph or release its root lock. A fixed-root replacement
must not start while old cleanup still owns the XFS lock or can remove its directories. A failed
invocation drain disables fallback removal of both host directories and retains their temporary
parent on disk. Production drain and cleanup share the existing shutdown grace, not a new budget.
Bootstrap owns a cancellation guard until it returns `RunDetails`. `epoch_ticker::EpochTicker`
owns its thread from creation, before snapshot binding or service construction can fail. Every
drop stops and joins that thread. An ordinary bootstrap error cancels only this executor's token
and waits up to `SHUTDOWN_GRACE` for tracked cleanup. It never shuts down the caller's join set,
which can also contain other services. A timeout logs that resources may still be held and returns
the original error. Neither that error nor cancellation of the bootstrap future is a completed
cleanup receipt. The cancellation guard exists for abandoned futures; normal error paths must
signal before awaiting their bounded cleanup.
Both failed bootstrap and process termination use `Shutdown::cancel_and_wait_for_tracked` for
the cancel, bounded wait and warning. The caller still owns termination of its server join set.

`storage_volume` owns shared storage roots, capacity observations, native cloning and XFS project
allocation and quota operations. Its XFS and APFS implementations do not construct sandboxes.
`sandbox_filesystem` owns one capability-confined tree, its handles, provisioning and verified
deletion, and seed/copy orchestration. XFS quotas account and limit allocation; root capabilities
confine path access. Physical pressure recovery observes the volume without accessing a sandbox.
Sandbox, volume and host-directory errors embed `operation_failure::OperationContext`. It owns
the operation, optional path and typed `OperationFailure`, plus their construction, formatting
and cause access. It carries no domain classification, cleanup flag or error-selection policy.
The domains own those decisions. Pressure validation returns its own configuration error. At service binding,
`FilesystemSetupError` keeps that cause distinct from storage failures; filesystem and active-agent
constructors use explicit `Result<_, FilesystemSetupError>` contracts. Task errors retain
`BlockingTaskError`; they do not acquire an invented I/O kind. Conversion to sandbox errors retains
these facts without classifying a storage failure as a sandbox escape. A primary host-directory
discard or preflight stale-probe removal is not a cleanup failure. Only the owning cleanup path
applies that flag. Volume probes, executor directory setup and sandbox creation
rollback retain both diagnostics when the operation and its cleanup fail, with the actual cleanup
failure's classification. A failed rollback deletion always sets `cleanup_failed`, even if the
lower-level error was unflagged. Worker startup then retains its cleanup obligation instead of
treating the result only as a creation error. Task failures have no I/O kind and remain terminal.

Plain construction uses `OperationContext` and `From` rather than domain constructor forwarders.
The sandbox I/O factory remains because it classifies capability escapes. `cleanup_failure::CleanupFailure<E>`
shares the flag, original-error chain and formatting without choosing when a domain marks cleanup.
Its type parameter preserves original error types. Conversions consume the chain and map its
original errors; they do not detach them through a mutator. Debug output uses the stored context
and chain directly.

Native cloning uses source-first arguments. APFS `clone_file_at` creates a directory entry;
XFS `clone_file_contents` fills an already-open target. The XFS project-aware operation keeps
the same source and target order. Tree-copy dispatch calls these operations without aliases or
argument-swapping wrappers.

`AgentFilesystemSnapshots::bind` requires a volume that makes copy-on-write copies. Production
uses Linux 5.8 or later with XFS and reflink, through `ManagedXfs` or `ReflinkXfs`. Local development
and tests on macOS use `Apfs`. Project quotas are not required for snapshots. `Temporary` and
`Directory` make byte copies, so binding `Enabled` on them fails at startup. The volume reports
`copies_on_write`; the snapshot service owns this rule.

`seed` and `copy_contents` make copy-on-write copies on XFS or `Apfs`, and byte copies on other
storage. `Apfs` makes the root as `Directory` does, then probes a file clone with `fclonefileat`
under it. A failed clone prevents startup. There is no filesystem type check, exclusive root lock
or volume identity check. Both case-sensitive and case-insensitive APFS volumes use `Conservative`
filename coordination. An entry edit waits for conflicting operations in the same parent directory;
two observations can run together. macOS keeps its native filename spelling, case sensitivity and
lookup behavior. Users do not normalize names. Case sensitivity does not prove byte-exact lookup,
because both APFS variants alias Unicode normalization forms. No special volume, disk image or
privilege is required. The mode logs that it is for local development and tests only.

On macOS, the shared `seed` path for `Apfs`, `Directory` and `Temporary` explicitly uses plain
native `fsync` for each seeded regular file, rather than `File::sync_all` for each file. The
directory scopes attempt the required namespace `fsync` calls before the final batch flush.
Each fresh macOS filesystem has one precreated, unlinked regular `seed_flusher` owned by
`NativeRoot`. Seed admission pins the root and `seed_flusher` atomically. Batch completion
attempts ordinary `fsync` on `seed_flusher`, ordinary `fsync` on the root, then `F_FULLFSYNC`
on `seed_flusher`. This includes empty, symlink-only and directory-only batches, and batches
that fail early. The finalizers attempt every required submission and keep the first error.
In unwind-capable builds, later required finalizers also run after a panic. The original panic
or an earlier error retains priority. Production panic-abort profiles do not run unwind cleanup.

`seed_flusher` creates no per-batch directory entry, so batch completion does not change the
root's modification time or require a writable root. A pathname replacement cannot redirect
the admitted descriptors. A setup or admission failure before pinning them has no admitted
`seed_flusher` to flush.

This policy applies only to macOS seed synchronization. Native cloning and byte copying are
unchanged. Linux seed synchronization and explicit guest flush operations keep their existing
behavior. XFS remains the validated, supported production copy-on-write path. Initial-file
installation still calls `seed` once per file. A seed batch does not combine separate install
operations. The policy does not guarantee seed, snapshot capture, blob upload or worker recovery
time. See
[macOS seed synchronization](../../../golem-worker-executor/worker-executor-walkthrough.html#macos-seed-synchronization)
for the internal policy.

`Apfs` states `AgentAccounting::Unaccounted`, as `ReflinkXfs` does. All disk limits resolve to
`Unlimited`; the mode has no project quotas or per-agent usage reports, and filesystem metering
is refused at startup. It reads volume space with `fstatvfs`, so startup capacity checks, capture
admission and write pressure recovery apply. APFS volumes in one container share free bytes,
so byte targets are approximate. Object counts are synthetic and very large, so object targets
always pass. `Temporary` and `Directory` do not use this pressure recovery.

On macOS, the local `golem server run` command defaults to `Apfs` at `<data_dir>/agents` and
`Enabled` filesystem snapshots. No filesystem TOML or environment settings are needed. The
`--agent-filesystem-root` option changes the default root. The filesystem settings are part of
the server configuration, as `worker_executor.filesystem_storage` and
`worker_executor.filesystem_snapshots` in the `--config` file or in the
`GOLEM__WORKER_EXECUTOR__` environment variables, and `cli/golem/src/config.rs` applies the platform
defaults as the lowest layer above the defaults. Standalone executors and local
servers on other platforms keep disabled snapshots by default. To choose `Directory` or `Temporary`
on macOS, also set `filesystem_snapshots.type = "Disabled"`. An agent still needs a snapshotting
policy. Without a configured `repository_key`, the local server generates a 64-byte repository key
and stores its 128 hex characters in `<data_dir>/filesystem-snapshots.repository-key`, with permissions
`0600` on Unix. Later starts use the retained repository key. A configured key takes precedence.
Without one, a corrupt retained repository key prevents startup and is not replaced. The snapshot
store uses the executor's blob storage, which the local server keeps at `<data_dir>/blobs`. There
is no separate snapshot destination.
See `docs/src/content/next/operate/filesystem_snapshots.mdx` for the local setup and
`reference/testing-patterns.md` for the native APFS checks.

With `filesystem_snapshots` set to `Enabled`, a snapshot record also names a filesystem snapshot
(`services/agent_filesystem_snapshots`). The snapshots belong to one agent incarnation: they are
keyed by `AgentSnapshots::agent(agent, fingerprint)`, and a fork target uses its stage id as its
fingerprint. `invocation_loop.rs::take_guest_snapshot` calls one function,
`worker/filesystem_snapshots.rs::periodic_snapshot`, which asks the service for admission, runs
the guest snapshot hook (`snapshot_guest`), captures the tree (`agent_filesystem::capture`), appends the
`Snapshot` entry, commits, and gives the capture to an upload job. The record has no name when the
tree holds only the initial files of the agent; the worker slot (`SnapshotSlot`) then keeps the
mark of that tree with no name. When the tree did not change since the last confirmed snapshot
that a start would select now, the record reuses its name and the confirmation comes with it
(`Snapshot` then `SnapshotConfirmed`); the record before it, which has its own application
snapshot, becomes the fallback. When the tree did not change since a record without a name that a start would select
now, the record has no name again: no store call, no confirmation, and no check of the
declarations and directories. A manual update uploads its filesystem snapshot before it writes `PendingUpdate`
(`worker/filesystem_snapshots.rs::update_snapshot`). When a
periodic upload of the agent runs, the update first stops the deletes that the running job makes
after its save (`retention_stop`, a child of the job's stop); the job still ends its save and its
confirmation. The update then waits until the job is gone or an admission can replace it, and asks
again; another refusal fails the update. The admission and the slot takes of its upload end
`confirmation_wait` after `admit_update` started, with `SaveRunning` or `NoSlot`; a run of the
upload that started before then runs to its end. A terminal interrupt ends that wait, or the upload
of the update, and fails the update; after a lost shard it writes nothing, and the update stays
pending for the new owner (`interrupted_update`). `SavedUpdate::delete_older_snapshots` runs after
`PendingUpdate` commits, and keeps every name that the status uses
(`snapshot_selection::names_in_use`: the two start candidates, the successful and the pending
updates, manual and snapshot-assisted, and the authoritative snapshot-assisted baseline);
`retained_update_snapshots` limits only the other update snapshots. Periodic retention keeps the
same names, so the filesystem snapshot of every successful snapshot-assisted update stays while
its `SuccessfulUpdate` is live, whatever its age.

An admission replaces a periodic job that has not decided and whose store call still holds the
save mark of the agent, when the call waits for its next run after a failed run, or waits for or
checks writes that can still land (`RunPhase::WaitingForLateWrites`, which the store reports through
`RunSlots::waiting_for_late_writes`). The new job starts at once. The replaced call runs on as a
tail: it keeps the busy count of the agent, starts no run and no write, never confirms
(`run_job` checks `ticket.replaced()`), and its capture is discarded after the call returns. A clean
successful save is never replaced, and neither is a job whose call has returned. A periodic
admission never waits.

`filesystem_snapshot/mod.rs` holds the store contract and `create_store` factory.
`agent_snapshots`, `snapshot_name`, `errors` and `run_admission` hold the shared caller vocabulary.
`operation_drain` tracks store-local operation cohorts by incarnation, not queued jobs.
Its `OperationLease` stays alive through retained tasks. `drain_before` establishes the
`delete_all` cutoff before admission and links each old cohort to its successor, so a later drain
still waits for earlier work when the first drain's caller drops.

The two store backends live in `filesystem_snapshot/backend/in_memory` and `backend/rustic`.
The in-memory backend remains available only for tests and `test-utils`.
`backend/rustic/mod.rs` owns the concrete rustic backend, construction, shutdown and operation
routing. Operations are free functions with explicit dependencies. `operation::Operation::new`
borrows the lease and a named `Resources` input without acquiring a tracker token or capturing
a runtime. The operation's fields are private. Its agent comes only from that lease, and `files`
accepts no separate agent. Accessors expose the key, clock and priority; task and backend methods
own registration. `BackendRun` keeps an untransferred backend before its cancellation guard in
field order. Transferring the backend into a blocking task retains no extra backend owner.
`Operation::read_existing` opens and reads a repository in one tracked blocking task. It uses the
caller's backend and starts no run or retry. Save keeps its cancellation link, per-run tracker and
late-write recorders in its own setup order.
`run_outcome` defines one `RunResult` for all operations: `Result<Ran<_>, Ended>`. An `Err` is the
only run failure path. An `Ok` holds an answer or settlement; settlement returns the same result
shape. `call_execution` drives runs and settlement. `clock` owns both wall and monotonic time.
`blob_io` is the single blob-call policy;
`blob_backend` bridges blocking rustic calls to it. `save`, `restore`, `snapshot_catalog`,
`snapshot_deletion` and `repository_namespace` own their operations. Save and restore also own
their operation-specific failure classification, settings and rustic options. `store_policy`
constructs `SaveSettings`, `RestoreSettings` and `PruneSettings` through matching factory methods.
Restore owns its reader-thread count by value. Test observations use the borrowed resource input.
`pruning` owns its defaults, options, eligibility criteria, durable records, claims
and timing. `tracked_tasks` retains the runtime supplied by each operation.
Tests and single-consumer fixtures live beside their owners. The `test_support` modules hold
shared fixtures; shared prune inputs live in `pruning/test_support`. The shared contract cases in
`filesystem_snapshot/tests.rs` register for both adapters and own their counting limiter. They
use independent tree fixtures. `filesystem_snapshot/time_zone_tests.rs` checks the executor image.

`StoreCalls` is the only owner of the store. The store takes a slot of its caller's limiter
(`RunSlots`, `max_concurrent_uploads` for the uploads and deletes) for each run of a call, and
holds none while it waits. A storage call gets a few tries in its run; after them the run ends,
and the store runs the operation again after a wait. `save` answers `Saved`, `NameInUse`,
`Source` (the save could not read the tree), `Failed` or `Stopped`, and checks its own name
itself. A write whose try was sent and failed in a
way that can still land is recorded; the call answers only after that write has landed or can no
longer land, one storage call deadline after its try. A save run ends by cancelling its run token
and giving its slot back; then it waits until rustic's threads release its blob files (a per-run
`TaskTracker`), reads its records, and waits for its late writes. Only a lost shard,
`BoundPassed` and a shutdown cancel a save run; a job stop and `delete_all_snapshots` leave it to
end as a tail. The discard of the capture and the confirmation hold no slot. A start waits only
for a job that has started saving (it got its first slot) and has not decided.

The deletes of the clean-up queue wait for the busy count of the agent to reach 0: a revert's
names, `delete_all_snapshots` and the stale-member deletes. The queue keeps one clean-up state for
each incarnation (`Names`, or `All`, which replaces pending names), and a fixed pool of
`max_concurrent_uploads` workers takes ready agents from a ready queue. Its bounds are 65,536
pending entries, 786,432 pending names, and `max_pending_deletes_per_agent` names for each agent; a
request past a bound is counted as a leaked clean-up. The bound of one agent applies only to the
names of a revert: retention deletes directly, and a delete of all snapshots carries no names. A
revert's names come newest first, so on a large revert the bound keeps the newest names and
refuses the oldest, which count retention or the delete of the agent removes later. At the bound
of the agents with pending work, a delete of all snapshots evicts the oldest waiting revert names
of another agent, counted as a leaked clean-up, and is itself refused only when no names are left
to evict. Retention and the superseded delete call the
store directly; they run after their own job's save answered. Two runs of saves of one agent never
run at the same time: a save call starts a run only while it holds the save mark of the agent. A
save call whose periodic job an admission replaced while the call only waited for its late writes
runs on as a tail that starts no run and no write, and the next save of the agent runs beside
it. The metrics are
`filesystem_snapshot_cleanups_pending`, `filesystem_snapshot_copy_seconds`,
`filesystem_snapshot_delete_seconds` and `filesystem_snapshot_prune_seconds`. `upload_now` answers `Stopped` after a lost
shard, a shutdown or `delete_all_snapshots`, unless the save had already finished; ephemeral
agents make no store call.

The upload job uploads the snapshot (`job.rs::upload`), then asks the worker for a confirmation
(`Worker::confirm_as`, with the pure `worker/filesystem_snapshots.rs::owner_gate`). The worker
appends `SnapshotConfirmed` only while the instance that took the snapshot runs, in one status job
that checks that the record is still the status candidate. Otherwise the answer is `Deferred`, and the snapshot stays in the
store. `Superseded` (an update or a revert replaced the record) deletes it (`delete_superseded`); `Confirmed`
deletes the older snapshots of its kind (`delete_older_snapshots`). A stop never waits for
an upload. A shutdown ends a job also during its deletes, and so does a call of
`AgentFilesystemSnapshots::delete_all_snapshots` for the agent of the job; a manual update of the
agent ends its deletes, and so does a revert, whose `RevertHold` (`begin_revert`) stops the deletes
of the jobs of the agent until the revert ends.

The next start confirms instead (`Worker::confirm_filesystem_snapshot_before_start`, called from
`WaitingWorker::new` before it takes permits, with `AgentFilesystemSnapshots::prepare_start` for
the wait and the store check). When the start would select the last record if it were confirmed,
it waits for an upload of that record on this executor, for at most
`confirmation_wait`. Then it asks the store once whether the snapshot is whole, for at most what is
left of `confirmation_wait`, or for at most `store_check_limit` when it did not wait. When the store
holds it, the start appends `SnapshotConfirmed` as the owner of the agent. A terminal interrupt ends
the wait. A start that finds no whole snapshot falls back to the previous usable record.

A loaded agent that restarts in place for an automatic update waits the same way before it ends
its generation, so that the strategy can select the newest record. The invocation loop asks
`Worker::upload_before_an_update` only when the final decision is `RetryDecision::Immediate`; a
`Delayed` retry does not wait. It gives a name when the head of the queue is an automatic update
without a strategy and the newest record that the update would select once confirmed is not
confirmed (`snapshot_selection::upload_before_an_automatic_update`). Then
`Worker::confirm_filesystem_snapshot_before_an_update` calls `prepare_start` for that name: it
waits for the upload for at most `confirmation_wait` and asks the store, or asks the store once
for at most `store_check_limit` when no upload runs. When the store holds the whole snapshot, the
running generation appends `SnapshotConfirmed` (`Confirmer::Running`). The wait ends at
`confirmation_wait`, at a terminal interrupt such as a lost shard, at a stop of the worker, which
closes its command channel (a revert and a delete stop it), or at a requested retirement of the
owner (`InvocationLoop::confirm_filesystem_snapshot_before_an_update`). The loop keeps the
commands that arrive during the wait for the next generation. The unload deadline of the restart
moves by the time of the wait, so the wait does not use it up.

`create_instance` restores the tree of the selected baseline. `StartFilesystem::load_and_plan`
plans the baseline with `plan_start` and makes its restore, and `StartFilesystem::materialize`
applies it to the new filesystem through `materialize_baseline`: the filesystem snapshot of a
named record, or the initial files of the replay revision for a record without a name. The
replay plan is selected before the filesystem is built. A
manual-update record without a name restores the initial files of the source revision when they
are all read-only, and then applies the initial files of the target revision.

A capture leaves out initial-file bytes only when every stored path component has the declaration's
exact spelling. Native alias lookup is not that proof. An APFS file or parent renamed to a Unicode
or case alias stays in the tree under its stored spelling; its declared path is not in `left_out`.
The check retains sorted, boxed names for each relevant parent within one fenced operation.
A separate positive byte-exact lookup proof avoids that name listing on validated XFS parents.
Canonical coordination labels alone are not a lookup proof.

An application snapshot is used only when the files of the agent can come back with it. With
`filesystem_snapshots` disabled, the admission answers `InitialFilesOnly`, and the snapshot checks
the tree in place of the capture (`agent_filesystem::check_initial_files`): the same fence and
the same rule as a capture, with no host directory, copy or store call. The tree holds only
initial files when each declaration is a read-only initial file that is untouched and has a single
name, every stored path component has the declaration's exact spelling, no entity-provisioned file exists, no call set a chosen modification time, and the tree
holds nothing else; a read-write initial file counts as a change. A rename or hard link of any
file, or a given time that the agent sets, counts as a change; a time that Golem puts back from a
recorded stat does not. A tree of initial files gives a record without a name. Any other tree,
or a check that cannot decide (a file call that stays open, a sandbox error), gives no periodic
record, so a start uses an older usable record or replays the whole oplog. A snapshot-based manual
update then fails as a failed update, and the agent stays on its revision: changed files give
`UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS`, and a failed check gives the details "failed to check the
agent filesystem for the update: <cause>", with no code (`UPDATE_CHECK_FAILED` is only the name of
the Rust constant). A periodic boundary that writes no record waits one period before the next attempt
(`invocation_loop.rs::snapshot_baseline_timestamp`). A save hook must not write files of the
agent: a boundary that writes no record is not replayed. A start from the initial files of the
source revision of a manual update counts as an install, not a restore of saved times
(`RestoreTree::gives_saved_times`), so the check after it can still find a tree of initial files.

`SnapshotBoundaryConditions` lists what blocks taking a snapshot: replaying,
snapshotting already, in-flight live host call, and open registered tool
attachments. Attachment producers, consumers, guest stdin-closure watchers and pending output
regeneration need the history that a snapshot would skip. Internal observers alone do not block
a snapshot. Automatic snapshots are revision-scoped and ignored once the revision changes.

Resetting a cursor is not recreating an instance: component metadata, revision and plugin context
come from instance creation. When history or provenance changes, go through the outer loop.

A revert may cross a completed manual snapshot-based update only when its cut is before that
update's `PendingUpdate`, so both the pending record and its `SuccessfulUpdate` are deleted
together. Assisted automatic updates follow automatic-update semantics instead: deleting a
successful or failed outcome while retaining `P` leaves the same frozen request pending and
retryable. A cut retaining assisted `U` retains promoted `S`; removing `U` removes only that
promotion contribution. A pending update without an outcome may be either retained or removed
entirely; when retained, its component and required snapshot payload are included in preflight.
Revert
validation reconstructs skip provenance: removing a migration
baseline must not remove earlier operator `Revert` regions, and the resulting mask is
also used to detect durable constructs spanning the cut. Before committing, the executor verifies
that the restored component, retained manual snapshot payload, replay metadata and initial files
are available. This is input preflight, not speculative replay; a later replay failure does not
undo the committed `Revert`. After the `Revert` entry commits, the revert deletes the filesystem
snapshot names of the dropped region that no live record outside it uses
(`reverted_snapshot_names`, `RevertHold::delete_snapshots`). A refused commit deletes nothing.

Guest `revert-agent` resolves a count-based target only after its call resolves as new work,
before authorization and `Start`. The permission resource is the resolved oplog index, and the
request records the resolved cut and observed target tip. Incomplete repair reuses that request
rather than resolving the count against newer target history. Resolution failures use the same
immutable rejected-call path as permission refusals: `Start.denial` freezes the error and `End`
returns it. Recovery of either a completed failure or a rejected `Start` without `End` never
resolves the target again or dispatches the revert.

Filesystem metadata has one deliberately deterministic exception to ordinary durable host-call
recording. P2/P3 `stat` and `stat-at` of Golem's file at the exact path of a **read-only** component
or pinned entity-activation initial file still execute the sandbox stat, but clear access and
modification timestamps and return without an oplog `Start`/`End`. Type, size, and link count
therefore come from the reconstructed filesystem, while the only volatile fields are removed. The
agent can delete or rename a read-only initial file and put its own file at the path, so the check
reads the object that the stat reads: it passes only when that object is a regular file without
write bits whose content equals the declaration (the file that an install recorded passes without a
read of its content, through the same `InstalledFile` identity and `initial_file_match` that the
initial-file rule uses). The answer depends only on the tree and the declarations, so a replay
chooses the same oplog shape. Mutable paths, aliases, a deleted initial file, and a file of the
agent at the path retain the ordinary `ReadLocal` durable path. An unexpected failure of the check,
or of the stat of an object classified as immutable, traps the invocation so a retry cannot choose
a different oplog shape. The host-call observation counter is still incremented on the successful
fast path. The check lives in `services/agent_filesystem/lifecycle/mod.rs::is_immutable_initial_file`;
the P2/P3 adapters live in `wasi_filesystem/{p2/types.rs,p3/mod.rs}`.

Filesystem permission decisions have a track in the oplog. Every protected P2/P3 filesystem import
records its permission decision in a normal durable call (`ReadLocal`), allowed or denied, so replay
returns the recorded decision and never checks a policy. Most imports record a separate
`filesystem::permissions::check` call before their effect; its request holds the verbs and the
guest paths, and its response is `Ok(())` or `Err(reason)`. A recorded denial returns
`not-permitted`. `stat`/`stat-at` (P2 and P3) check inside their own durable call and record a
denial as their `not-permitted` result; their immutable initial-file fast path records a separate
check. P3 `write-via-stream` and `append-via-stream` record the decision in their admission call
(`admit_stream_write`). The live check (`live_permission_decision`,
`start_authorized_access`) runs after the call resolves as new, before the call writes its
`Start`. A denied check records the denial in its `Start` (`start_live_denied`, or
`AccessClaimOptions::denial` in `run_deniable_filesystem_access`), so recovery of an incomplete
denied `Start` completes it with the same denied response and the protected effect does not run.
An incomplete `Start` without a denial was allowed, so its repair completes it as allowed without a
new check. A new check in the replay tail has no recorded `Start`, so the call switches to live and
checks the policy before appending its `Start`. P3 imports that await before their check reserve their invocation id first
(`reserve_invocation_id_before_await`). Test:
`tests/scope_cards.rs::denied_filesystem_imports_are_recorded_and_replayed_after_restart`.

RDBMS connection query-stream and transaction query, execute and query-stream authorize through
`start_with_options_and_authorize`, after replay and protocol readiness resolve but before `Start`.
Their denial builders record the same query-execution failure for all database engines.
`replay_or_persist_failure` restores a recorded denial before considering resource loss: an
incomplete denied call remains denied; an admitted incomplete call still reports transaction or
stream loss without repeating its database effect.

P2 TCP `start-connect` records its decision in the same way, in a
`sockets::tcp::permissions::check-connect` call (the request holds the remote address). A recorded
denial returns `access-denied`, and its replay does not connect. A denial is recorded in the check
`Start`, so the recovery of an incomplete denied check does not connect either. Test:
`tests/scope_cards.rs::p2_tcp_connect_denial_is_recorded_and_does_not_reach_the_backend_on_replay`.

P2 UDP records explicit remote-address permission decisions in
`sockets::udp::permissions::check`, for stream creation and addressed datagram batches. Its
request holds the addresses; replay returns the recorded decision without consulting current
permissions. Connected sends without an explicit address retain their stream's admission.

Filesystem permission failures carry their cause. `FilesystemStorageError::io` tags the synthetic
capability escape refusal as `SandboxEscape`; P2/P3 return `access`, or EACCES, and keep the
filesystem generation valid. Other `PermissionDenied` failures are terminal. The lifecycle can
return `not-permitted` without invalidation when a failed writable open rechecks a read-only regular
file, or a hard link fails with `PermissionDenied` or `AlreadyExists` and rechecks a directory at the
source without following the final symlink. The hard-link recheck holds the namespace edit lease, so a
coordinated replacement cannot change the source during classification. A terminal failure of that
recheck invalidates the generation through `classify_query_error`. A missing source, a non-directory
source or a nonterminal recheck failure leaves the original link error in control. A failed cleanup
prevents either policy recheck from replacing the storage failure. A failed writable-open recheck or
an object that does not establish its guest-policy refusal leaves the original permission failure
terminal. These rechecks use the current object and do not identify the object
that caused the native denial. See `services/agent_filesystem/lifecycle/mod.rs` and the permission
failure table in the executor walkthrough.

## Concurrency and guest completion delivery

Recorded async events reach the guest only at a blocked wait, at most one event per block,
live and replay alike. Native stream items and terminals (including producer cancellation),
timers, accessor completions and tool results share `RuntimeStore::await_guest_blocked` in the
consuming Store; an entity body uses its own Store, not the owner's. A tool result reaches the
guest through `golem::tool::internal::await-invoke-results` (`tool/execution.rs::observe_tool_invoke_plans`),
a guest-facing durable call that the host records around the result await; it is not a WIT import. Its `CompletionDelivered` marker records
the guest's receipt; the entity terminal records only how the body ended. The gate consumes one unconsumed
`DriverBlocked` edge, promoted only after the outer driver returns Pending. A delivery that
becomes ready after the guest blocked consumes that already-published edge immediately.
Waking the waiting host future invalidates the blocked generation, so checking the current
blocked level would deadlock. Non-blocking waitable polls and yields admit no recorded events.
A busy guest must ask through recorded synchronous host calls. Host-side takes and direct
non-accessor host returns do not use this gate. Native guest write completions are not
instrumented by this rule. Every bash script has a time limit and blocks with recorded
`wait-for(0)` every 1024 yields so a computing script can observe its stop request or time limit.

Before the blocked-wait gate, a replayed native read waits for every earlier retained Start of
its invocation, including host-internal descendants, to be claimed or adopted as replay-inert;
reserving an import's ordinal alone does not release the read.

`Accessor` host calls run concurrently inside one `Store`; direct `&mut self` calls retain
the Store while awaiting (`concurrent/mod.rs`). P2 wall-clock reads and monotonic `now` remain
exclusive because concurrent bindings cannot run during synchronous core initialization.
Fresh clock/random value reads in the primary Store skip live wallet synchronization: their
results require no permissions, and waiting for an accessor holding `card_event_boundary_lock`
would retain the Store that accessor needs. The explicit allowlist is in `call_coordinator.rs`;
`ReadLocal` alone does not imply permission independence. Snapshotting, retained recorded Starts,
entity Stores and replay still use the ordinary boundary. An automatic-update latch prevents
the exemption until update success is committed, including after the pending description is taken.
Clock-only execution does not guarantee pending card-transfer progress. Permission-sensitive calls
still synchronize. Replay-transition/cleanup lock contention is not eliminated by this exemption.
WebSocket connect/send/close use accessor bindings so their asynchronous work releases the Store.
WebSocket drop only removes local state and does not await boundary work.
Concurrent completions may finish in any host order, but the guest observes
them in exactly one order per run, and that order is recorded by the `CompletionDelivered` markers.
`ReplayDeliveryBarrier` transfers the cursor gate so replay releases each completion at its
recorded boundary. `supersede_prior_completion_delivery` hard-errors if an observer is still armed:
delivery must be the tail operation of a host function. Tests: `tests/concurrent_delivery_order.rs`
(bare Wasmtime: guest initiation order is stable across re-execution while host completion order
is not — the properties replay relies on), `replay_state` unit tests
`switch_to_live_wakes_parked_awaiter_as_incomplete`, `await_natural_tail_end_returns_once_tail_drains`.

For p3 HTTP sends, `TerminalConsumption::NotDelivered` is a positive guest
cancellation/abandonment signal. Store teardown and observer supersession suppress it. The send's
span-only cleanup appends a live `SpanClosed` hint when the send terminal did not already close
the span. Replay performs resident cleanup without claiming or waiting for a cleanup operation.
Network execution, call drop policy, terminals, and non-span cancellation retain their own
owners; collecting a response does not create a second durable call.

Spawned tasks may park across invocation settlement at guest-driven waits, passive markerless
replay-tail waits after durable finalization, or a destructor's passive wait for sibling history.
Cursor operations and recorded-marker waits stay active (`tail_work.rs`). A missing destructor
terminal can repair only after the natural replay-to-live transition. Recovery can return before
the parked Store task resumes; a subsequent invocation then drives the repair after its own
`AgentInvocationStarted`. The task restores activity before effects or terminal recording, so
that invocation's settlement waits for the repair. The original Start retains the effect's identity.
Test: `completed_invocation_cache_drops_repair_inside_next_invocation`.

A replayed websocket handle is reconstructed per handle while concurrent accessor calls race to
use it. `connect` on replay installs `WebSocketConnectionEntry::Replay(Arc<Mutex<()>>)` — the
per-handle reconnect gate — and every `send`/`receive`/`receive-with-timeout`/`close` on that
handle goes through `ensure_websocket_connection_live_access` in
`durable_host/websocket/client.rs`.
The helper takes the gate (racing the wait against the interrupt signal via `wait_or_interrupt`),
re-reads the entry *while still holding it* (`classify_reconnect_entry`), and only a call that
still sees its own gate in the entry applies the policy. With the default
`reconnect-automatically`, it proceeds to take one pool permit, run the handshake, and
publish `Live` in a single store window re-verified against the current entry. A queued peer
therefore re-reads the entry instead of independently reconnecting: a `Live` entry published by
the leader means it uses that entry and takes no permit, and a `Terminal` entry fails with the
recorded error. The handle's calls borrow the resource, so the guest cannot drop the handle while
one of its calls queues on the gate, and a fresh table after reconstruction discards the queued
call — a call holding the gate can only ever see its own replay incarnation in the entry, so a
different incarnation is the `InconsistentReplayGate` invariant violation, never a reconnect. A
failed handshake publishes its terminal through the same re-verified window
(`mark_websocket_reconnect_failure_terminal`): it only terminally closes the handle when the
entry still carries the same replay incarnation the failed attempt gated on, so a replayed
response that terminally closed the handle, or a concurrent call's live publication, owns the
entry's outcome and is never superseded. Without the gate, two accessors can each snapshot
`Replay`, each acquire a pool permit, and the leader's published `Live` connection then retains
the only permit while the follower waits on the pool forever; the gate bounds reconnection to one
in-flight attempt per handle without changing pool capacity or timeouts. An unpublished
`LiveWebSocketConnection` is dropped, releasing the permit and socket it held. Gate and pool waits
race against interruption, so a reconnecting peer never blocks suspension or explicit
interruption. Tests:
`tests/websocket.rs::websocket_reconnect_concurrent_receives_complete_after_reconstruction`,
`websocket_reconnect_coordination_{admits_one_accessor_reconnect,after_executor_restart,with_spare_pool_permits,is_per_handle}`,
`websocket_reconnect_coordination_is_interruptible_{while_pending_on_pool_permit,while_pending_on_handshake}`,
`websocket_reconnect_{direct_send_is_interruptible_and_continues,direct_close_reconnects_and_terminalizes}`,
`websocket_reconnect_failure_publishes_terminal_outcome`,
`websocket_closed_connection_stays_terminal_after_replay`.

The third optional WIT `connect` parameter is `option<reconstruction-policy>`. Absence selects
`reconnect-automatically`; `report-connection-loss` publishes terminal `SessionLost` at the first
live access to a reconstructed successful handle, without a pool permit or handshake. Recorded
send, receive, timeout, error, and close outcomes replay unchanged under either policy. Suspension,
restart, and crash all use this reconstruction path; the option does not change ordinary live
disconnect retry behavior. Live `close` on the lost handle reports loss without reconnecting;
resource drop releases it locally. Loss does not prove an earlier send never reached the peer.

An `End` result alone does not override an unfinished durable scope. Nonresumable operations
report typed loss at their live boundary; they do not rewind neighboring effects.

RDBMS stream `get-columns` returns `result<list<db-column>, error>` and `get-next` returns
`result<option<list<db-row>>, error>`. Only successful `none` is EOF. Recorded accessor outcomes
replay unchanged; an irrecoverable transaction or stream reports typed loss when execution reaches
its live boundary. Explicitly open a new transaction and reconcile uncertain external outcomes;
do not silently rerun the old transaction. Storage unavailability remains infrastructure recovery,
not a business loss result.

Guests that need protocol initialization should supervise session generations: one scope per
generation, reader started before initialization writer acquisition, reader failure observed or
raced during all writes, old generation cancelled and joined before explicit connect and
initialization of the next. Never forward the old writer's pending traffic automatically.
Rust takes `Option<golem_rust::websocket::WebSocketReconstructionPolicy>` and reports
`WebSocketError::SessionLost`. TypeScript uses `ConnectOptions.reconstructionPolicy` and
`WebsocketError.tag === "session-lost"`. Effect's `Websocket.isSessionLost` checks structural
`SocketError` / `SocketReadError` or `SocketWriteError` tags and the plain payloadless
`{ _tag: "SessionLost" }` in `reason.cause`. Explicit Effect writer close propagates loss and
fails the reader deferred with the same error; the scope finalizer remains best-effort.

## Streaming invocations

A streaming RPC is an ordinary durable RPC whose method carries input or output streams
(`remote_method_uses_streams`, `wasm_rpc/mod.rs`). Three facts prevent most mistakes:

- **Two owner-relative journals are authoritative, nothing else.** A producer records
  `StreamRegistered`/`StreamItems`/`StreamEnd`/`StreamCancel` in its own oplog. Its persisted
  `LocalStreamId` is the registration oplog index, not a globally meaningful handle. Every
  cross-record source is explicit `StreamRecordReference::{Local, Foreign}`; a received handle
  remains `Foreign` even when it happens to name the receiving oplog's owner. Consumer records
  store bytes or a terminal plus `consumer_read_ordinal`, `source_offset`, and the
  `LocalStreamReaderId { introducing_oplog_index, binding_slot }` that names the binding which
  introduced that reader. Nested registrations and the enclosing item record apply atomically.
- **Replay is owner-oplog-only.** A restarted consumer rebuilds bindings and observations from its
  own oplog in ordinal order. Replay performs no source RPC, authorization, or attachment; only
  the live suffix reads the source after the last journaled offset. An unread input may be
  forwarded as its original foreign handle without first attaching or consuming it.
- **A guest observation commits only past the published horizon.** Its owned write calls
  `DurableStreamStore::commit_through` with the reserved index. Every commit flushes the whole
  oplog buffer, so a durable agent's attached published status at or above that index proves an
  earlier commit made the record durable and folded it; the write returns its receipt without a
  commit. Ephemeral status (weaker levels fold without storage) and detached status prove nothing,
  so those writes always commit. The receipt still means durable and folded.
- **RPC result and stream draining are separate.** The caller's durable call completes with the
  result *stripped of streams*, so the RPC `End` may be recorded while items still flow.
  Streaming keys follow the RPC identity rule above. Terminals finalize once; protocol terminals fence
  later guest terminals. Terminal outputs reconstruct from committed records without reattachment.
  The persisted request also retains the original logical streaming origin, so retries and caller
  forks do not rewrite who originated the logical RPC.

Forks copy ordinary oplog entries and append a `ForkCut`. That marker clips retained stream history,
resets live controls, and stores the creation receipt; it does not carry handle aliases or authorship
mappings. Entity-invocation requests do not record the internal calling principal: live execution
and replay derive it from the current owner, including every middleware and leaf invocation.
Request matching still checks entity, operation, call mode, input and descendant plan position.
The separate execution `principal`, authority snapshot, guest inputs, recorded environment and
external effects remain unchanged. Inline request bytes are preserved; external payloads are copied
from authoritative contents under the target without decoding or re-encoding them. Cached external
values are not a substitute for copying durable bytes. Repeated forks derive the new owner without
rewriting historical guest observations or logical RPC origins.
Export forks also append `ExportForkInitialized`, which binds their new public session ID,
fresh invocation key and expiry policy. Revert raises the generation/epoch fence before reconstruction,
so handles issued by the discarded generation cannot control the rebuilt streams. Export targets
are built in hidden staged oplogs and published atomically; matching retries trust the immutable
target receipt while that target remains live. Do not model staging by adding provenance to stream
records.

The primary remains `ExecutionStatus::Running` after the guest returns while owned output
streams drain and invocation/session completion runs. `materialize_streaming_result`
(`worker/invocation.rs`) publishes the early result and preserves typed traps during production
and settlement. Both live execution and replay retain invocation-owned context spans until
materialization finishes, so output producers can read the current context and create child spans
after the guest export returns. Resident span cleanup still runs if materialization fails.
Suspension belongs to the outer live invocation or replay boundary, not the
guest-result boundary; interruption must still reach a producer that no longer writes to its
stream. Snapshot calls retain their own settled suspension boundary.

Tests: `tests/rpc.rs::durable_streaming_{output,input}_recovers_after_executor_restart`; full
mechanics and crash windows: `reference/streams.md`.

External Durable Streams use reader/writer resources in `durable_host/external_durable_stream/`,
not a new native stream source or session journal. Serialized `ReadLocal` constructors record
immutable descriptors and pinned secrets; replay validates and restores those descriptors.
Content-based resource identities survive snapshot initialization and exclude diagnostic secret
resolution timestamps, not pinned revisions. Resources own no cursor or producer progress.
Finite async reads record a complete payload/checkpoint as `ReadRemote`; appends record the
resource ID and immutable sequence/body/close as `WriteRemote`. SDKs own pending buffers,
producer progress and durable retry timers. Methods use exact request claims and the existing
cancellable completion-delivery boundary; dropping a resource only deletes its table entry.
Completed replay performs no HTTP or secret fetch. Golem forks retain external URLs and producer
identities; they do not issue DS-level forks. See `reference/streams.md` for protocol, auth and
memory boundaries.

The custom Durable Streams HTTP surface maps an opaque public session ID to a concrete invocation
key through `DurableStreamPublicBinding::{Live, Retired}` in the independently persisted session
index. A durable session creation uses a fresh UUID invocation key, so an expired public ID can be
recreated without reusing the old invocation. Ephemeral sessions retain public ID = invocation key
and are fail-stop, so recreation is rejected. `ExpiryRefreshed` and `Expired` are durable session
records; expiry scheduling uses `ExpireDurableStreamSession` fenced by agent fingerprint, invocation
key and expected deadline. Sliding refreshes are coalesced until they extend the deadline by at
least 10% of the TTL. Only a new origin GET and an accepted or duplicate append count as sliding
activity; HEAD, continuation reads (including long-poll/SSE), repeat PUT, and agent-side production
do not.

## Tool invocations and entity bodies

`durable_host/tool/mod.rs` implements `golem:tool/host@0.1.0`. Tool bodies (sidecars, middleware)
are *entity bodies*: guest code in its own Wasmtime `Store` with **no oplog or cursor of its own**
(`durable_host/entity.rs`). They record into the owner's oplog and share its `ReplayState`
cursor (`OwnerExecution`, `worker/instance/mod.rs`).

- `get_all_tools_model` and `get_tool_model` durably record a
  `SerializableToolDiscoverySnapshot`: the optional selected deployment revision plus ordered,
  per-import projected MCP observations, including empty tool lists and exclusions. Live selection
  chooses the latest deployment containing the running owner's component ID/revision, not the
  environment current deployment and not a permanent worker pin. Replay exact-rehydrates fixed
  definitions at the recorded revision (missing is terminal; transient registry failure retries
  the same revision) and reuses dynamic observations without MCP/OAuth. `None` and a selected
  revision with no dynamic observations are distinct. Native names, including unbound ones, are
  reserved before dynamic names; earlier imports win dynamic collisions.
- Dynamic MCP invocation is wired through a synthetic, filesystem-incapable native activation.
  Admission freezes the complete projected tool, protocol version, exact deployment/import source,
  binding and digest in the entity activation. The native body validates that projection, uses the
  executor's shared MCP transport, and records `tools/call` as `WriteRemote` with the ordinary key
  derived from its `Start`. Its encoded remote response is committed before result projection,
  stdout publication, or best-effort 401 feedback; completed replay is therefore offline.
- Dynamic discovery and admission use the shared middleware compiler with installations,
  environment/agent bindings and compatibility mode from the exact deployment snapshot.
  Discovery presents effective metadata without changing the lookup name. Admission pins the
  chain and unchanged MCP leaf projection in the ordinary entity plan. Incompatible refreshed
  definitions fail closed without selecting a later colliding import. Authority scopes intersect
  across environment and agent, then revealable secrets narrow to readable secrets. Explicit
  bindings, including all-keys bindings, are persisted and hashed; absent dynamic bindings deny
  config/secret access. Missing required middleware records fail rather than produce an empty chain.
- A remote `-32602` triggers a separate durable `ReadRemote` presence observation with a forced
  exact-source refresh. Quota suspension or a crash can repair that read while preserving the
  committed call. Present or
  observation-missing is `InvalidInput`; observed absence is `InvalidToolName`; MCP `isError`
  becomes a custom tool error. Fixed discovery exact-rehydrates its recorded deployment revision,
  while dynamic execution uses the source and full projection frozen at admission.
- `dispatch_tool_call` claims (replay) or creates (live) an `EntityInvocationDurability`, a
  `DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>` in the owner's oplog. Its
  `Start` index is the entity invocation id; body host calls carry `parent_start_index`.
- `InvocationExecutionMode` comes from `has_visible_terminal(start_index)`.
  `ReplayingCompleted` **re-executes the body's guest export** (`invoke_tool_sidecar`) with its
  durable host calls replayed from the owner oplog, then validates the claim
  (`StartClaim::owned_tool_invocation`) and releases the recorded terminal: no repeated external
  effect, but a second guest call. The body is re-executed rather than replaced by its recorded
  result because the entity Store shares the agent's filesystem (attached to the owner's
  `AgentFilesystem` generation via `OwnerRuntimeResources`): files the body wrote are agent
  state and must be reapplied for the agent's own replay to see them; `OwnerLane` therefore
  serializes filesystem-capable bodies. Exception: a terminal recorded with
  `body_execution: Skipped` is returned without invoking the export.
- `HistoricalReconstruction` fences keep the primary's `PendingReplayToLive` fail-closed until
  every completed body has validated (`primary_publication_requires_completed_reconstruction_validation`).
- `worker/invocation.rs::permanent_tool_guest_trap` isolates only its fixed Wasm
  instruction-trap allowlist. The tool terminal has `body_execution: Trapped` and
  `RemoteInternalError`. Typed quotas, policy exhaustion, `WorkerOutOfMemory`, infrastructure
  and lifecycle failures retain owner recovery even when the retry decision is exhausted.
  Physical Wasmtime Store destruction retains the original `Ctx` for cleanup. Supervisors
  independent of the caller Store cancel a failed parent's descendants while preserving selected
  outcomes and unrelated siblings. Replay re-executes trapped bodies and recorded host calls to
  reconstruct shared filesystem effects and partial stdout, then validates the exact terminal
  and descendants without repeating completed external effects. Stream completion is independent
  of the structured result; already-completed streams remain completed.
  Allocation-dependent `memory.grow` backing refusal returning -1 and leading to panic is a
  separate preexisting nondeterminism limitation. This contract provides no universal isolation
  guarantee and introduces no fork or snapshot recovery.
- Entity and native-tool invocation spans open on the invocation `Start` and close on its
  `End`/`Cancelled` terminal. `set_entity_invocation_scope` restores the recorded invocation
  context before the body runs; keep this reconstruction boundary covered.
- Every live admission, ambient or underlying, authorizes the command and arguments its layer
  receives, after replay claims and boundary projection; entity Stores authorize with the owner's
  generation-bound canonical live wallet and the caller's inherited invocation scope. Stores retain
  scopes and derived surfaces, not independent live membership. Accepted wallet commits publish
  through the status actor even if their producer is cancelled; owner-wide interest is the union
  of wallet, scope and admission-candidate contributions. Historical bodies retain recorded wallets
  and pins until their own live transition, never borrowing the latest live snapshot. Guest
  `self-wallet` and `self-version` reads are `ReadLocal` calls whose `End` records the exact
  observed view, so replay returns each read's own observation whatever card events it applies
  around it; an incomplete read completes live at its own `Start`. Card authorization decisions
  are recorded in the same way (see "Card authorization decisions"). The root `Start` records the
  pinned chain plan; descendants record root plus position, and every occurrence retains the
  original calling principal and typed static installation parameters. Component and host leaves use the
  same dispatch path.
- Middleware can start its next layer repeatedly and concurrently. Each result has independent
  `get`/`cancel`; dropping the observer does not cancel execution. Handler return revokes new
  admissions, while already admitted children remain owned until full operation settlement.
  A parent body terminal may precede child settlement so nested filesystem lanes can progress.
- Typed stream mappings are materialized durably while the producer/session remains alive, but a
  result is exposed only through the entity completion. Underlying stdout is an ordinary writer.
  Actual result awaits scope their causal lane edges, so later capable work cannot overlap a
  resumed caller.
  Store or executor loss stops resident drains without recording EOF; reconstruction resumes
  from durable input offsets. Normal settlement finalizes the session and cancels unread inputs.
- Entity admission does not modify history. Reconstruction repairs eligible incomplete calls
  at their own `Start`, preserving completed sibling work and committed outcomes. A nonresumable
  handle reports typed loss; application code must explicitly open and initialize another session.
  Loss does not prove that an earlier write failed.
- SDK sagas compensate and loop in guest code. New attempts append new calls and retain earlier
  attempts and compensations. After each fully compensated failed infallible attempt, resolve the
  policy with verb `saga`, noun the agent URN, and `attempt`/`error-kind` properties. The host's
  `next-retry-delay` uses the existing evaluator and an opaque binary continuation; SDKs pass it
  unchanged and use durable clocks and timers. No selected policy means the agent default.
  Unbounded policies are rejected with a typed policy failure, not an invented attempt limit.
  Count/time/never exhaustion returns a typed guest failure; selector/evaluator errors retain the
  original failure. Bounds keep attempts finite without rewinding history.
  TS, Effect, Scala and Go compensate caught failures in reverse order. Fallible helpers propagate
  the original exception/panic only after complete rollback; partial rollback retains both failures.
  Effect external cancellation stops retries and retains original and compensation evidence in
  `Cause`; self-interruption is policy-retryable. Rust WASM panic aborts without compensation;
  Result failures are typed, and `infallible_transaction` returns Result for compensation failure,
  policy errors and exhaustion. MoonBit exposes the retry API but has no public saga helper.
- Guests cannot disable recording, group effects into atomic regions, or get/set an execution
  cursor. External operator revert/fork remains available at validated cuts. `oplog-commit` is a
  replication barrier, not an external transaction commit or an undo operation.
- Startup seals deferred append admission for the old runtime and waits for accepted HTTP frame
  recordings and custom-call initiation coordinators. A transport-held old body cannot append
  after sealing. Destructive cuts also fence and drain the durable-stream producer, then reload
  it from repaired history. Tool failure election keeps driving Store tasks across secondary
  task errors until cancellation selections and lane drainage finish.
- The status actor retains the committed prefix preceding the current invocation in memory.
  Detached repair tries it through ordinary baseline validation before the persisted checkpoint;
  Revert invalidation or a missed receipt falls back without changing correctness. Retention
  adds no storage write or foreground wait; existing snapshot checkpoints remain unchanged.

Detailed mechanics, including scheduling and memory admission: `reference/tools.md`.

## External-effect boundaries

| Peer | Guarantee | Who enforces it |
|---|---|---|
| Durable Golem agent (RPC) | Exactly-once logical invocation and result | Target's durable queue + shared key |
| Ephemeral Golem agent (RPC) | At-most-once per accepted invocation; no resumption after target loss | Fail-stop target, deterministic phantom id |
| Oplog-processor plugin | Exactly-once delivery of each oplog batch | `services/oplog/plugin.rs`: deterministic batch key (`oplog_processor_idempotency_key`: source agent, grant id, first/last index → UUIDv5) and per-plugin `confirmed_up_to`/`sending_up_to` checkpoints persisted in `AgentStatusRecord.oplog_processor_checkpoints` |
| External HTTP/TCP/DB/etc. | Exactly-once only if the peer honours an idempotency key; otherwise retries in the ambiguous crash window are visible | Golem attaches an `idempotency-key` header to outgoing HTTP from `derive_idempotency_key(start_index)` (`http/policy.rs`, unless the guest set one) and offers `golem:api/host.generate-idempotency-key`; the peer must deduplicate |

Four exactly-once contracts coexist and must not be conflated: RPC exactly-once (one logical
target invocation per key), oplog-processor delivery (batch key + checkpoints), stream-item
exactly-once (each `StreamOffset` consumed once per consumer session, owned by
`durable_stream/mod.rs`/`durable_session.rs`), and exactly-once finalization (one terminal per
stream, protocol terminal fencing guest terminals). A change that
satisfies one does not imply the others.

## Permission-card handles

Persistent `derive` and `derive-from-wallet` return the host-tagged `persistent-card` sum:
`concrete` or `template`. Child slots name the eventual installed holder, not the caller.
A template parent must be in the caller's wallet and is resolved for that caller. Every possible
child instantiation must attenuate against that resolved parent. The stored card keeps its original
slots; the universal envelope is proof only. Explicit derivation inherits the exact parent
negatives and adds requested denials; wallet derivation takes complete bounds. Slot-bearing denials
cannot supply universal proof evidence; concrete inherited denials remain. `derive-scope` stays
concrete and rejects requested slots with `InvalidPattern` and an explicit message. A slot-bearing
tool owner does not resolve aliases against caller metadata; metadata-dependent short flags are
rejected instead.

A persistent permission-card handle (from `self-wallet`, `derive`, `derive-from-wallet`, or a
received value) caches no liveness. Every guest operation that resolves one — the `id`, `parents`
and `expires-at` accessors, `inspect-card`, `derive`, `derive-scope` and `install-card` — first
runs a `ReadRemote` durable call, `golem::permissions::types::card-liveness`
(`durable_host/permissions/mod.rs::resolve_permission_card`). Live, it asks the card service and
records `Live(card)`, `Revoked` or `Expired`; replay returns the recorded answer and never asks
the card service again. A failed lookup, or an answer that omits the card, traps and leaves the
`Start` incomplete, so recovery asks again; no error is recorded. The answer never depends on
wallet membership; the operation's recorded authorization decision holds the wallet data that the
operation uses. A revoked or expired card gives `card-revoked` or `card-expired`; the accessors
trap. `revoke-card` resolves its handle inside
its own recorded call. Runtime-card creation (`persist_runtime_card`) records a typed refusal in
its `End` when the card service refuses a child because a parent is no longer live. The card
service reports an expired card as revoked, so the parent's expiry decides between
`card-expired` and `card-revoked`. Creation repairs an interrupted call only while its repair
authorization still holds. Otherwise it revokes the card the interrupted attempt may have
created and records the refusal; the guest has not observed an incomplete call, so either
outcome is a valid first observation. The registry never creates a revoked runtime card id
again (`RuntimeCardCreation::Revoked`), so a later repair of a refused repair that failed before
its `End` records `card-revoked` instead of retrying. `install-card` of a template card records
the same typed refusals in its `install-card-transfer` `End`. Tests: `tests/scope_cards.rs::{cached_child_handle_reports_its_self_revoked_card_before_and_after_replay,
received_card_replays_its_recorded_liveness_after_an_external_revocation,
externally_revoked_cached_handle_returns_typed_errors_and_its_accessors_trap,
cached_handle_reports_card_expired_after_its_lifespan,
derive_from_a_card_that_expires_before_its_creation_records_card_expired,
a_refused_repair_that_fails_after_revoking_its_card_settles_with_card_revoked}`.

Source transfer progress is status state, not an oplog scan. The fold keeps
`AgentStatusRecord::source_card_transfers` (`worker/status/fields.rs::calculate_source_card_transfers`):
per transfer ID the first `CardDerived` (linked by its `transfer_id`), queued `TransferStarted`
payload, `CardTransferStarted` and `CardTransferConfirmed`, or `Conflict` when two records of one
kind differ. Every producer commits through the status actor, so the boundary's prepare and
confirm steps and the guest `install-card` path read `get_repaired_status()` and no oplog entry.
The index is sticky across skipped and deleted regions and keeps confirmed transfers. A repair
after `CardDerived` reuses the recorded child instead of creating it again. Tests:
`tests/api/mod.rs::pending_source_card_transfer_boundary_reads_no_oplog_entry`,
`tests/scope_cards.rs::template_transfer_repaired_after_its_derived_child_reuses_the_child`.

### Card authorization decisions

Card events are hints. When replay commits a recorded entry, `skip_forward` publishes every card
event up to the next recorded entry, and the next authority boundary applies them. A decision that
records nothing therefore sees on replay the card events that live execution applied only to a
later call. So `inspect-card`, `derive`, `derive-from-wallet`, `derive-scope` and `install-card`
each make their authorization decision in a `ReadLocal` call
(`permissions/mod.rs::record_card_decision`): `golem::permissions::inspect::inspect-card-decision`,
`golem::permissions::derive::{derive-decision, derive-from-wallet-decision,
derive-scope-decision}` and `golem::permissions::wallet::install-card-decision`. The request holds
the card ids of the handles. The `End` (`PermissionCardDecision`) records the outcome, allowed or
the typed permission error, and on success the serialized data the operation continues with: the
wallet copy of the `derive` parent, the selected `derive-from-wallet` parent, each `derive-scope`
parent (a valid scope card, the wallet copy of a persistent parent, or the refusal of an unusable
parent), and the `install-card` target with its holder identity. Replay returns the recorded
decision; the rest of the operation is deterministic from it and from the recorded `card-liveness`
answers. Each decision keeps its place relative to the handle's liveness call: `inspect-card`,
`derive` and `derive-scope` decide first, `install-card` resolves its handle first. A repaired
incomplete decision decides at its own `Start`. Tests:
`tests/scope_cards.rs::card_decisions_survive_reconstruction` and its cases.

## Wrong model → right model
| Wrong | Right | Fails under wrong model |
|---|---|---|
| "HashMap iteration / poll order makes the guest nondeterministic, so replay must tolerate different calls." | Guest inputs are recorded; same inputs ⇒ same calls. Different calls = executor bug. | `no matching Start` / `unexpected_oplog_entry` errors in replay tests |
| "Cursor reached the end, so I can do the live effect now." | Liveness is `store_is_live(...)`: the primary needs `switch_to_live` to publish after reconstruction fences; an entity Store needs its own `local_live_tail`. Cursor exhaustion is neither. | `pending_replay_to_live_is_fail_closed_until_finished`, `entity_store_liveness_is_scoped_to_its_invocation_mode` |
| "The voluntary-suspension predicate gates interruption or recovery." | It only defers proactive yielding while live work progresses; explicit interruption and arbitrary Store loss still use ordinary reconstruction. | Simulated-crash tests at arbitrary points (`simulated_crash`, `interrupt`) |
| "Restart differs from suspend." | Both discard the `Store` and reconstruct. | `counter_resource_test_2_with_restart` (state continues across an executor restart), `reacquire_permits_restart_preserves_accepted_queued_live_invocation` |
| "Losing a shard reconstructs the agent, like a restart." | It is retired instead: that generation is stopped without writing its status, dropped and never restarted. Only the executor now holding the shard reconstructs it — the new owner, or this one if the shard came back at a higher epoch. | `worker/mod.rs::a_failure_that_is_a_lost_shard_is_given_up_on_every_path`, `services/oplog/tests.rs::a_fenced_oplog_refuses_new_adds_and_keeps_the_indices_it_handed_out_readable`, the fence tests in `tests/indexed_storage.rs` |
| "A card handle can answer from its cached card, or from wallet membership." | A held handle outlives revocation and expiry, and wallet membership at an arbitrary point is not a recorded input. Each use records the card service's answer as `card-liveness`. | `received_card_replays_its_recorded_liveness_after_an_external_revocation` |
| "A retried RPC attempt executed the target again." | Same key ⇒ same target invocation; count target mutations, not attempts. | Provider-side counter tests in `tests/rpc.rs` |
| "Repair should generate a fresh RPC key." | Repair retains the call's own `Start` and derived key. A new saga attempt appends new calls. | RPC crash-recovery tests |
| "Equal return values prove deduplication." | Deterministic echoes are equal even with duplicate execution; count side effects. | Counter-based RPC tests |
| "A snapshot load is just a fast-forwarded replay, so its host calls are recorded/consumed like any other." | Snapshotting mode (`durability_is_suppressed`) neither appends nor consumes; the loaded state replaces skipped history from a revision-scoped baseline. | `tests/hot_update.rs::{auto_update_invalidates_snapshot_from_previous_revision, automatic_snapshot_invalid_entry_fallback_recreates_replay_context}` |
| "Spawned continuations can live in Store memory after the invocation finished." | Store memory is disposable; only oplog-backed work survives. | Restart tests after `AgentInvocationFinished` |
| "Rewinding the cursor recreates the worker." | Only the outer loop recreates metadata/revision context. | `tests/hot_update.rs::snapshot_after_auto_update_recovers_with_updated_component_context` |
| "The stream bus / socket is where stream items live; losing a reader loses items." | Producer oplog is authoritative; the bus is a live-tail optimization over committed records; consumers resume by offset. | `durable_streaming_output_recovers_after_executor_restart`, `callee_recovery_continues_output_after_committed_item` |
| "A stream reference stays valid as long as the target agent id exists." | Liveness is bound to the durable `AgentFingerprint`; a recreated agent is a different producer. | Fingerprint-mismatch unit tests in `durable_session.rs` / `durable_stream/mod.rs` |
| "A completed tool replay either skips the body entirely, or must redo its external effects." | The body's guest export is re-executed; its durable host calls replay from the owner oplog; the recorded terminal is validated and released. No repeated external effect. | `completed_tool_replay_bypasses_current_attachment_memory_pressure`, `deterministic_stream_crash_checkpoint_matrix` |
| "An entity body has its own oplog and cursor." | Bodies record into the owner oplog under `parent_start_index` and share the owner's cursor. | `concurrent_tool_attempt_identity_survives_reordered_admission_and_replay` |
| "`AttemptId::fresh()` in a replay-sensitive path breaks determinism." | Randomness is fine when appended and committed before observation and read back on replay. | Consumer-journal replay in `durable_session.rs` |

## Retries: in-function versus trap-based

A guest trap is permanent. Return an error for a transient condition and use a retry schedule;
application-level errors need application-level handling and do not automatically activate a host policy.
A Rust panic is permanent even in code that formerly used `atomically`; it does not retry the block.

The explicit `golem:api/host.trap` function wraps `wasmtime::Trap::UnreachableCodeReached` with
the guest-requested reason. It becomes `AgentError::DeterministicTrap` and is never retried.
This rule does not change the retry-policy handling of generic `AgentError::Unknown` errors.
Do not predicate retry policies on `is-idempotent`; the host no longer supplies that property.

A failed durable call has two recovery paths (`durable_host/durability.rs`):

- **In-function retry** re-runs the effect inside the same host call under the same `Start`.
  `InFunctionRetryState::decide_retry_with_properties` allows it only when the function type is
  `is_eligible_for_internal_retry` (`ReadLocal`/`ReadRemote`/`WriteLocal`/`WriteRemote`), the resolved
  `NamedRetryPolicy` still permits an attempt, and the delay is ≤ `max_in_function_retry_delay`.
  Each attempt appends and commits an `OplogEntry::Error { retry_from, .. }` hint.
  RDBMS transaction statements bypass this loop: transaction membership and statement retry
  refusal belong to the RDBMS protocol, independently of incomplete-repair authorization.
- **Trap-based retry** is the fallback (`FallBackToTrap`/`Exhausted` →
  `try_trigger_host_trap_retry`): the invocation traps, `on_invocation_failure` produces a
  `RetryDecision`, and the whole worker is reconstructed and replayed to `retry_from`.

For a selected policy containing a `TimeBox`, the persisted retry state also carries the
sequence's first-decision timestamp and a monotonic elapsed high-water mark. Policy evaluation
uses `max(previous_elapsed, now - started_at)`, so restart/replay and a backward wall-clock step
cannot reset the elapsed budget. The first decision sees zero elapsed, and the canonical
`elapsed >= limit` boundary gives up. Policies without a `TimeBox` retain their existing state.

In-function retry is the large optimisation over trap-based reconstruction; a new or changed
durable function must pick its `DurableFunctionType` with that eligibility in mind. Decision
table and tests: `reference/retries.md`.

## Review checklist for executor changes

1. Does every new nondeterministic host interaction go through `begin_durable_function` and a
   `DurableCallSession`, with a `DurableFunctionType` chosen for both its incomplete-replay
   re-execution and its in-function retry eligibility (the same predicate)?
2. Does the replay path claim by identity and fail on missing `Start` rather than skipping?
3. Is guest observation recorded (`CompletionDelivered`/`Discarded`) as the tail op of the host
   function, and is the replay boundary preserved?
4. Is any live effect gated on published liveness, not cursor position?
5. After an arbitrary `Store` loss at any point, is every obligation either in the oplog or
   deterministically recomputable from it? List the crash windows explicitly.
6. Is every identity that replay must reproduce derived from oplog indices, keys or *recorded*
   values? Randomness is fine only when appended and committed before the guest can observe it
   and read back on replay; unrecorded guest-observable randomness is a bug.
7. Does acceptance of remote work return only after `PendingAgentInvocation` is committed?
8. Does the change alter oplog shape? Audit every consumer — `is_hint()`, the WIT `oplog-entry`
   types, cut-point validation (`worker/cut_point.rs`), status folding (`worker/status.rs`),
   public oplog rendering. Reverted entries must not constrain the retained history's recovery.

## Debugging workflow

1. Get the oplog: test DSL `get_oplog` / `search_oplog`
   (`golem-test-framework/src/dsl/mod.rs`), or `golem-cli agent oplog`. Locate the last
   `AgentInvocationStarted`, then walk `Start`/`End`/marker pairs.
2. Classify the failure:
   - `unexpected_oplog_entry` / no matching `Start` → determinism break. Find which host input
     differed (missing recording, wrong request-identity match, delivery at wrong boundary).
   - Hang in replay → an awaiter waiting for a terminal that is not in history or a delivery
     gate never released; check `ReplayDeliveryBarrier` and `switch_to_live_wakes_parked_awaiter`.
   - Live effect during replay → liveness read from the wrong place; check `store_is_live`.
   - Duplicate side effect → key derivation or acceptance ordering; compare keys across attempts.
3. Reproduce with a real reconstruction (`simulated_crash`, or executor drop + restart via
   `start_with_overrides`), injecting the failure at the exact window with
   `TestExecutorOverrides` — see `reference/testing-patterns.md`. Never a cursor reset.
4. Fix the recording/reconstruction bug. Do not add tolerance to replay.

Use the `testing` skill to run tests and `debugging-hanging-tests` for stuck runs.
