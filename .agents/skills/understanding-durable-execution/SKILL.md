---
name: understanding-durable-execution
description: "Explains how the Golem worker executor implements durable execution: oplog recording and deterministic replay, durable host calls and guest completion delivery, the replay-to-live transition, the invocation queue and results, durable RPC exactly-once, streaming invocations and durable streams, tool invocations and entity bodies, snapshots and updates, and interruption, suspension, and reconstruction. Use when changing, reviewing, or debugging golem-worker-executor code or tests that involve replay, oplog entries, crash recovery, idempotency keys, or worker lifecycle."
---

# Understanding Durable Execution

The worker executor runs WASM agents whose progress survives the loss of the process running
them. Everything below is verified against `golem-worker-executor/src/` and `tests/`; when code
and guide disagree, the code wins and the guide needs a fix. The scoped `AGENTS.md` files under
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
   is a strict divergence error, except for entries deleted by a `Jump`). The axiom is about
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
   (replay, transport retry, atomic rollback) share a durable key. An executing target persists one
   invocation and result; read-only cache hits/followers persist neither. Attempts are not executions. Owner:
   `durable_host/wasm_rpc/mod.rs`
   (`derive_idempotency_key(begin_index)`), `worker/mod.rs::enqueue_worker_invocation_with_effect`
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
| `BeginAtomicRegion`/`EndAtomicRegion`/`Jump` entries (they mark skipped history; existing entries keep their indices) | Atomic-region logical idempotency counter (rebuilt from those entries during replay) |
| Agent status **folded from the oplog** (`worker/status.rs`) | Status cache / checkpoints (baselines only) |
| Persisted promises and scheduler actions (`WakeupScheduler`), `RunningWorkers` recovery index | Tokio timers, in-memory wake channels |

Anything in the right column may be rebuilt from the left column at any time; a design that needs
something from the right column to survive a restart is wrong. Sockets and other OS resources are
recreated, not preserved (`durable_host/sockets`, `durable_host/http`); the application protocol
must tolerate reconnect. Durable liveness also needs *rediscovery*: a timed wait schedules its
wakeup as a persisted scheduler action first (`durable_host/suspendable_wait.rs`,
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
incarnation and removes only an exact stale member.

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

The check sits at the commit, not at the effect. A call whose `Start` is only buffered when its
effect runs — an idempotent `WriteRemote`, which opens no committed scope — can still run on an
executor that has just lost the shard; its commit is then refused and the new owner runs it
again, the same window as a crash before that commit. Non-idempotent, batched and transactional
calls commit their scope `Start` first, so a refusal stops them before the effect. Oplogs that
assert no epoch are never refused: a handle opened before this executor has an assignment, and
fork stages and their publication. The archive transfer is fenced like the primary oplog (see
"Resharding, revocation and the oplog epoch fence" below), and so is an ephemeral agent's oplog,
which writes only through the compressed archive levels. Two writes stay outside the fence: the
blob archive layer, because blob storage has no conditional write, and blob uploads of large
payloads.

`worker/state_actor.rs::commit_and_update_state` samples the appended tip before its explicit
commit and ignores receipt entries already folded into the published status. Primary/ephemeral
threshold flushes and replica waits can commit outside the status actor, so even an empty receipt
may hide a committed suffix. Ephemeral threshold flushes hand batches to a bounded asynchronous
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
| Host context | `durable_host/mod.rs` | `prepare_instance`, `resume_replay`, `switch_to_live`, `PendingReplayToLive`, idempotency-key derivation, snapshot boundary checks |
| Tail work | `durable_host/tail_work.rs` | Keeps store tasks running until no durable work is active before `AgentInvocationFinished` |
| RPC | `durable_host/wasm_rpc/mod.rs` | Key derivation, first dispatch vs `MayExist`, replay claims, ephemeral phantom identity |
| Worker | `worker/{mod.rs,invocation_loop.rs,lifecycle.rs,instance.rs,status.rs}` | Queue persistence, dedupe, interrupt/resume/update decisions, eviction, retry decisions |
| Cut points | `worker/cut_point.rs` | Preserves atomic/transaction outcomes; ordinary calls recover from the exact retained prefix |
| Durable streams | `durable_host/{durable_stream/mod.rs,durable_session/mod.rs,stream_session.rs,stream_bus.rs,stream_transport.rs,schema_value_stream.rs}` | Producer-oplog stream records, consumer session journal, exactly-once item delivery, protocol terminals |
| Tool invocations | `durable_host/tool/{mod.rs,streams.rs,boundary.rs,operation/mod.rs,attachment.rs}`, `durable_host/entity.rs`, `worker/{entity_slot.rs,owner_lane.rs,entity_invocation.rs,instance.rs}` | Outer-surface authorization, pinned middleware plans, typed boundaries, owner-oplog entity bodies, shared replay state, operation settlement |

## Worker lifecycle and reconstruction

`Worker` (`worker/mod.rs`) receives invocations, persists them, and processes them in order. The
run loop (`worker/invocation_loop.rs::run`) is:

```diagram
┌─────────────────────────────────────────────────────────────────────┐
│ outer loop (one iteration = one resident instance)                  │
│  monthly admission ──▶ create Store + instance ──▶ prepare_instance │
│    durable agent:  handle PendingUpdate ▸ try snapshot ▸ resume_replay│
│    ephemeral:      replay typed initialization only, append Restart │
│  ──▶ inner loop: pop durable queue, run invocation, persist Finished │
│  ──▶ instance ends (idle / interrupt / trap / suspend)               │
│  ──▶ RetryDecision: Immediate | Delayed | ReacquirePermits | None | TryStop │
└─────────────────────────────────────────────────────────────────────┘
```

`resume_replay` (`durable_host/mod.rs`) loops `get_oplog_entry_agent_invocation_started`, replays
each recorded invocation in `InvocationMode::Replay`, and when there is no further
`AgentInvocationStarted` it switches to live. Ephemeral owners replay at most one recorded
`AgentInitialization`, and only when their resolved owner is a typed agent. Component-baseline
tool owners instantiate the deployed component but never queue or replay an agent constructor.
Replay starts from the chosen snapshot baseline
(see Snapshots and updates), not necessarily from `OplogIndex::INITIAL`. Interruption kinds
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

Before extending quota interruption to pending host adapters, follow
[reuse-first resource-limit delivery](../testing/reference/resource-limit-delivery.md).
Inherit Worker acceptance, typed signals, invocation-loop cleanup and ordinary reconstruction;
record independent defects and unverified combinations separately from the bounded delivery claim.

Monthly policy uses the shared account `AtomicResourceEntry` in `services/resource_limits.rs`.
Registry revisions arrive through the existing refresh path; stale refreshes must not restore
older policy. Admission in `worker/invocation_loop.rs` checks the latest cached capacity before
instance creation, replay resumption, and invocation. Reconstruction must pass those checks too;
a resume or queued invocation cannot bypass an exhausted applicable cap.

During guest execution, `Context::ensure_fuel`, the `FuelManagement` implementation in
`workerctx/default.rs`, calls `ensure_monthly_resource_capacity` to check compute when enabled,
then monthly memory and the agent's matching storage class. `worker/instance.rs` calls it from the
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
refresh within five minutes. Excess accrued under the applied `allowOverage` revision remains
billable until `hardLimit` applies; earlier `hardLimit` usage never becomes billable retrospectively.

Disabled meters omit their monthly accounting, caps, overage, and billing. Filesystem metering
controls both storage meters but their amounts and usage remain separate. Per-agent memory and
managed-XFS quota enforcement remain independent; unmanaged storage has no finite per-agent quota.

A pending invocation still holds its allocated linear memory and allocated filesystem storage
while waiting for host I/O. Those allocations occupy real capacity that other agents cannot reuse
while it is held. Memory and storage therefore accrue byte-time during permit-held host I/O waits
even when no Wasm executes, as well as during replay. The memory meter measures actual allocated
linear-memory bytes, not the configured maximum memory limit. Fuel measures instruction consumption,
so the parked guest itself burns no fuel.

The concurrent-agent permit defines the billing window. Permit release settles and pauses metering
before publishing reclaimable `LoadedIdle` or `WarmRunnable` state. This ends the billable window by
design; it does not mean every cached allocation is physically freed immediately. Released-permit
durable sleep, reclaimable cache time and unloaded time do not accrue memory or storage byte-time.

`worker/monthly.rs` attaches one monitor to an enabled, permit-held execution window. It checks
local capacity on a normal 30-second cadence and on applied Registry updates, settling owner-scoped
byte-time first and reading published fuel reservations without locking the Store. Before each wait,
it settles that usage and wakes earlier when exact remaining monthly memory byte-nanoseconds divided
by the primary runtime's current allocated linear-memory bytes predicts exhaustion sooner. The
horizon rounds up to a nanosecond; zero allocation, exhausted or inapplicable memory caps retain the
normal cadence, including rejected-stop retries. This owner-rate estimate is not a global account
prediction or a universal interruption deadline. It neither scans other
account owners nor changes Registry refresh cadence. All-off windows have no monthly monitor or
capacity subscription. Window closure invalidates its target and joins the monitor and registered
stop cleanup. During unload, the filesystem drains before its final allocation read. That allocation
is frozen while deletion runs; enabled memory and storage meters continue until actual permit
release. Failed deletion retains the permit and frozen window with the filesystem's explicit repair
obligation. A successful repair settles through release without reading the deleted filesystem.
Ordinary startup rollback after a window opens uses the same unload path. This does not cover
independent startup panic or cancellation: the filesystem and execution-window Drop fallbacks
have separate physical tasks, and the panic notification does not join them.

Initial idle settlement precedes startup readiness. Monitor, accepted-stop driver and settlement
errors at either idle boundary survive successful filesystem deletion as `CleanupFailed`; they
neither request immediate reconstruction nor acquire a replacement permit. The monitor treats a
closed lifecycle channel or lost send as an infrastructure error, retains that health, and requests
a cooperative Worker stop. If status work then panics because the actor has stopped, the invocation
loop retains its resident agent across the borrowed inner-loop failure and runs ordinary unload
without submitting more actor work. Tests in `tests/api/monthly_health.rs` cover both idle boundaries
and a pending silent-TCP invocation. Other panic paths still use the outer panic boundary.

Worker stop registration and window closure share a synchronous admission lock. Closure seals
its accepted drivers and installs a retained successor. Later accepted drivers wait for the old
permit's physical release, including any retained filesystem repair, before touching the runtime.
New startup and resident-generation publication join prior drivers outside the instance lock and
recheck under the acceptance lock order. Loading never clears unfinished work or retained errors.
Owner retirement closes further stop admission before joining the last drivers.

An accepted stop in `WaitingForPermit` takes that attempt's exact task handle before signalling
Loading subscribers. The driver cancels and joins the task without acquiring its blocked permit,
then settles the matching startup tracker and `WorkerLoaded` event before resolving the existing
stop receipt. Restart and Jump preserve pending work without lifecycle or invocation-finished
markers and request ordinary reconstruction. Dropping the receipt does not cancel the driver.
If core initialization stops before a prepared Store exists, the invocation-loop-owned Worker
stop acknowledges the published interruption under the lifecycle lock. No Store callback remains
to send that receipt. The real initializer tests in `tests/api/monthly_preparation.rs` verify
cleanup and one receipt, durable Suspend and reconstruction, and an ephemeral typed resource
error recorded as Recovery before any guest invocation starts.

The monitor submits a proposal to the existing Worker state actor. Worker acceptance revalidates
the cached owner, fingerprint, startup attempt, resident generation, held permit, exact active
window, policy revision, period, fuel generation and current exhaustion. Acceptance registers
retained progress before any signal. Restart/Jump remain replaceable until the retained driver
or terminal invocation-loop teardown freezes the queue's elected cause under the interrupt guard.
Teardown registers its terminal demand through the same Worker election. Admission-driven
`TryStop` uses its actual Suspend timestamp rather than synthesizing an Interrupt. An earlier
accepted monitor stop wins even if teardown reaches the owner fence before its driver. Both use
the retained frozen cause; only the driver publishes it. Later accepted proposals join that
publisher; they cannot publish a provisional kind. The driver fences the exact owner
with the frozen kind, verifies the physical owner-failure winner, then publishes that kind to
existing Loading/Running subscribers and the late-subscriber projection. A conflicting independent
owner failure remains authoritative and fails stop cleanup rather than becoming a quota error.
The invocation loop remains the sole primary-Store owner. Tests in
`tests/api/monthly_admission_owner_election.rs` gate a real Context Worker's loaded Compute
admission teardown and monitor driver in both fence orders, then verify one pending invocation
executes after physical unload and reconstruction.

Ordinary nonterminal unload closes tool/entity admission and drains physical ownership without
electing a synthetic lifecycle failure. The operation owner retains that closed admission separately
from its first selected Trap, Infrastructure or real Lifecycle cause. A later stop during unloaded
OOM backoff can therefore elect its exact accepted kind without clearing the old physical fence,
cleanup or selected invocation outcome. The exact owner-kind guard still rejects real conflicts.

Monthly acceptance rejects further proposals once a terminal stop is claimed, under the existing
interrupt and admission locks. The same active window can keep accounting during retirement, but
later ticks or applied updates cannot replace the accepted monthly kind/reason or queue another
terminal demand. This also preserves a claimed user stop. Generic terminal queuing still permits
later deletion. `tests/api/monthly_p2_input/repeated_stop.rs` holds a real outcome writer after
selection, checks tick and changed-resource update rejection, then verifies one terminal, physical
cleanup, durable continuation and ephemeral archival.

Outcome selection shares the interrupt guard. Success, a typed invocation-deadline failure and
a lifecycle candidate wait for an unpublished terminal stop accepted before selection; the selected
failure keeps the typed interrupt and applicable ephemeral monthly error. The primary invocation
loop passes `InvocationFailureOrigin::InvocationDeadline` from `InvokeResult::Failed.timed_out`
and `Lifecycle` from `InvokeResult::Interrupted`, including monthly admission's Suspend. A frozen
explicit Interrupt therefore cannot lose the invocation marker to a later admission Suspend.
Neither classification depends on error text. Independent guest traps, owner infrastructure failures
and tail-work failures keep their original classification. Tests in
`tests/api/monthly_lifecycle_regressions.rs` gate the frozen publisher against real monthly admission
and verify that a consumed Suspend cannot survive joined unload into the next activation. A timeout selected before stop acceptance remains authoritative.
Real Worker tests in `tests/api/monthly_deadline.rs` expire the actual invocation timer while
silent TCP receive is pending and the accepted stop driver is held before publication. A result
selected first retains its writer completion through the existing commit receipt and waiter
notification. A later stop waits for that completion before fencing or signalling, and cannot
write a competing result. Durable success still uses `Always`; ephemeral success still uses
`Deferred`, without waiting for the status fold. If the selected writer's own terminal append or
commit receives `OplogError::Fenced`, it explicitly acknowledges that refusal to its writer barrier
without publishing a success or failure result. Selection stays authoritative, and ShardLost
retirement still joins physical cleanup before removing the exact old generation. A missing receipt
or panic remains a lost-writer error; a latched lost-shard reason never erases cleanup health.
`tests/api/fenced_outcome.rs` gates both selected success and guest failure through epoch takeover,
physical release, old-generation removal and higher-epoch regrant. Tests in `tests/api/monthly_cause.rs`,
`tests/api/monthly_cutoff.rs` and `tests/resource_limits/stop_cause.rs` gate publication and
selection on real Workers, including silent TCP and a blocked concurrent-agent permit.

The default 10 ms epoch increment still does not wake pending host futures. Real silent-TCP tests
cover monthly-memory detection by both the local tick and applied Registry update, cooperative
socket closure, durable reconstruction and ephemeral resource failure. The P2 TCP input
`subscribe().block()` tests in `tests/api/monthly_p2_poll.rs` cover memory, prepaid compute and
mode-matching scripted storage in both modes. They verify physical unload and permit release
before peer readiness, then repair the original poll Start for durable agents. Scripted allocation
proves Worker orchestration, not native managed-XFS measurement. These tests do not establish a
bound for other host waits; preserve the permit-owned billing window.

P2 TCP `start_connect` re-executes natively on reconstruction. A completed connect poll must drive
the new native connect future before returning its recorded readiness, otherwise synchronous
`finish_connect` can return `WouldBlock` and diverge. `TcpConnectReplay` in
`durable_host/sockets/tcp.rs` classifies only socket pollables with an unfinished connect.
`durable_host/io/poll.rs` revalidates their recorded-ready `poll` or `ready` result without changing
the oplog. Only the native TCP readiness wait selects the existing typed interrupt signal, after
completed replay has closed the durable handle. Replay resolution, the recorded terminal and
file-readiness revalidation stay outside that race. Late subscribers read an already published
stop from `ExecutionStatus`. Tests in `tests/api/monthly_p2_poll.rs` gate both replay entry points
before subscription, while waiting and after readiness selection; they retain the original input
Start across another monthly stop and recovery. Holding the Store at that gate proves the permit
is still held after signal delivery, before physical unload. The gate controls native readiness
delivery, not a real network timeout. Failed native reconnect outcomes remain a raw-P2 replay
limitation; readiness does not guarantee the original successful connection result.

Successful pollable drops clear classification. Native socket deletion requires all pollable
children to be dropped first, so socket cleanup need not scan unrelated subscriptions. This does
not revalidate HTTP, UDP or TCP input pollables. Exported P2 `pollable.ready` remains a nonblocking
live probe, as verified by `tests/api/p2_ready.rs`.

### Blobstore and filesystem guest observation

Live blobstore provider futures in `durable_host/blobstore/{mod,container}.rs` select the composed
`create_interrupt_signal()`. Provider completion has priority when both helper branches are ready.
An interruption abandons only the incomplete durable call and propagates the typed trap. Retry
policy, End persistence, metrics, resource registration and accessor delivery remain outside the
select. Container creation and its subsequent metadata lookup are separate waits. Buffered value
consumers, size/drop and materialized object names do not acquire a new backend wait. Dropping a
provider future does not undo a remote effect before End; existing incomplete-call replay semantics
still apply.

P2/P3 filesystem descriptor calls select guest observation of each admitted operation. Open selects
its attributes and open calls separately. Stat interruption escapes as a typed trap before error
serialization; completed stat history and replay timestamp restoration remain authoritative. Direct
P2 file read/skip, flush readiness, file-only blocking splice and completed-record file readiness/read
revalidation use the existing file maps. Live multi-poll retains its existing Worker subscriber.
Mixed network/unknown streams keep their own paths.

P3 file prefetch, subsequent producer demand and result-future observation use the composed signal.
An interrupted read traps instead of reporting EOF. Streaming chunk effects and task completion are
not selected. `FilesystemCall` Drop keeps already-started work and its generation lease under the
filesystem module until completion. Guest observation can therefore stop while flush or write still
runs; unload must drain that work before deletion and actual permit release. Known-local synchronous
syscalls cannot be preempted inside a future poll, and started blocking tasks may delay drain. Cleanup
failures and unknown writers remain observable, not successful release.

`tests/api/monthly_blob_filesystem.rs` verifies representative monthly memory in both agent modes.
Its blob override observes real provider Pending and stays withheld through physical release. Its
keyed filesystem gate observes admitted stat waiting before native execution, not a stalled native
syscall. Durable cases retain and complete the original Start under the same invocation key after a
grant; ephemeral cases persist the typed monthly failure. The lifecycle test
`interrupted_guest_observation_retains_started_call_and_drain` holds a started scripted flush and
proves deletion waits after typed observation interruption. These are separate observation and
physical-drain witnesses, not a native-XFS stall or universal release bound.

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
`tests/api/monthly_frontend_streams.rs` observes a real native-tool stdin reader's first Pending,
then monthly memory Suspend, joined physical unload and permit/monitor release while stdin stays
quiet. The original accepted session and invocation key subsequently consume their first packed
bytes and finish normally after a grant. This is representative durable monthly-memory evidence,
not an ephemeral frontend matrix, CLI log-persistence stall, arbitrary-stop guarantee or universal
publication/cleanup release bound.

Raw P2 TCP `blocking_read` and `blocking_skip` in `durable_host/io/streams.rs` select the existing
typed interrupt signal around only the native input future. Successful `finish_connect` and
`accept` classify the returned input stream; successful native stream drop removes that rep.
File streams use the separate filesystem observation path above; unknown streams keep their existing behavior. Native stream cancellation,
resource deletion and invocation-result commit remain outside the select.

These raw TCP calls have **no durable host Start or recorded read/skip result**. At a pending
stop, the connect poll is already complete and the method remains unfinished. Reconstruction
reconnects and re-executes input under the retained invocation key, not a repaired stream Start.
Even completed historical raw input executes again when a fresh invocation reconstructs an
unloaded Worker. Deterministic peer data is required to reproduce the recorded invocation result;
this does not provide exactly-once TCP consumption.

`tests/api/monthly_p2_input.rs` covers each export with memory, prepaid compute and matching
scripted storage in both modes. Its Worker-local test observer reports the real native future's
first `Poll::Pending` and tracks return versus drop without gating readiness or locking the Store.
Tests keep the peer silent through monthly stop, physical unload and permit release. Durable
cases retain 2 Started/1 Finished, then reach 2/2 after reconnect and one-byte input. The separate
completion tests let native input finish, hold the existing selected-result boundary, then accept
a monthly stop. Commit and notification win before stop publication. They also reconstruct
completed history and count the repeated native input before fresh work. Other TCP output and
UDP waits are separate operations, not covered by these input tests.

Raw P2 TCP `output.blocking_splice(&input, 1)` is a distinct export whose native provider first
checks output capacity, then waits for input and finally writes and flushes output. Successful
TCP `finish_connect` and `accept` register input and output reps independently; successful stream
drop removes only that stream's classification. When both reps are registered raw TCP streams,
the native splice selects the existing composed `create_interrupt_signal()` through
`interruptible_tcp_input`. Input and output need not share a socket. HTTP batched calls,
filesystem readiness, unknown outputs, durable settlement and resource deletion stay outside
this select. Raw read/skip retain their separate signal construction.

`tests/api/monthly_p2_input.rs` and `monthly_p2_input/cross_socket.rs` observe the actual splice
future's first Pending for same-socket and cross-socket pairs. A positive native `check_write`
probe runs only when the test observer is installed. Fresh output capacity and a silent source
support input-demand attribution through provider ordering; the provider-internal input await
is not instrumented. Both configurations exposed accepted-stop physical-teardown failures.
Each now has memory, prepaid-compute and matching scripted-storage cases in both agent modes.
They check socket closure and joined Store/permit/window cleanup before any input byte, durable
same-key reconnection or ephemeral terminal/probe, and completion-first cached success. The
cross-socket tests require both sockets to close and later count a byte consumed on A and emitted
on B. Memory accounting uses a drained seed and isolated target account interval, not a per-worker
billing key or an exact parked-duration charge. Scripted storage is not native managed-XFS evidence.

`durable_p2_cross_socket_splice_stop_before_subscription` holds a keyed test-only gate immediately
before the composed constructor. A real monthly-memory stop is accepted, frozen and published
while the Store, window and permit remain held. Releasing the gate allows the latched signal to
interrupt, followed by physical cleanup and ordinary same-key reconstruction. The unbiased select
may skip native polling or poll Pending before dropping; neither order is a ready/stop tie proof.
This control covers durable monthly memory only. Deadline/entity sources are composed in code,
but separate guest deadline/entity cancellation tests remain unproved.

Like raw read/skip, splice has no durable host Start or recorded byte result. Completed guest
history can consume and emit TCP bytes again; only a same-key result-cache lookup stays offline.
These tests do **not** prove output back-pressure, post-consumption flush cancellation,
transactional byte effects, accepted/half-dropped socket combinations or external exactly-once.

A P2 HTTP response-body `blocking_read` is different from raw TCP input: it opens a
`HttpTypesIncomingBodyStreamBlockingRead` child under the batched HTTP request scope. Only its
live native read selects the typed interrupt. An interrupted native read leaves that child
incomplete without a `Cancelled`; reconstruction retains the scope and Jump-replaces its
contents, reissues the HTTP request with the same idempotency-key header and reads the first
byte again. The native select excludes response-body resumption, retry policy, durable
completion, stream drop and request-scope finalization. Inline resumption classifies errors where they arise: retry-delay interruptions and typed
native traps abandon the child with its original error chain. Payload download, request
reconstruction, resource-table failures and non-interrupt native traps use the child's
call-owned infrastructure trap, leaving its Start incomplete. A 416 response, a short
resent prefix or native prefix `LastOperationFailed` instead completes the child with
recorded `LastOperationFailed` before returning that stream error; completion failure
propagates. The resent response is prepared and its prefix skipped before either guest
body or stream is replaced. The inline retry delay, live response-body resend
`future_resp.ready()`, and matching full-response prefix-skip `new_stream.ready()` listen for
the typed interrupt. Each ready select prefers native completion on a tie. An interrupted
prefix skip drops the detached body without replacing the original guest body or stream.
Retry policy, response classification, prefix read, durable completion and request-scope
settlement remain outside those ready selects. `tests/api/monthly_p2_http_body.rs` observes the second native read's actual Pending
after a first byte and keeps the first response silent through physical stop.
`tests/api/monthly_p2_http_retry_delay.rs` instead closes the partial first response, observes
the actual retry sleep's first Pending with the same read child and invocation, and stops before
any second request. It tests both modes under memory, prepaid compute and matching scripted storage,
plus a completion-before-stop case where the sleep returns before the second request. A two-slot
pool permits durable repair. Account-level positive memory updates after the seed are not attributed
to the target because the seed's sub-unit remainder may flush later; the closed-window zero-delta
check remains separate. `tests/api/monthly_p2_http_resend.rs` ends the partial first response,
observes the second request and its native resend-ready Pending with two HTTP pool slots, then verifies
physical stop and socket closure while the second response headers remain withheld. Its
completion-before-stop control releases the second response while the same native ready future
is Pending and verifies normal End and Finished with no monthly stop. `tests/api/monthly_p2_http_prefix.rs` receives the second
Range request, sends a matching full 200 response's headers, then withholds its first body
frame while the actual prefix-skip ready future reports Pending. It checks physical unload
and socket closure while that frame remains withheld; its completion control releases the frame
first and observes Returned,
settled child/scope and Finished. All four suites cover memory, prepaid compute and matching
scripted storage in both modes; they count physical reissues, not exactly-once remote effects.
Direct P2 HTTP response-body `blocking_skip` also opens a `WriteRemoteBatched` child under
that request scope. Its live native wait selects the typed signal with native completion first
on a tie, abandoning only the skip child on interruption. Replay retains the single scope,
Jump-replaces the interrupted children and sends a new HTTP request with the same key.
`tests/api/monthly_p2_http_skip.rs` observes the actual skip Pending after a completed first-byte
read, keeps the second byte gated through physical stop, and verifies completion-first return
separately. The one-slot pool self-wait is separate GOL-685. Status-code retry resend-ready, background
resend-ready, nonblocking body reads and upload capacity remain separate boundaries.

The three durable-memory `stop_before_subscription` tests in the resend, prefix and skip suites
park the real live operation just before its existing Worker-backed typed subscription. One
`test-utils` latch matches both invocation key and exact operation, reports the open child Start,
stream rep and permit-held monthly runtime, and leaves the native future unpolled while parked.
Resend has already sent request 2 but receives no response headers; prefix has received matching full
200 headers but no second-response body frame; direct skip withholds the first response's
remaining byte. Each test verifies the open child under the original request scope, the peer
attempt count and held permit, then accepts and freezes the matching monthly Suspend with the
owner before releasing the latch. After release the native-first select can poll the real
future once to `Pending` before it reads the latched stop. The observer may therefore report
no Pending event, or one matching event ending in `Dropped`; `Returned` would be wrong. A
selection receipt confirms the actual typed interrupt branch. Physical unload, socket closure
and permit release occur while the peer still withholds the response headers or body bytes. The
child has no invented End or Cancelled; the method has no Finished until a new request supplies
data and durable reconstruction Jump-repairs that child under the same scope and invocation key. These three tests cover **durable monthly memory** with publication
before subscription. The separate six-cell matrices cover already-subscribed native Pending
waits in both modes under memory, prepaid compute and matching scripted storage. Completion-first
controls let native work finish before a stop; none of these cases asserts a simultaneous
native-ready/stop-ready tie.

P3 UDP `receive` in `durable_host/p3/sockets/udp.rs` selects the same typed signal only
around the native live receive. `run_read_access` leaves an interrupted
`P3SocketsTypesUdpSocketReceive` Start incomplete; its call-owned trap context preserves the
`InterruptKind` root cause. Replay resolution, End persistence, accessor delivery and native
bind/drop remain outside the select. The call remains `ReadRemote` and repairs its original Start.

`tests/api/monthly_p3_udp.rs` covers explicitly bound, unconnected receive in both modes with
memory, prepaid compute and matching scripted storage. A test-utils-only observer reports the
real receive future's first Pending and native local address, then distinguishes return from drop.
The guest binds loopback port zero and never reads that varying port. After physical unload,
rebinding the observed address proves socket retirement; UDP has no EOF. Reconstruction binds
again and the test sends one datagram to the newly observed address. The original Start gets one
End and delivery, and the retained invocation finishes once. Idle eviction then forces completed
history to replay with the sender gone. The next native receive belongs only to fresh work.
A completion-first control verifies End and delivery before a monthly stop, then preserves the
selected invocation result through commit and notification. These tests do not establish
exactly-once datagram consumption across the receive-before-End crash window. Connected UDP
reconstruction and incomplete batched sends remain separate, unresolved contracts.

WebSocket initial `connect` in `durable_host/websocket/client.rs` opens its `WriteRemote`
Start before acquiring the executor-wide connection-pool permit. The live pool acquisition
selects the existing typed interrupt signal without a Store lock. An interrupted acquisition
abandons the call for a trap, leaving its Start incomplete; it does not append `Cancelled`.
Only successful acquisition transfers the permit to the live socket. The subsequent handshake
has its own interrupt select; neither call completion nor resource-table registration is raced
with this pool wait. `tests/api/monthly_websocket/pool.rs` holds the sole pool slot with a
completed, loaded-idle Worker whose monthly permit and monitor have been released. A second
Worker parks on initial connect with no peer connection or receive Start. Six resource/mode
cases stop and physically unload that target before evicting the holder. Durable reconstruction
repairs the original connect Start and invocation key after the slot is freed; ephemeral failure
remains terminal. Peer handshakes and frames are counted as attempts, not external exactly-once
effects.

The initial `connect_async` handshake begins after a free pool permit is acquired. A passive
observer in `tests/api/monthly_websocket/handshake.rs` reports its first native Pending while an
in-process TCP peer holds back the Upgrade response. Six resource/mode cells stop the Worker,
close the half-open socket and return both permits while 101 remains withheld. Durable replay
repairs the original Connect Start with a new handshake; ephemeral failure is typed and a fresh
Worker probes the released pool. A completion-first control lets 101 arrive before exhaustion.
The handshake already selected the typed signal; these tests did not expose a production RED.

Completed durable Connect replay instead restores a `Replay` connection handle without a TCP
connection. A fresh Receive accessor or direct Close opens its own Start, then acquires a pool
slot before reconnecting. `tests/api/monthly_websocket/reconnect_pool.rs` holds the only slot
with another loaded-idle Worker and observes the *target's* native acquire Pending. Both helper
acquisitions select the composed typed signal; the direct helper formerly waited without a
signal. The holder stays loaded until the stopped target physically unloads. After holder
eviction and capacity grant, the original Receive Start gains End then CompletionDelivered, or
the original Close Start gains direct End, with one new handshake and no historical replay
traffic. `tests/api/monthly_websocket/reconnect_handshake.rs` instead starts with the pool free
and withholds the third TCP Upgrade response after the target's actual reconnect `connect_async`
reports Pending. The typed stop closes that socket and frees the slot before the peer supplies
101; recovery repairs the same operation Start on a fourth TCP connection. Both helper paths
have durable memory, prepaid compute and scripted durable-storage cases and completion-first
controls for each wait. The direct post-permit handshake also uses the composed interrupt signal.
Completed-connect replay is not available to ephemeral agents, which cannot resume or invoke a
reconstructed incarnation. Entity-local cancellation, a native ready/stop tie, timed receive,
reader/writer locks and send/close transport back-pressure remain unproved by these matrices.

A timed WebSocket receive is another distinct frame read: `live.reader.lock().await` completes
before `reader.next()` is wrapped by the timeout and raced against the composed typed signal.
`tests/api/monthly_websocket/timed_receive.rs` observes the *native read's* first Pending after a
valid Upgrade while the peer withholds its frame, matched to the timed Receive Start, invocation
key and runtime. The guest's 300-second deadline is not driven by the monthly test clock. Six
resource/mode cases check stop and physical socket/Store/permit/window release with the frame
still withheld, durable same-key repair of the original timed Start and ephemeral typed failure
with a fresh Worker probe. A separate one-second timeout-first control returns `None` before
any monthly exhaustion, then preserves its cached result. The existing frame select already
interrupted; no production RED or timeout/stop tie is claimed. The reader mutex acquisition
**precedes** that select, so these tests do not cover lock contention. Scripted storage proves
Worker orchestration, not native managed-XFS measurement.

A second timed receive on the **same** guest WebSocket can reach the reader mutex while an
untimed receive holds it in its native frame read. `tests/api/monthly_websocket/reader_lock.rs`
observes the timed `reader.lock()` future's first Pending after both distinct Starts; six
resource/mode cells prove **stop-only** physical cleanup with both frames withheld. The first
interruptible read releases its guard on an owner monthly stop, so the missing select around
the second lock await has no demonstrated normal-Worker RED. A separate control shows the timed
deadline begins only after lock acquisition. This is not completed coverage: durable same-key
repair of the two incomplete reads timed out because both replay accessors may independently
reconnect the saved handle through a one-slot pool; one holds the slot while the other remains
in the acquire branch without rechecking Live. This failed continuation is tracked separately as GOL-693, Backlog/Medium. It is not passing
recovery evidence or a repair gate for the pending-host quota extension. Preserve that limitation;
do not add a second pool slot merely to make this replay test pass or claim an invented terminal.
Entity-local cancellation and the writer mutex are distinct.

Explicit interruption retires the cached owner and fences its replacement startup. Automatic
shard-assignment recovery leaves `Interrupted` workers stopped, even with queued invocations or
updates; an executor restart or shard move is not a request to resume them. If the worker
has already unloaded (for example during OOM backoff), the retiring owner must commit an unclaimed
terminal interrupt and notify invocation waiters before removal; no Store remains to do it. A
terminal interrupt already claimed by the invocation loop is not recorded again, and a completed
or failed invocation is not overwritten. A claim retains its exact request, so teardown cannot
requeue that same cause. A distinct terminal following a completed Restart keeps its own queue
demand while the original publication stays Restart. Normal loop stop or joined owner retirement
commits the unclaimed status marker once, including an outer-loop concurrent-permit wait with no
new Store or permit. A Suspend already committed by admission is not appended again. Loop stop
never claims a terminal merely to discard it. Tests:
`tests/scalability.rs::interrupt_during_oom_backoff_is_durable_before_restart` and
`tests/resource_limits.rs::concurrent_agent_limit_restarted_waiter_stops_without_permit`.

`recover_immediately` selects `Restart` for Running, Suspended and Retrying workers. It never
turns a simulated crash of a parked worker into a permanent interruption. If no invocation loop
remains, the existing promise, scheduler or permit wakeup starts reconstruction; the queued
restart does not fail the invocation waiter or append `Interrupted`.

Environment and application deletion invalidate component metadata, environment state and agent
type caches before awaiting owner retirement. New metadata lookups then observe deletion instead
of admitting requests against a retiring cached owner.

External metadata observation uses a fallible FIFO status read. If the actor has stopped, the
lookup takes the cold oplog lifecycle guard, verifies retirement, joins writer completion, and
reconstructs from persisted metadata and the oplog. A failed deletion can therefore leave a
cached but stopped worker observable until removal is retried. Missing storage means absence;
failed reconstruction means an error, never a stale cached status or permission to restart work.

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
An archive read needed for replay still fails recovery rather than being treated as absent data.
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
interrupt, delete, resume, update, revert, and plugin changes never create an absent agent. Delete
is owned by that worker: concurrent callers share its retained attempt result, a later call retries
only unfinished cleanup stages after failure, and successful cleanup retires active-worker and
open-oplog cache entries only for the generation being deleted. A stale `Arc<Worker>` therefore
cannot continue deletion against, or evict cache state belonging to, a replacement with the same
`AgentId`.

The bounded unload result and final cleanup completion are separate facts. An unload timeout
permanently fails that deletion attempt, while module-owned cleanup continues. A later explicit
delete joins the retained completion, or retries a failed filesystem deletion through the owning
filesystem generation. Durable storage and cache authority remain fenced until verified cleanup
succeeds; old attempt handles retain their original error. Cleanup with no verifiable
owning-component repair remains a failure: successful filesystem deletion cannot erase an
unverified metering settlement, including `ObserverLost` during startup rollback.

Create, open, archival, fork-source reads, and deletion share the logical oplog's exclusive cold
lifecycle guard. Fork reads persisted source history without constructing an absent source;
the complete hidden stage is published atomically into an absent target under its lifecycle guard.
Source and target guards are never held together, and publication releases the target guard before
resuming the child. Cancellation or failure may leave an unreachable hidden stage; cleanup removes
only that stage's index and never target payloads or canonical state. Archival is routed through the
existing worker owner. A scheduled archive releases the
guard once its transfer is queued, while the oplog sweep holds it until the transfer finishes. No
lifecycle lock is taken for individual stream items, oplog reads, or replay steps.

`Oplog::stop_and_wait` closes admission and joins work associated with the actual open oplog
generation, including tasks belonging to older worker shells removed from the active cache.
Transport roots are cancelled and their children joined without waiting for client IO. Invocation
loops are joined, not cancelled: their final commits, state destruction, and panic cleanup must finish.
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
terminal even when the immediate cause is a payload download failure, because recovery has no
compatible replay fallback. The
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
- **Archive transfer.** Opening the layered oplog records the owner's epoch on every compressed
  archive level's own key (`services/oplog/compressed.rs::CompressedOplogArchive::opened`) before
  the archive watermark is read, so an older owner's transfer either landed before the record,
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
  replaces a primary one. The blob archive layer has no conditional write and stays outside the
  fence.

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
before removing anything, and deletes the oplog after everything it can rebuild. See `crash-matrix.md` for the fence's
failure modes and `services/active_agents/mod.rs` for the sweep that retires agents on an
assignment change.

## Oplog model

Entries are positional or hints (`OplogEntry::is_hint()`). Replay consumes positional entries in
order and skips hints. Key kinds:

- `Start { parent_start_index, observational_owner, request, span_started }` paired with
  `End { start_index, response, span_finished, span_attributes }` or
  `Cancelled { start_index, partial, span_finished }`. A call is identified by
  its `Start` index; `End`/`Cancelled` is its *terminal*. A terminal is *visible* when it lies
  below the replay target and outside a `Jump`/`Revert`-skipped region
  (`has_visible_terminal` / `visible_terminal_record`, `replay_state/cursor.rs`). Request-less Start/End pairs are *scopes* (batched writes, transactions),
  named `<scope:batched-write>` or, when the caller supplies a discriminator,
  `<scope:batched-write:DISCRIMINATOR>`; a discriminated claim matches only its exact name and
  never falls back to a plain sibling scope (`execute_access_scope_start`, `concurrent/call.rs`).
- `AgentInvocationStarted` / `AgentInvocationFinished` bracket one invocation;
  `PendingAgentInvocation` (hint) is the durable queue entry.
- `CompletionDelivered` / `CompletionDiscarded` (hints, always after their `End`) record the
  guest-observation boundary of an accessor completion.
- `HostStreamFrame` (hint): a frame of a host-owned stream (e.g. a p3 HTTP request body) attached
  to its owning call by `parent_start_index`; consumers find frames by scanning, interrupted
  recordings need no closing entry.
- `BeginAtomicRegion` / `EndAtomicRegion` and `NoOp` are positional; `Jump` and `Revert` are hints.
  Entity-local atomic rollback can append Jumps at the live tail during replay, outside the
  discontiguous regions they delete, so the cursor must skip those markers without a guest claim.
- `PendingUpdate`, `SuccessfulUpdate`, `FailedUpdate`, `Snapshot` (hint).
- Lifecycle hints: `Suspend`, `Error`, `RecoverySucceeded`, `Interrupted`, `Resumed`, `Exited`,
  `Restart`.

Hints are skipped by `skip_forward` (the physical cursor moves past them, but
`last_replayed_non_hint_index` does not), take part in no `Start`/terminal pairing, and never
satisfy a claim. `Jump`/`Revert` do not relocate entries: they mark a region as skipped or deleted,
and the atomic-region logical counter is rebuilt from them (see RPC section).

Span lifecycle is metadata on ordinary durable operations, not a second positional protocol.
There are no standalone `StartSpan`, `FinishSpan`, or `SetSpanAttribute` entries. Replay first
claims the operation by its normal function/owner/request identity, then the owning host path
restores `SpanStarted` and applies `SpanFinished` or successful attribute updates. Span ids,
timestamps, parents, and links never select a claim, and the generic cursor performs no
span-specific terminal-tail consumption. Long-lived resource and guest spans therefore open on
one operation and close through a later short local operation; a span lifetime never keeps the
opening durable call or an atomic lease open.

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

Every nondeterministic host function goes through `begin_durable_function` /
`end_durable_function` (`durability.rs`). Concurrent (p3 accessor) calls run inside a
`DurableCallSession` (`concurrent/call.rs`); the serialized p2 path uses
`persist_durable_function_invocation` / `read_persisted_durable_function_invocation` with the same
`Start`/`End` shape.

Live (accessor path): append `Start` eagerly → run the live action → append `End` (or
`Cancelled`) → hand the result to the guest → append `CompletionDelivered` (or
`CompletionDiscarded` if the guest dropped the completion unread). The serialized direct path
has no markers: its result is delivered when the host function returns. A trap while a call is
in flight leaves `Start` incomplete (`abandon_for_trap`); a trap never writes `Cancelled`.

Replay: claim the matching `Start` (`StartClaim`, identity + optional request payload match),
resolve its terminal through `ConcurrentReplayResolver`, then classify with
`classify_replay_resolution` (`concurrent/call.rs`), which is total and shared by every path:

| Recorded | `ReplayedResolution` | Guest handoff |
|---|---|---|
| `End` + `CompletionDelivered` | `Delivered`, `AtMarker` | released exactly at the recorded marker boundary (`ReplayDeliveryBarrier` holds the cursor gate) |
| `End`, no marker (accessor) | `Delivered`, `AtReplayTail` | crash after host completion, before observation: withheld until the cursor drains (`await_natural_tail_end`, `replay_state/resolution.rs`), then delivered live-armed; the effect is **not** re-run |
| `Cancelled { partial: Some }` | `Delivered`, `Immediate` | guest-initiated, deterministic drop point; no gating |
| `Cancelled { partial: None }` or `End` + `CompletionDiscarded` | `Undelivered` | the guest never saw a value; the future is parked for the guest's deterministic drop |
| `Start` without terminal | `Incomplete` | see below |

**Custom durable invocations** (`golem:durability/durability.begin-custom-durable-invocation`,
`durability.rs::begin_custom_durable_invocation`) wrap a whole guest block as one logical durable
operation. Completed: the recorded root result is returned and the body does not run (nested
entries are consumed as a replay-inert subtree). Incomplete: the block goes live under its
original root `Start` and the **whole body re-runs**, re-recording nested calls as new physical
`Start`s. This is the one ordinary durable path where a completed nested effect legitimately
repeats; the block author owns its idempotency (`reference/timelines.md` §15,
`tests/durability.rs::custom_durability_crash_mid_live_invocation_reexecutes_whole_body`).

`Incomplete` (`prepare_incomplete_live_repair`): the handle switches to live completion of the
*existing* `Start` — no second `Start` is appended — if `can_reexecute_on_incomplete_replay`
(`durability.rs`): `ReadLocal`, `ReadRemote`, `WriteLocal` always; `WriteRemote` only while the
guest's idempotence mode is on (`assume_idempotence`, default `true`);
`WriteRemoteBatched`/`WriteRemoteTransaction` never. Otherwise the session
hard-errors and the enclosing durable scope's `ScopeReplayRecovery` decides whether recovery is
possible; a hard error by itself is not a recovery.

Host completion time and guest observation time are different facts. Only the second is a guest
input, and only the second must recur exactly; the first may vary between runs.

Fork and revert retain the exact inclusive prefix. A cut between `Start` and its terminal uses
ordinary incomplete-call recovery; a cut between an accessor `End` and its delivery/discard
marker uses `AtReplayTail`. No outcome beyond the cut is inherited. Revert leaves deleted entries
physically present, so completion-marker discovery excludes deleted regions before checking
uniqueness. A replacement marker for a retained `End` then survives the next reconstruction;
two visible markers for one `Start` remain corruption. Atomic-region and remote-transaction
outcome cuts are still rejected. Test: `reverted_completion_marker_can_be_replaced_and_reconstructed`.

## Replay cursor, claims and strictness

`ReplayCursor` (`replay_state/cursor.rs`) is the single chokepoint: `try_get_oplog_entry` reads
speculatively, `commit_consumed_entry` commits, `move_replay_idx` advances and emits
`ReplayFinished` exactly once. Concurrent accessor host calls may append `Start` entries in
scheduling order that replay does not reproduce, so `claim_start_matching` claims the *first
unclaimed matching* `Start` between cursor and target. That is the only justified use of
scan-ahead: it routes concurrent completions to the right awaiter. It does not license the guest
to make different calls; when no matching `Start` exists, replay fails with a divergence error.

**Entry ownership.** A reader that drives the cursor without owning the entry at its head — a
positional marker read, a direct call awaiting its own `End`, a sibling's terminal drain — is
entitled to nothing it did not record. Kind and owner are validated before consumption:

- A durable-call `Start` nobody has claimed is never handed to a positional reader and is never
  parked on either: its owner may be a concurrent host task that needs the very Store the parked
  reader holds (an entity body waiting for admission, an accessor task behind a Store-holding
  direct call). The cursor commits past it and **retains** it (`CursorState::retained_starts`,
  in oplog order, together with its terminal once reached). The owner's later identity claim
  checks retained `Start`s before the head (`claim_retained_start` in `claim_start_matching` and
  `claim_start_matching_request`); a `CompletionDelivered` marker at the head may belong to a
  retained `Start`, so retained-first ordering is required, not an optimisation.
- Retaining is not consuming. Retained `Start`s and their attached terminals are committed
  through `commit_retained_entry`, which does not advance `last_replayed_non_hint_index`; the
  position is published by `publish_claimed_position` only when the owner claims the `Start`.
  A durable call that derives its position from that index (`begin_function` without a scope:
  the replay-time begin index and retry point) therefore observes the same value the live run
  saw as the oplog tip before its own `Start` was appended, not a position advanced by entries
  no caller has consumed. The same holds for replay events: a `CardDerived` or `CardInstalled`
  hint trailing a retained `Start` or its terminal is a side effect of that call, and its owner
  looks it up when it replays the terminal (`pending_card_derivation`). Publishing it while the
  `Start` is merely retained would let an intervening authority boundary apply it first, so
  events recorded during a retained commit are deferred on the `RetainedStart`
  (`deferred_events`) and published by `publish_deferred_events` when the `Start` leaves the
  retained map: claimed, adopted into a custom subtree, folded into the abandoned tolerance, or
  released at the live invocation end (`release_retained_starts_at_live_invocation_end`).
- A recovery Jump abandons the attempt it deletes, including any `Start`s retained from it.
  Every recovery-time Jump (incomplete batched-write and remote-transaction retries on the
  direct and accessor paths, atomic-region rollbacks on the primary and entity Stores) goes
  through `commit_replay_jumps` (`durable_host/mod.rs`), which appends the Jump entries,
  registers the deleted regions with the cursor (`register_replay_jump`) and prunes the retained
  `Start`s inside them, so the re-executed attempt cannot claim the abandoned attempt's history.
- Positional reads through the shared cursor are attributed per Store (`get_oplog_entry(scope)`,
  `OplogEntry::entity_attribution()`): the primary agent consumes only entries with no entity
  parent, an entity body only entries recorded under its own invocation `Start`. A read that
  finds another Store's entry at the head parks only while that Store can still consume it (the
  primary while the cursor replays; an entity body whose `Start` is claimed, retained, or
  scan-ahead claimed). Otherwise `check_parked_positional_read` reports the head as divergence
  instead of hanging replay; the invocation-boundary reader never parks on another Store.
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
  follows the same per-Store admission: `begin_custom_durable_invocation` claims through
  `claim_custom_start_for_store` and continues to live with the Store's own `ReplayToLiveRole`
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
  (HTTP for p2/p3 HTTP, MCP dispatch and external durable streams; RPC for `golem:rpc` invokes);
  the plain `<scope:batched-write>` scope name is classified as HTTP because p2 HTTP records it
  (rdbms transactions share it).

Tests: `replay_state/tests.rs` (`positional_reader_waits_for_a_retained_entity_start_to_be_claimed`,
`interleaved_positional_markers_are_consumed_only_by_the_recording_store`,
`request_matching_claim_adopts_retained_start_behind_its_own_delivery_marker`,
`retained_start_publishes_the_non_hint_position_only_when_claimed`,
`replay_jump_prunes_retained_starts_of_the_abandoned_attempt`,
`retained_start_names_are_published_per_call_kind`,
`closed_retained_starts_are_released_at_live_invocation_end`,
`unclosed_retained_starts_are_rejected_at_live_invocation_end`),
`tests/tool_streaming.rs` (`positional_atomic_marker_does_not_consume_unclaimed_body_*`,
`direct_call_waits_without_blocking_body_admission_*`,
`incomplete_custom_durability_waits_for_overlapping_completed_reconstruction`).

Tolerance machinery (poll-ID stabilisation, response reordering, synthesized readiness, "skip
unmatched entries") hides the first real bug and must not be added. Retaining an unclaimed
`Start` is not tolerance: the entry stays claimable only by its identity-validated owner and is
reported if nobody claims it.

## Replay-to-live

Positional operations (`NoOp`, `BeginAtomicRegion`, and retry-policy entries) use
`get_oplog_entry_or_continue_live` and `prepare_live_continuation_at_replay_tail` in
`durable_host/mod.rs`. The positional read waits out reserved completion-delivery gates and
distinguishes an entry from `ReplayEnded`. Continuation is allowed only for the primary agent
at `ReplayEnded` or an incomplete entity (including its deleted-region local continuation);
a `ReplayingCompleted` entity is rejected. The helper returns `None` when already live or after
the guarded transition finishes, and retries replay on `ReplayResumed`. A wrong recorded entry
is still a mismatch. `EndAtomicRegion` and transaction-protocol reads remain strict.

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
   live iff its own `local_live_tail` is set by `PendingReplayToLive::finish` (`#[must_use]`).
   For `ReplayToLiveRole::NonPrimary`, `switch_to_live` skips (2): the entity Store goes live for
   its own invocation scope (after attachment admission) without waiting for the primary.

Tests: `entity_store_liveness_is_scoped_to_its_invocation_mode`,
`pending_replay_to_live_is_fail_closed_until_finished`,
`replay_state/tests.rs::switch_to_live_wakes_parked_awaiter_as_incomplete`,
`tests/tool_streaming.rs::completed_reconstruction_claim_blocks_concurrent_replay_to_live`.

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
During replay the recorded result is compared with the recomputed one
(`replay_equivalent`); a mismatch is an `unexpected_oplog_entry` determinism error. Tail work
(`durable_host/tail_work.rs`) keeps the store loop running until no spawned task is still
*active*, so a task's positional `Start`/`End` never lands after `AgentInvocationFinished`
(tasks parked at guest-driven safe park points may span invocations). Hints can follow
`Finished`: `invocation_loop.rs::agent_invocation_finished` completes the streaming session
(protocol terminals, `StreamSession { Finished }`) after `on_agent_invocation_success`.

This execution-needing path acquires no capacity during preparation; cancellation cannot split its
durably enqueued acceptance prefix. Read-only hits/followers persist no invocation/result/alias.
Test: `tests/api.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart` — after an
executor restart, an old key returns the recorded result without re-running the guest.

## Durable RPC exactly-once

`RpcTargetAdmission::{Recorded, LiveOnly}` keeps admission separate from call resolution.
Deferred durable activation records or replays its own decision (`Recorded`). Non-deferred
targets return `LiveOnly(PermissionTarget)`; their asynchronous permission check runs only in
`ResolvedCall::Live`, after resolution, preserving operator authorization and decision telemetry.
Denials use `persist_*_denial_from_begun` to record on the already-begun call, not begin another
one. Recorded calls, including incomplete repairs, retain admission without re-authorizing.

1. The caller opens a durable call; its `Start` index `begin_index` is the call identity.
2. `derive_idempotency_key(begin_index)` (`durable_host/mod.rs`) yields
   `IdempotencyKey::derived(current_key, current_idempotency_key_oplog_index(begin_index))`. Inside
   an atomic region the index is the outermost region's *logical* counter
   (`next_idempotency_key_oplog_index`), not the physical index, because a rollback re-executes
   the region at new physical positions and must not mint a new key. Both streaming entry points
   reserve this logical key once on live and replay paths and reuse it for request persistence
   and dispatch metadata. Outside atomic regions, streaming RPC uses the exact physical `Start`
   index to distinguish concurrent calls. Its session descriptor excludes retry tracing and
   canonicalizes environment ordering while retaining execution-relevant configuration.
3. Caller state that the target may depend on is committed before the first dispatch.
4. The target persists `PendingAgentInvocation` (durable acceptance) and later
   `AgentInvocationFinished`; same-key arrivals attach to that one invocation/result.
5. Replay of a completed call returns the recorded `End`; replay of an incomplete call (`Start`
   without `End`) re-dispatches with the **same** key (`InvocationFreshnessDisposition::MayExist`,
   the default). RPC is a `WriteRemote` call, so this repair is allowed by the idempotence mode
   (`assume_idempotence`, default `true`; `golem:api/host.set-idempotence-mode(false)` turns it
   off, after which an incomplete remote write fails instead of being re-dispatched).
6. `KnownFresh` is allowed only when `is_live && is_ephemeral && !assume_idempotence`
   (`wasm_rpc/mod.rs`) and forces a `DurableOnly` commit before dispatch. Ephemeral phantom
   identity is `ephemeral_invocation_phantom_id` = UUIDv5 of the idempotency key — deterministic,
   never random.

### Pending RPC waits and proactive suspension
Pending durable RPCs proactively suspend after a grace period, then reconstruct with the same key;
this never gates recovery. See `reference/rpc-suspension.md` for timing and admission details.
Exactly-once describes the *target's logical execution and effect*, not attempts or packets. Tests:
`tests/rpc.rs::counter_resource_test_2_with_restart`, `failed_ephemeral_invocation_retry_does_not_reexecute`,
`ephemeral_rpc_invocations_get_distinct_final_identities`,
`reacquire_permits_restart_preserves_accepted_queued_live_invocation`, and
`tests/api.rs::lost_card_transfer_response_converges_after_source_and_target_restart` (a wrapped
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
`component_version_for_replay`): the last authoritative snapshot is the baseline. It is either a
manual-update payload or the periodic snapshot promoted by a successful snapshot-assisted
automatic update. A
revision-matching automatic snapshot overrides it and skips `INITIAL+1..=snapshot_idx`, but only
while no update is pending and its index is newer than the fingerprint-scoped persistent
`rejected_periodic_snapshot_through` watermark and the startup attempt's temporary unavailable
watermark. `prepare_instance`
(`durable_host/mod.rs`) then branches on `PendingUpdate`:

- `SnapshotBased` — the save hook already ran and the payload is already recorded; the store must
  already be live, and `finalize_pending_snapshot_update` loads it into the new revision.
- Public `Automatic` admission `P` records only the requested target and its exact attempt identity.
  When that request reaches the queue head, the invocation loop selects the newest eligible
  periodic snapshot `S` in the then-active source revision `R`, after the oplog entry `E` that
  established that revision and beyond the rejection/unavailability watermarks. This means a
  snapshot committed after admission but before activation can be selected. A second correlated
  `PendingUpdate` durably freezes either that exact assisted provenance or the full-replay strategy,
  so reconstruction never recomputes the choice. If none is eligible, the pending kind remains
  `Automatic`: `try_load_snapshot` loads the authoritative baseline, then `resume_replay` replays
  the remaining old history against the new component. If `S` is eligible, the derived internal
  kind is `SnapshotAssistedAutomatic`: the target loads required `S`, skips only through `S`, and replays the surviving
  committed stopped-source suffix; that tail is not fixed at `P`. Missing selection, stale `R/E`,
  load failure, replay divergence, trap/exit, or final-result mismatch
  commits one `FailedUpdate` and reconstructs the healthy source without periodic-snapshot
  quarantine, an application `Error`, or retry-budget charge. It never tries another snapshot or
  switches to full target replay after selection. Genuine interruption retains `P`. Both strategies
  are reported publicly as `Automatic`; assisted provenance identifies the selected strategy.
- No pending update — `try_load_snapshot`; an automatic snapshot load failure or divergent replay
  suffix rejects that snapshot through its index and returns `RetryDecision::Immediate`. The outer
  loop recreates the entire Store, component metadata, revision, and plugin context from the
  authoritative manual-update or promoted assisted baseline, never replaying pre-migration
  history. Only after this
  fallback succeeds, and before readiness is published, is the monotonic rejection watermark
  persisted under the worker's `AgentFingerprint`.

An automatic snapshot payload-download failure instead records an in-memory unavailable watermark
for that startup attempt, so the retry skips the payload without permanently rejecting it; a
successful preparation clears the temporary watermark. A manual-update snapshot cannot be skipped:
its load failure is terminal, wrapped as failure to resume while retaining the underlying cause.
The same is true of a periodic `S` promoted by an assisted `SuccessfulUpdate`: optional later
snapshot rejection falls back to promoted `S`, never to history before it.

Assisted success is finalized through the existing replay-to-live settlement boundary. `U` is
committed after historical invocation results and entity reconstruction validate but before target
live effects or a target periodic snapshot can run. A mutable invocation may therefore span
`Started < P < U < Finished`: the update completes without waiting for that invocation to finish,
and the invocation continues on the target. `U` carries `P/R/E/S`, so
both status and skipped-region folds derive promotion from the outcome itself. Promotion skips only
`INITIAL.next()..=S`, preserves the suffix, switches active Wasm metadata to the target, and keeps
source revision `R` as the historical replay metadata used when reconstructing the suffix.

`SnapshotBoundaryConditions` lists what blocks taking a snapshot: replaying, open atomic region,
open durable scope, snapshotting already, in-flight live host call. Automatic snapshots are
revision-scoped and ignored once the revision changes.

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
baseline must not remove overlapping `Jump` or earlier `Revert` regions, and the resulting mask is
also used to detect durable constructs spanning the cut. Before committing, the executor verifies
that the restored component, retained manual or promoted periodic snapshot payload, replay metadata
and initial files are available. This is input preflight, not speculative replay; a later replay
failure does not undo the committed `Revert`.

Filesystem metadata has one deliberately deterministic exception to ordinary durable host-call
recording. P2/P3 `stat` and `stat-at` on the exact path of a **read-only** component or pinned
entity-activation initial file still execute the sandbox stat, but clear access and modification
timestamps and return without an oplog `Start`/`End`. Type, size, and link count therefore come from
the reconstructed filesystem, while the only volatile fields are removed. Mutable paths and aliases
retain the ordinary `ReadLocal` durable path. An unexpected failure while statting a path already
classified as immutable traps the invocation so a retry cannot choose a different oplog shape. The
host-call observation counter is still incremented on the successful fast path. The predicate lives in
`services/agent_filesystem/lifecycle/mod.rs::is_immutable_initial_file`; the P2/P3 adapters live in
`wasi_filesystem/{p2/types.rs,p3/mod.rs}`.

## Concurrency and guest completion delivery

p3 `Accessor` host calls run concurrently inside one `Store`; p2 `&mut self` calls are serialized
(`concurrent/mod.rs`). Concurrent completions may finish in any host order, but the guest observes
them in exactly one order per run, and that order is recorded by the `CompletionDelivered` markers.
`ReplayDeliveryBarrier` transfers the cursor gate so replay releases each completion at its
recorded boundary. `supersede_prior_completion_delivery` hard-errors if an observer is still armed:
delivery must be the tail operation of a host function. Tests: `tests/concurrent_delivery_order.rs`
(bare Wasmtime: guest initiation order is stable across re-execution while host completion order
is not — the properties replay relies on), `replay_state` unit tests
`switch_to_live_wakes_parked_awaiter_as_incomplete`, `await_natural_tail_end_returns_once_tail_drains`.

For p3 HTTP sends, `TerminalConsumption::NotDelivered` is a positive guest
cancellation/abandonment signal. Store teardown and observer supersession suppress it. The send's
span-only cleanup is a short local durable operation: live drop submits its complete pair to the
oplog actor, and replay cleanup is retained as one shared future so cancelling a waiter cannot
re-submit or lose ownership. Failure terminals and response/body owners claim the same close once.
This machinery changes only span ownership; network execution, call drop policy, terminals, and
the existing non-span cancellation behavior stay with their original owners.

Spawned tasks may park across invocation settlement at guest-driven waits or passive markerless replay-tail waits after durable finalization.
Cursor operations and recorded-marker waits stay active; durable `Start`/`End` work must precede `AgentInvocationFinished` (`tail_work.rs`).

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
- **RPC result and stream draining are separate.** The caller's durable call completes with the
  result *stripped of streams*, so the RPC `End` may be recorded while items still flow.
  Streaming keys follow the RPC identity rule above. Terminals finalize once; protocol terminals fence
  later guest terminals. Terminal outputs reconstruct from committed records without reattachment.
  The persisted request also retains the original logical streaming origin, so retries and caller
  forks do not rewrite who originated the logical RPC.

Forks copy ordinary oplog entries and append a `ForkCut`. That marker clips retained stream history,
resets live controls, and stores the creation receipt; it does not carry handle aliases or authorship
mappings. Export forks also append `ExportForkInitialized`, which binds their new public session ID,
fresh invocation key and expiry policy. Revert raises the generation/epoch fence before reconstruction,
so handles issued by the discarded generation cannot control the rebuilt streams. Export targets
are built in hidden staged oplogs and published atomically; matching retries trust the immutable
target receipt while that target remains live. Do not model staging by adding provenance to stream
records.

The primary remains `ExecutionStatus::Running` after the guest returns while owned output
streams drain and invocation/session completion runs. `materialize_streaming_result`
(`worker/invocation.rs`) publishes the early result and preserves typed traps during production
and settlement. Suspension belongs to the outer live invocation or replay boundary, not the
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
cursor (`OwnerExecution`, `worker/instance.rs`).

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
  committed call (ordinary atomic-region rollback can still roll both back). Present or
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
  every completed body has validated (`completed_reconstruction_claim_blocks_concurrent_replay_to_live`).
- A body trap fails the owner invocation without inventing entity terminals.
- Entity and native-tool invocation spans open on the invocation `Start` and close on its
  `End`/`Cancelled` terminal. `set_entity_invocation_scope` restores the recorded invocation
  context before the body runs; keep this reconstruction boundary covered.
- The ambient call authorizes once against the outer surface. Its root `Start` records the pinned
  chain plan; descendants record root plus position, and every occurrence retains the original
  calling principal and typed static installation parameters. Component and host leaves use the
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
- Incomplete entity recovery installs rollback for that entity's abandoned atomic regions before
  body or descendant claims, without removing unrelated ownership entries.

Detailed mechanics, including scheduling and memory admission: `reference/tools.md`.

## External-effect boundaries

| Peer | Guarantee | Who enforces it |
|---|---|---|
| Durable Golem agent (RPC) | Exactly-once logical invocation and result | Target's durable queue + shared key |
| Ephemeral Golem agent (RPC) | At-most-once per accepted invocation; no resumption after target loss | Fail-stop target, deterministic phantom id |
| Oplog-processor plugin | Exactly-once delivery of each oplog batch | `services/oplog/plugin.rs`: deterministic batch key (`oplog_processor_idempotency_key`: source agent, grant id, first/last index → UUIDv5) and per-plugin `confirmed_up_to`/`sending_up_to` checkpoints persisted in `AgentStatusRecord.oplog_processor_checkpoints` |
| External HTTP/TCP/DB/etc. | Exactly-once only if the peer honours an idempotency key; otherwise retries in the ambiguous crash window are visible | Golem attaches an `idempotency-key` header to outgoing HTTP from `derive_idempotency_key(begin_index)` (`http/policy.rs`, unless the guest set one) and offers `golem:api/host.generate-idempotency-key`; the peer must deduplicate; atomic regions group calls |

Four exactly-once contracts coexist and must not be conflated: RPC exactly-once (one logical
target invocation per key), oplog-processor delivery (batch key + checkpoints), stream-item
exactly-once (each `StreamOffset` consumed once per consumer session, owned by
`durable_stream/mod.rs`/`durable_session.rs`), and exactly-once finalization (one terminal per
stream, protocol terminal fencing guest terminals). A change that
satisfies one does not imply the others.

## Wrong model → right model
| Wrong | Right | Fails under wrong model |
|---|---|---|
| "HashMap iteration / poll order makes the guest nondeterministic, so replay must tolerate different calls." | Guest inputs are recorded; same inputs ⇒ same calls. Different calls = executor bug. | `no matching Start` / `unexpected_oplog_entry` errors in replay tests |
| "Cursor reached the end, so I can do the live effect now." | Liveness is `store_is_live(...)`: the primary needs `switch_to_live` to publish after reconstruction fences; an entity Store needs its own `local_live_tail`. Cursor exhaustion is neither. | `pending_replay_to_live_is_fail_closed_until_finished`, `entity_store_liveness_is_scoped_to_its_invocation_mode` |
| "The voluntary-suspension predicate gates interruption or recovery." | It only defers proactive yielding while live work progresses; explicit interruption and arbitrary Store loss still use ordinary reconstruction. | Simulated-crash tests at arbitrary points (`simulated_crash`, `interrupt`) |
| "Restart differs from suspend." | Both discard the `Store` and reconstruct. | `counter_resource_test_2_with_restart` (state continues across an executor restart), `reacquire_permits_restart_preserves_accepted_queued_live_invocation` |
| "Losing a shard reconstructs the agent, like a restart." | It is retired instead: that generation is stopped without writing its status, dropped and never restarted. Only the executor now holding the shard reconstructs it — the new owner, or this one if the shard came back at a higher epoch. | `worker/mod.rs::a_failure_that_is_a_lost_shard_is_given_up_on_every_path`, `services/oplog/tests.rs::a_fenced_oplog_refuses_new_adds_and_keeps_the_indices_it_handed_out_readable`, the fence tests in `tests/indexed_storage.rs` |
| "A retried RPC attempt executed the target again." | Same key ⇒ same target invocation; count target mutations, not attempts. | Provider-side counter tests in `tests/rpc.rs` |
| "Atomic rollback should generate a fresh RPC key." | Logical counter is owned by the outermost atomic region; keys survive `Jump`. | `tests/transactions.rs`, `tests/revert.rs` |
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

A failed durable call has two recovery paths (`durable_host/durability.rs`):

- **In-function retry** re-runs the effect inside the same host call under the same `Start`.
  `InFunctionRetryState::decide_retry_with_properties` allows it only when the function type is
  `is_eligible_for_internal_retry` (`ReadLocal`/`ReadRemote`/`WriteLocal`; `WriteRemote` only
  with `assume_idempotence`; batched/transaction writes never — the same set as
  `can_reexecute_on_incomplete_replay`), the call is not inside an atomic region, the resolved
  `NamedRetryPolicy` still permits an attempt, and the delay is ≤ `max_in_function_retry_delay`.
  Each attempt appends and commits an `OplogEntry::Error { retry_from, .. }` hint.
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
