# Testing patterns for durable execution

A durable-execution test earns its place when a plausible wrong implementation fails it. The
wrong implementations that recur are: duplicate execution hidden behind equal return values,
tolerance machinery in replay, liveness inferred from cursor position, and resident state treated
as authoritative. Each pattern below names the mistake it catches and an existing test to copy.

Run tests with the `testing` skill (`cargo make worker-executor-tests-groupN` or a `cargo test -p
golem-worker-executor --test <file> -- <filter>`); build the required WASM fixtures with
`modifying-test-components`.

## Tooling

- `golem-worker-executor-test-utils/src/lib.rs`
  - `start_with_overrides(deps, &context, TestExecutorOverrides { .. })` — start an executor with
    wrapped services. Dropping the returned executor and starting another with the same context
    tears down and recreates the whole executor in-process (new `Store`s, new caches, new
    invocation queues) over the same oplog storage; that is a complete reconstruction even
    though no OS process dies.
  - `TestExecutorOverrides::wrap_rpc` — intercept RPC dispatch to drop, delay or duplicate
    attempts.
  - `TestExecutorOverrides::wrap_key_value_storage` — inject storage faults above the retry
    decorator (`FaultInjectingKeyValueStorage` from `src/storage/keyvalue/fault_injecting.rs`,
    used in `tests/api/mod.rs`).
  - `TestExecutorOverrides::wrap_shard_service` — fake ownership changes.
  - `executor.oplog_service_call_count(&worker_id, "read_exact")` — prove a path did not scan
    the oplog.
  - For recovery-data outages, target an uncached recorded payload and prove that each failed
    physical runtime is destroyed before replacement. HTTP tests must use Preview 3.
- `golem-test-framework/src/dsl/mod.rs`
  - `simulated_crash(&agent_id)` — `InterruptKind::Restart`; the resident worker reconstructs
    immediately, or on its next wakeup if already unloaded.
  - `interrupt(&agent_id)` / `resume(&agent_id, force)` — stop and later reconstruct.
  - `get_oplog(&agent_id, from)` / `search_oplog(&agent_id, query)` — assert oplog shape.
  - `wait_for_status(..)`, `check_oplog_is_queryable(..)`.
- Test components under `test-components/` provide counters (`agent_counters`) and host-API
  drivers (`host_api_tests`); a counter is the canonical provider-side effect meter.

## Pattern 1: count the effect, not the echo

Mistake caught: duplicate execution that returns equal values.

Do: increment a durable counter in the target (or an external stub), crash or retry the caller,
then read the counter. Assert the *count* and, separately, that the *next* fresh invocation
continues from that count.

Copy: `tests/api/mod.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart`. 32
increments, executor drop + restart, re-sending the oldest key returns `1` (the recorded result),
and the next fresh key returns `33`: the dedupe oracle is the fresh key, which proves the repeated
key did not increment. `tests/rpc.rs::failed_ephemeral_invocation_retry_does_not_reexecute` — two
failed attempts of `increment_remote_then_fail`, then a plain `increment` on the target returns
`2` (one from the single execution of the failing body, one from the observation call); a
re-executed body would make it `3`.

## Pattern 2: assert identity and execution count separately

Mistake caught: "same result, so probably deduped" and "new key on retry".

Do: capture the idempotency key the target observes on each attempt (e.g. via `wrap_rpc`),
assert the keys are equal, and assert one `AgentInvocationStarted` for that key in the target
oplog. After an ordinary crash-retry the caller keeps a single `Start` (the incomplete call is
repaired under the existing entry). Inside a custom
durable invocation recovered incomplete, nested calls are re-recorded with
`observational_owner`, and the library author owns their idempotency.

Copy: `tests/api/mod.rs::lost_card_transfer_response_converges_after_source_and_target_restart`.
a wrapped `RecordingWorkerProxy` drops the RPC response; after both source and target restart,
the proxy has seen two identical attempts and the target oplog holds exactly one
`CardTransferred`. `tests/rpc.rs::counter_resource_test_2_with_restart` (counter reads `1`
before and `2` after the restart: reconstruction neither lost nor repeated the increment).
`tests/rpc.rs::ephemeral_rpc_invocations_get_distinct_final_identities` (distinct *logical*
invocations must get distinct phantom identities; the same one must not).

## Pattern 3: gate a crash at a named window

Mistake caught: designs that are only correct for the crash windows the author imagined.

Do: pick the windows from `crash-matrix.md` — before `Start`, after effect before `End`, after
`End` before delivery, after `AgentInvocationFinished`, during snapshot load, mid-update — and
force the crash there. Use `wrap_rpc`/`wrap_key_value_storage` to fail the exact operation, or
a guest method that performs the effect and then traps (`increment_remote_then_fail`), or
`simulated_crash` while a guest is parked on a known await.

Copy: `tests/rpc.rs::caller_recovery_restarts_input_drain_after_rpc_result_commit`
(crash after the RPC result was committed, before the caller drained the stream),
`callee_recovery_continues_output_after_committed_item`,
`tests/durability.rs::custom_durability_crash_mid_live_invocation_reexecutes_whole_body`.

## Pattern 4: real reconstruction, both state and oplog

Mistake caught: "cursor rewind is recreation" and fixes that only work while caches are warm.

Do: drop the executor (or `simulated_crash`) and start again; then assert (a) guest-visible state
via a fresh invocation and (b) oplog shape via `get_oplog`/`search_oplog`: exactly one
`AgentInvocationFinished` per key, every `Start` in successfully settled history has one
terminal, no positional `Start`/`End` after the last `AgentInvocationFinished` (hint entries such
as stream-session completion may follow), expected `CompletionDelivered` markers present. When
component
revision, plugins or metadata matter, the restart must go through instance creation
(`recovering_an_old_worker_after_updating_a_component`).

For infrastructure reconstruction, also assert the accepted invocation remains pending while the
fault is active, `Error.kind` is `Recovery`, the complete semantic retry-state map is unchanged,
each failed body-read `Start` has no invented `End`/`Cancelled`, and clearing the fault completes
the original idempotency key with exact asymmetric output. Repeated failures must cross distinct
runtime-disposal generations; rewinding only the replay cursor is not recovery.

Copy: `tests/api/mod.rs::p3_promise_suspend_survives_executor_restart`,
`tests/rpc.rs::ts_cancel_survives_executor_restart`,
`tests/durability.rs::snapshot_based_recovery_preserves_state_across_multiple_restarts`.

### Completed-history input acquisition without a Worker pause

The completed Counter increment is pure guest code. Use the supported
`oplog.max_payload_size = 1` inline threshold to store its invocation input externally, then select
the exact payload from its original Started and require a Finished before the next invocation.
Require uncached external data and no Jump. The normal replay path must acquire that recorded input
through the oplog dependency before lowering and invoking the guest.

The test-utils payload-read adapter is keyed by the full owner and actual payload UUID. Its entry
receipt follows a Pending poll of withheld delivery, then release calls the unchanged classified
download path. This is a real data-acquisition boundary, not a native syscall Pending witness or a
claim that the guest has already executed. It neither sets replay state nor adds a Worker hook.

Keep the memory-stop and same-cached-Worker window-replacement assertions strict: exact acceptance,
period/revision/fuel facts, old/new attempts and generations, held permits, receipts, physical cleanup
and original Started/Finished/results. Compare the complete raw prefix after each stop and later
reconstruction. A repeated-key result is not enough; the next fresh Counter result must also prove
that completed replay did not repeat its effect.

## Pattern 5: completed replay performs no new effect

Mistake caught: re-executing a recorded call during replay.

Do: stop or count the external dependency before restarting. With a wrapped RPC or storage
service, assert its call count is unchanged after a restart that replays a completed call. With
an HTTP test server, stop it before restart; replay must still succeed.

Copy: `tests/api/mod.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart`. The
`read_exact` count is unchanged across the repeated-key lookup, proving the recorded result came
from the physical index rather than an oplog scan; the effect assertion is the `33` on the fresh
key. `tests/rpc.rs::durable_streaming_output_recovers_after_executor_restart` — after the restart
the consumer receives exactly `66 - observed` remaining items and each output stream ends once,
so no already-observed item is re-emitted.

### Reconstruction adds no customer charge

Use the production idle-eviction operation after observing `EvictionClass::LoadedIdle`. Require
unloaded state and physical permit release before fresh demand reconstructs the Store. An interrupt
on an idle worker, or graph shutdown while retaining its cached Worker, is not an unload receipt.
Compare actual delivered usage updates for identical logical work, not invocation diagnostics.

Copy `tests/api/interruption/metering/reconstruction.rs`. Its Counter case compares live increments
after charged setup, with and without real eviction. Replayed setup remains inside the measured
interval. The colocated `services/resource_usage_metering/tests/reconstruction.rs` uses the existing
clock and allocation-reader boundaries to compare memory/filesystem units, preserve blob stock,
retain permit contention and exercise the per-agent memory callback. These observations are scripted,
not native managed-XFS measurements. Its local entity case distinguishes primary replay memory,
entity live memory and filesystem capability. `workerctx/default.rs` tests actual component
call-hook boundaries while retaining physical instruction diagnostics.

## Pattern 6: asymmetric concurrent orderings

Mistake caught: delivery-order logic that only works when completion order equals initiation
order.

Do: run several concurrent host calls whose completion order is a non-identity permutation of
their initiation order, record the delivery order, restart, and assert the guest observes the
same delivery order on replay. Use permutations, not a single reversed case.

Copy: `tests/concurrent_delivery_order.rs::delivery_order_matches_completion_order_for_permutations`,
`concurrent_host_call_initiation_order_is_stable_across_reexecution`.

## Pattern 7: replay strictness is a passing condition

Mistake caught: treating replay errors as flakes.

Do: when a test produces `unexpected_oplog_entry`, "no matching Start", an unexpected extra
entry, or a changed set of pending completions, the test has found a determinism bug. Do not
retry the test, loosen the assertion or add a scan-ahead/tolerance path. Reproduce with
`get_oplog` before and after the restart and diff the guest-visible inputs.

Unit-level copies: `src/durable_host/replay_state/tests.rs` (cursor, claims, barrier semantics,
`switch_to_live_wakes_parked_awaiter_as_incomplete`), `src/durable_host/mod.rs` tests
`pending_replay_to_live_is_fail_closed_until_finished` and
`entity_store_liveness_is_scoped_to_its_invocation_mode`.

## Pattern 8: snapshot and update modes

Mistake caught: snapshot hooks that append or consume oplog entries; updates that keep stale
snapshots.

Do: take a snapshot, invoke, restart, and assert the oplog after the snapshot has no durable-call
entries from the save/load hooks; update the revision and assert the pre-update snapshot is not
loaded.

Copy: `tests/durability.rs::{automatic_snapshot_every_2nd_invocation, snapshot_based_recovery}`,
`tests/hot_update.rs::auto_update_invalidates_snapshot_from_previous_revision`.

## Pattern 9: unload during arbitrary progress

Mistake caught: feature-specific "safe to suspend" gates.

Do: interrupt or crash while the guest is inside the feature under test (mid-RPC, mid-stream,
mid-transaction, holding a spawned task), then resume and assert progress completes with the
recorded effects exactly once. If the feature needs the runtime to stay resident to finish, the
feature is not durable yet.

Copy: `tests/rpc.rs::reacquire_permits_restart_preserves_accepted_queued_live_invocation`,
`tests/api/mod.rs::pending_self_card_transfer_recovery_does_not_deadlock`,
`lost_card_transfer_response_converges_after_source_and_target_restart`.

### Custom scopes compose with suspension

`tests/api/interruption/lifecycle/custom_durability.rs` uses production Context and real SDK
promise waits. Its automatic case observes Suspended, Store disposal and permit release before
completing either promise. Reconstruction must retain both sibling Starts and the later root call's
ordinal. The quota cases hold either sibling's custom End receipt at the existing oplog boundary, apply
a monthly memory-policy update and observe actual acceptance. The permit and pending finish remain
owned until that receipt is released. Whole-Store teardown retains the End without a delivery/discard
marker, while the other sibling remains incomplete. Ordinary reconstruction skips the completed
body and repairs the sibling under its original Start.

Keep exact terminals, invocation acceptance/result counts, the complete committed raw prefix and
closed-window settlement separate. Bind dependency gates to the recorded Start after both sibling
waits enter. A function-name gate can select the wrong sibling when Start observation order differs
from logical initiation order; the reverse-completion case reproduced that test-targeting defect.
Keep both completion orders instead of serializing the bodies. Promise values prove continuation,
not external mutation deduplication. The upstream custom HTTP sibling test supplies a separate
provider-effect counter.
CompletionDelivery body tests separately cover accepted terminal ownership and call-specific error
attribution, including a failed terminal during teardown. Neither layer alone proves every custom
operation, agent mode or resource meter.

## Pattern 10: streams and tool bodies at named crash checkpoints

Mistake caught: stream liveness inferred from the bus/socket; tool bodies repeating their
external effect on completed replay; body state kept only in the transient entity Store;
admission skipped for incomplete bodies.

Do: for streams, crash the producer after a committed `StreamItems` and the consumer after a
journaled `ConsumerItemValue`, restart, and assert every item is observed exactly once by offset
and that exactly one terminal is recorded. For tools, drive the body to a named checkpoint with an
in-process crash-checkpoint server (`tests/tool_streaming.rs::start_crash_checkpoint_server`, a
tokio task — not an external process), crash, restart, and assert (a) the completed path
(`ReplayingCompleted`) runs the body's guest export again with its host calls replayed from the
owner oplog and releases the recorded terminal without a repeated external effect (checkpoint
server hit count unchanged), (b) the incomplete path completes under the original entity `Start`
index, and (c) the owner oplog has one entity terminal per `Start`.

Copy: `tests/rpc.rs::{durable_streaming_output_recovers_after_executor_restart,
callee_recovery_continues_output_after_committed_item,
caller_recovery_restarts_input_drain_after_rpc_result_commit}`,
`tests/tool_streaming.rs::{deterministic_stream_crash_checkpoint_matrix,
active_stream_crash_replays_pinned_activation_with_fresh_attachments,
concurrent_tool_attempt_identity_survives_reordered_admission_and_replay,
incomplete_custom_durability_waits_for_completed_reconstruction}`.

## Pattern 11: treat owner and entities as one lifecycle unit

Mistake caught: restarting only the primary Store, allowing reconstruction to overtake an entity
drain, or letting stale owner A mutate replacement B because both have the same `AgentId`.

Do: enter a real entity body or completed-body reconstruction and hold it at a named checkpoint,
then request restart, suspend, or terminal interruption. For restart/control races, assert the
original invocation identity completes, its semantic retry counters are unchanged, completed
external effects are not repeated, filesystem state and the asymmetric result reconstruct
correctly, and a fresh follow-up invocation is safe. For explicit terminal interruption, assert
the invocation remains stopped until resume rather than silently restarting. Run the stale-owner
case asymmetrically: publish B, release A's delayed cleanup, and prove B still accepts and completes
fresh work—not merely that A's operation returned. For executor shutdown, enter and retain the
callback before shutdown, then prove task destruction and callback/oplog-layer joins complete
without semantic finalization.

Copy: `tests/tool_streaming/mod.rs::suspended_restart_replays_completed_tool_without_semantic_retry`,
the lifecycle propagation and stale-generation cases in `tests/active_agents.rs`, and the retained
entity shutdown cases in `src/services/active_agents/tests.rs` and
`src/worker/entity_invocation.rs`.

### Teardown without a private pre-fence pause

Use real pending outcome appends to prove writer and FIFO-capacity ownership. The retained-Suspend
native case keeps its writer withheld while an accepted stop signals, requires the same account
contender to stay blocked, then releases the writer and joins actual cleanup before that contender
acquires. It does not invent a second teardown checkpoint after the writer returns.

Later-start provenance cases first join automatic suspension and physical cleanup, then occupy a
real account permit for the next startup. Keep their exact receipt, error, marker, generation and
acquisition matrix. The fake before/after-fence snapshots and await-ready Pending projections retire
with the removed pause. They are not a reason to add another fence or to call append backpressure
the same checkpoint. The loaded-idle compute-admission case retains the original queued key without
guest execution until policy recovery, checks one Suspend and the complete raw prefix, then counts
the next fresh Counter effect. Admission-before-establishment release-order variants have no
surviving production protocol to reproduce.

Completion-first cases do not need a virtual timer when real native completion, the required host
records and an entered invocation-outcome append already establish the scenario. Let the monitor
use its ordinary timer. After the existing actual monitor and physical-cleanup joins, require the
monthly target to be inactive and capacity subscriptions to be gone. A virtual clock's receiver
count is not an additional physical-release guarantee. The P2 input/splice/multi-poll and P3
promise/UDP completion cases retain their other oracles unchanged without those unused freezes.

### Monitor cadence with the normal timer

Check exact deadlines on `MonthlyMemoryCheck::duration` and `wait` in a standard paused Tokio
runtime. The colocated tests poll the actual normal sleep before advancing time, reject early
completion and verify rearming after an applied update and a rejected proposal. They exercise the
existing queue function, not a copied monitor loop. This keeps two-period timing proofs independent
of native test setup time.

The real-Worker retry case uses actual interrupt-lock contention, two rejected applied revisions
with unchanged exhausted amounts, then one normal timer tick. The accepted tick must have the exact
latest rejected window/policy snapshot, with no intervening revision. Its elapsed lower bound comes
from the final update, not an ordering sleep. Exact tick-rejection cadence belongs to the colocated
proof; the native case proves applied-check rejection followed by real-timer retry, accounting,
terminal ownership, physical release and same-key recovery within its original timeout. It also
retains the complete stored raw prefix and checks invocation counts independently of the result.

Join full Registry refresh tasks after the silent invocation unloads. Monthly policy application
precedes the batch's unrelated filesystem-limit reply, which requires the occupied invocation loop.
The completed rejection diagnostic proves that the policy applied; joining the whole batch before
allowing interruption deadlocks the test. Keep both full joins after physical cleanup. The other
time-driving consumers and Worker clock support remain separate open work.

### Native wait observations without Worker hooks

Use production tracing fields for real-Worker native Pending observations, including P2, RPC, UDP
and passive WebSocket waits.
`tests/native_waits.rs` installs a structured layer once through the common tracing initializer,
before global subscriber initialization. Its cross-spawn interests, indexes and receipts exist only
in the integration-test binary. Decode numeric UUID halves, reps, Starts and ordered pollable bindings
with typed visitors; keys are string fields. Read fingerprint/window facts from `wait_started`, which
precedes owner span records. On the long `native_wait` span, first actual Pending records the boolean
`was_pending=true` once; `state` stays empty until its single Returned or Dropped terminal. The
collector reads that typed boolean and strict terminal state without a legacy Pending-state fallback.
Short transitions still carry their own Pending state. `execution_window` leaves `physical_release`
empty until its one true record after physical release. Standard OpenTelemetry export represents
unsigned attributes as exact decimal strings; tracing visitors still receive typed unsigned values.
No alternative field mapper is installed. Keep terminal state in shared span/receipt data so a queued
Pending receipt consumed after termination reports Returned or Dropped. Unindex terminal waits and
closed windows; unregistration must not break a retained receipt. Abandonment is not physical release.
Keep unexpected input operations visible to the caller. Subscription filtering separates poll,
RPC-result, UDP-receive and the known WebSocket operations from input, with an explicit input retry-sleep opt-in.
These observations do not pause native work.

RPC observes the actual task-result future after its existing lock, not lock admission or a
fabricated result. UDP observes the native receive and records its actual socket binding on the
same span. The test collector requires that binding before sending a datagram or checking physical
release by rebinding. A missing binding is not a zero/default endpoint. Socket events belong to the
exact wait ID, including IPv6; they must not leak to a sibling RPC wait or survive in collector history.
The standard SDK exporter test checks the same binding fields on the existing span and event.

Both old Worker observer modules, fields, setters and producer branches are removed. Retargeted
native tests retain typed Pending/Returned/Dropped, provider effects, history, accounting, same-key
continuation and cleanup checks. Compare trace receipts using the actual diagnostic window ID,
fingerprint, attempt, generation and capacity revision. This is read-only correlation, not an Arc
pointer or another acceptance proof. Production acceptance separately revalidates the exact target
Arc. The observer removal changes neither RPC recovery nor UDP terminal/delivery ownership.

WebSocket handshakes retain initial/accessor-reconnect/direct-reconnect operation labels. Pool
acquisition retains accessor/direct labels. Timed receive observes only the first frame future
after acquiring the same reader lock, with its timeout and signal priority unchanged. The three
passive Worker modules, fields, methods and producer branches are removed. Native integrations
retain all assertion payloads except the enum/path observation names. Their real peer/pool waits,
effects, typed outcomes, exact histories and cleanup remain separate from the shared collector
contract tests. Reader contention uses the real dependency below; neither change resumes the
deferred WebSocket replay investigation.

The reader-order family is also removed, including its Worker fields, setters, producers and
gate-only tests. `receive_lock_contention` creates its own guest promise, then its timed branch
awaits it before entering timed receive. After actual untimed frame Pending, commit and decode
that create response. Require its actual owner, original Start and one terminal before completing
it through the normal API. A seed-owned promise is invalid: the host correctly rejects foreign
awaits. Do not relax that ownership rule. The independent probe creates its own promise as well.

Require actual timed lock Pending on the same `connection_rep`, invocation, runtime and later Start.
Both waits use existing tracing; the `connection_binding` event belongs to the exact wait span.
Returned from the lock future means Acquired, not frame completion. Preserve the native timeout
test's elapsed-duration check and exact terminal after acquisition. All original reader assertions
remain after observation substitutions, with additional checks for the actual promise and binding.
This proves reader ordering and stop-only cleanup, not same-key two-read recovery after monthly
suspension. No lock, timeout, interruption priority or reconnect decision changes.

Keep two proof layers. Real Context integrations retain native identity, key, child Start, effects,
accounting, joined cleanup and ordinary reconstruction assertions. Precise provider/resource facts
belong to tests of the actual production body, not a copied select loop or a logging preflight.
`native_splice_interrupts_actual_pending_without_output_effect` checks writable output before the
actual Wasmtime splice parks on quiet input, then requires typed interruption, no output effect and
real resource deletion. `interrupted_prefix_preparation_keeps_original_real_http_resources` polls
the production prepare/install helper and requires the original stream, body and child relationship
after cancellation; its completion control checks installation and the unconsumed suffix. Peer
fixtures separately retain transport/recovery facts. Resend must observe the second body Pending
before releasing or closing the first peer.

`native_wait_diagnostics/otel_tests.rs` contains standard SDK adapter regressions, not a deployed
exporter. Their synchronous simple processor checks exact links and short Pending export while long
wait/window spans remain open, with unique terminal-state, Pending and physical-release attributes.
Keep these SDK assertions separate from Tempo delivery and performance evidence. Read
[native wait tracing](../../../../golem-worker-executor/native-wait-tracing.md) for deployment
configuration rather than adding a second tracing path.

### Monitor health at the usage-source seam

Monitor health tests use the existing owner-usage-flusher seam for panic faults, not a Worker panic
setter or an extra select arm. The test adapter signals actual unwind, keeps no Worker/Store, and
stays alive through cleanup. Apply an ordinary policy revision to wake capacity detection, then
join the full refresh only after withheld idle/physical cleanup completes. Preserve initial/later
idle, pending native I/O, exact retained error, generation and permit-acquisition assertions.

Actor-loss tests use the existing oplog append-failure fixture, not an orderly shutdown API or a
Worker stopper. Hold a real post-Started GrowMemory append, require its positive delta, then release
it into the provider's ordinary error path. Counter.allocate_memory supplies guest work at later
idle; the pending-TCP case uses its running invocation. The lifecycle job's fail-stop panic drops
the actor receiver, which the normal monitor observes. Test-side capture of the existing before-
fence event matches its exact target, OwnedAgentId, lifecycle kind and message. It unindexes before
delivery, keeps only active interests and does not claim that entity or filesystem cleanup completed.
The original exact health, cleanup, startup-refusal and generation assertions decide success.
Initial idle has no guest Started or growth job. Its manual-stop-only actor cell retires with the
stopper. The real initial-idle monitor fault still checks startup failure and retained cleanup health.

### Success finalization and the invocation boundary

The ordinary terminal check remains before success finalization. Its colocated test leaves a stop
that arrives afterward for the invocation loop. Actual native callbacks retain commit-before-result-
notification, exact key/result/history, physical cleanup and ordinary recovery. Stop signalling may
overlap an append held at the real oplog dependency; raw-signal-after-private-writer ordering is no
longer a contract.

The private selected slot, writer receipts/verdicts, capture/join and pending/committed projections
are deleted, including their protocol-only tests. No compatibility wrapper or replacement registry
remains. Guest-failure, timeout and automatic-Suspend regressions demonstrated that invocation exit
was calling failure twice. It now reads FIFO-folded status and records retry interruption only for
Running or Retrying. Those native assertions remain exact and passed after the source repair.
The pure monthly mapper cannot replace a deadline or unrelated error with a queued stop.
The subsequent establishment restoration passes its combined affected checks. Keep the broader
A1 review and remaining F10/F11 controls open; this is not full upstream SDK or managed-XFS evidence.

### Retained stop markers and physical release

Assert the marker's provenance, not just its kind or the current generation counter. A committed
admission Suspend can satisfy a later monitor Suspend for the same established owner with its
attempt-derived generation and FIFO Suspended status. Preserve the exact earlier prefix. A new
attempt stopped before establishing an owner must append its
own Suspend even when the resident counter still names the previous generation. The admission and
retained-terminal cases assert one and two Suspend entries respectively. The taken no-body demand
still owns physical disposal; an absent public receipt is not proof that disposal is complete.

For native cleanup, `tests/api/interruption/lifecycle/health.rs` uses an account limit of one and an
actual FIFO permit contender. It polls that contender Pending while native directory permissions
are changed while Started still holds the execution permit. After actual deletion fails, the same
contender acquires execution capacity while the runtime root still exists. Startup and readiness
still refuse the retained structural failure. Hold that lease through ordinary public Delete after
permissions are restored, then require verified root removal with generation and acquisition count
unchanged. Returned execution capacity is not successful filesystem cleanup.
Keep this real-Worker evidence separate from queue/predecessor and EntityFence body checks.
Other teardown and monitor controls remain separate.

`execution_window_closes_before_filesystem_deletion` calls the actual unload body and captures
settlement, permit and Store-drop facts at the dependency's entered pending deletion. Require settled
accounting and a released permit there, but keep cleanup publication and physical metrics pending.
Failure/repair tests retain the original error and actual filesystem owner through verified repair;
only a diagnostic cleanup receipt remains with that failure, not a retained billing/permit bundle.

`unload_preparation_defers_allocation_to_window_close` polls the actual preparation with the final
allocation dependency blocked, then completes ordinary close before asserting. Preparation must
not sample; close must take exactly the remaining final sample. Metering fixtures use real accepted
observations at known times and preserve the one-second stale-sample rule and exact charge oracles.
`active_allocation_failure_remains_failed_after_successful_physical_deletion` waits for the actual
periodic read and published enforcement fault before unloading. It retains that exact error through
verified deletion. Ordinary final-read retries and customer-favoring close timeouts have separate
metering tests; transient final-read errors alone do not establish retained enforcement failure.

### Stop publication, drain and acknowledgements

The production publication transaction is removed with its private receipt and leader/follower tests.
Use main's actual request/establishment objects. Queueing must advance the tail only on acceptance;
replacement retains the preceding establishment. A taken request waits on its own establishment.
The permit-wait body must drop acquisition before joining it, retain deferred command replies and
never poll even a ready acquisition when a request already waits.

The all-meters-off late-stop test subscribes in test code to the scheduler's existing exact queue
event for its account and agent. The event follows actual enqueue. Await it after the external FIFO
contender acquires capacity, then require the original loop's permit flag to be released before
issuing the later stop. This proves reacquisition without treating establishment as physical unload,
calling a cleanup API that requires the loop to have stopped, or adding a Worker observer. The
capture stores only active subscriptions and removes them before delivery. It adds no production
instrumentation, scheduler branch or wait hook.

`entity_fence_keeps_attached_store_and_registration_until_the_body_exits` polls the real
`EntityFence::drain` Pending with an attached EntitySlot and observable Store. Verify cancellation,
memory metadata, registration retention and an available slot-map mutex. Destroying the Store alone
does not drain registration. Releasing registration completes drain without destroying the primary
Store. This is a host-body proof, not a native guest charge or whole-Worker interleaving.

Window close no longer waits on artificial registered stop futures. Its tests retain actual permit
release, monitor join and first cleanup failure, including failure after target closure. Entered
effect and physical-drain retention remain native entity/backend/filesystem obligations, not another
stop coordinator.

Native Waiting tests hold actual lifecycle-marker persistence. Run the interrupt request independently
when that same request owns the held append. Observe its ordinary execution-status receipt, require
Empty before commit release and exactly one afterward, including on the returned receiver. After
ordinary status drops the sender, require Closed rather than an artificially retained channel. A raw
signal channel likewise need not remain open after the status and native wait drop its last sender.
Do not accept an additional signal or acknowledgement. The append
owns the instance mutex: do not query it or submit another stop while held. Started can precede Running
and promise no public receipt. Keep exact marker, key, result, cleanup and reconstruction assertions.
Monthly native tests use test-side `published_interrupt_receipt` while provider or oplog work is held;
it observes ordinary status rather than invent a Worker receipt. Teardown, idle, monthly-time and
replay controls remain unfinished work.

### Stop freeze capture and native prerequisites

There is no freeze operation, frozen projection or immutable publication cause. Retire tests for
that protocol with its producers. Main may signal Restart before establishing a later terminal
request; do not restore the stronger single-elected-signal rule through another gate or coordinator.

Native completion and pending-Suspend cases hold real terminal persistence. Subscribe raw before
public `set_interrupting` and retain its exact returned Some receiver, not a private writer
projection. A stop can signal during that append. The callback and real teardown still retain the
Store and permit until required work finishes. Require one exact outcome/marker, unchanged key and
history, physical cleanup and recovery; do not accept extra quota errors or duplicate Suspend.

The shared retained-terminal helper's first automatic phase has no public Some promise. Its private
after-freeze receipt subscription is gone. At the already-held teardown checkpoint, retain the
actual selected result, held permit and already-polled real FIFO
lease. Create and poll the existing AwaitReady future before releasing that same teardown hold. Then await the same
lease, await readiness, and join accepted work and cleanup. Do not substitute a fresh lease, new barrier
or an early empty join before automatic acceptance. Observe the automatic cause from the actual
owner fence after joined cleanup, not a deleted writer slot. Exact owner cause and final
marker/generation/acquisition facts remain native assertions.

Waiting retains its real append and public Some contract above, without instance queries or competing
stops while persistence owns the instance mutex. BeforeOwner/RestartFollower retain their held admission
before generation begin and actual driver join. Later pending demand, exact startup/receipt contracts
and one-versus-two Suspend provenance remain required. Neither proof layer alone establishes the old
controlled whole-Worker compound interleaving.

### Monthly acceptance guard and applied capacity

Acceptance observation uses test-side capture of the existing completed `monthly_acceptance`
diagnostic. The sample fields and exact acceptance boolean share that event. They do not expose a
pre-acceptance phase or form an interruption receipt. A bounded receive replaces the old proposal
receive plus separate runtime completion wait. Preserve the same required boolean; rejection never
allows a retry or either-result assertion. The dual-observation baseline checks both contended
rejections and a later acceptance before removing the Worker transport.

Use diagnostic window ID, fingerprint, attempt and generation for native same-window/replacement
comparisons. The ID belongs to the actual window, not a test-side reconstruction of its Arc address.
Colocated tests retain literal Arc matching. The read-only current-window snapshot remains separate
from the removed observer field and producer. Missing or mismatched events fail the test; capture
must never create synthetic proposals or results.

The monthly acceptance gate field, setter and pre-validation await producer are removed. Its existing
ordinary tracing event remains. The observer/completion transport is removed separately, without
a replacement runtime gate.
`matches_running_owner` matches the outer `WorkerInstance::Running` before loading generation or held-permit
atomics or locking the current target. It never unwraps `deletion_runtime`. The actual path keeps
cached-owner lookup before the instance mutex, then retains that guard through the predicate, active
recheck, interrupt try-lock, capacity sample, queue and dispatch. Existing short-circuiting stays lazy.

Actual enum tests reject Deleting around an otherwise matching Running owner,
and reject `WaitingForPermit`, `Stopping` and `Unloaded`. Poisoned unused current-target locks and the positive
identity/generation/permit/Arc matrix check the literal guard. Claimed-terminal rejection is a
separate proof; there is no generic retired-admission flag. Same-revision memory credit uses the actual sample, zero-usage snapshot adapter and
queue to reject captured exhaustion without stop side effects. Replacement checks the actual target
Arc/attempt/generation at otherwise equal capacity. These bodies do not execute the whole cached-owner,
instance-lock or mailbox transaction; native current acceptance exercises that real transaction.

`silent_tcp_after_same_revision_capacity_credit_stays_live` exhausts and credits the real account through
full batches before any Worker exists, then retains silent-I/O liveness and cleanup checks. It is not
live-window stale-message delivery. `same_cached_worker_replaces_monthly_window_and_accepts_current_quota`
keeps the cached owner, replaces its actual window/attempt/generation and accepts a current proposal.
Old-target rejection belongs to the body predicate, not a fabricated old completion or late false ack.
Its test-side acceptance subscription is installed after the old monitor joins and lifecycle FIFO drains,
not filtered to discard old events. Retirement cases retain first-cause/history/cleanup facts without
claiming particular late actor-message processing. The retry case remains a timer/rearm proof under
its existing interrupt-lock hold, not a new watch or notification checkpoint.

The service publishes applied monthly capacity before awaiting even an unchanged filesystem-limit
reply. A replay, terminal append or deletion hold can block that tail. Subscribe and mark the existing
capacity watch seen before each controlled refresh; await one changed version and check exact applied
capacity, revision, period, fuel generation, exhaustion and runtime/target identity. Watch version is not policy revision
or refresh generation. Do not drain pending events, infer completion from revision alone or join the
whole batch under those holds. Drop the temporary reader before exact one-monitor/final-zero counts;
two readers while it lives is explicit, not an at-least-one assertion. Release the corresponding replay
or persistence/deletion hold and finish physical cleanup before joining every batch tail. Complete old
exhaustion/credit tails before replacement replay begins. Pre-Worker credit has no filesystem target,
so its full joins are safe. These are read-only dependency observations, not a new Worker interface.

For prepaid RPC caller-first cases, an applied account update wakes both Stores. The target can stop
and refund at invocation exit before the caller's proposal is validated. A terminal-append hold is
too late to prevent that refund. Use the existing real target GrowMemory-append dependency to keep
its lifecycle actor behind that append through caller cleanup. Assert the actual positive append
and its absence from committed history while held. After exact caller acceptance and physical
cleanup, require restored capacity, release the append and drain the target's lifecycle mailbox.
The target must remain live with no monthly stop. Complete its original promise, require the single
logical increment, then use ordinary idle eviction to close its window before the same two-Store
refund assertions. Full Registry refreshes wait for live filesystem-limit replies; join every tail
after target completion, not inside the caller's unchanged physical-stop deadline. Keep the separate
target-first scenario, which expects caller rejection after real refund. Neither case may accept
either boolean result or retry a rejected proposal.

### Durable-source reads without a runtime observer

The keyed durable-source observer, weak registry and ReceiveGuard capture field are absent.
The plain first-read branch calls `await_first_durable_source` with the real reader future and a
borrowed guard. Native readiness wins a plain-branch ready tie. The suspension match remains in
the caller; classified waits keep their existing precedence. The outer receive retains its live-call
permit through selected packed reads, consumer-journal commit and nested mapping work.

`first_source_wait_real_reader_pending_interrupt_and_drop` probes the actual reader future without
adding polls or wakes. It distinguishes entered Pending from never-polled Drop and keeps ordinal
zero through interruption. Other body tests distinguish source errors, real End and missing-terminal
failure. `external_inline_demand_interrupt_preserves_ordinal_and_journal` exercises the full
ExternalInlineInput branch that cannot suspend, including an ignored suspension wait and mandatory
journal completion after source selection. These tests do not change native read cancellation or replay.

The native `durable_native_stdin_memory_quota_preserves_accepted_session` has a different scope.
Accepted can precede remaining ingress startup calls. Before applying exhaustion, the test subscribes
to the existing agent/key-qualified `wait_for_invocation_result` span and requires its creation plus
the first monthly timer poll. The span follows those startup calls. Test-side capture stores active
subscriptions only and adds no production instrumentation. This boundary proves startup ordering,
not source Pending or Running status. Raw delivery and no-Store acknowledgement have separate tests.

The native case requires the same exact monthly and owner cause after joined physical
cleanup, before input or policy restoration. It retains the accepted session, key and stream identity,
sends its first packed bytes only after release, and checks exact output and one successful result.
The typed raw prefix must remain unchanged after ordinary continuation. Positive account-level memory
usage may include the constructor; it is not a quiet-read or per-Store charge. Neither proof layer
alone is the former controlled whole-Worker Pending/stop/Drop witness.

### Entity blob mutation after caller loss

Copy `tests/tool_streaming/durable_entity_blob.rs` for the actual `ComponentEntityRunner` path,
including its completion-first control. Hold a reserved provider write Pending, enter a competing
mutation, then fence the owner and join entity/owner destruction. The original provider, environment
lock and quota must survive caller loss. With the stock cap at ten, the settled stock is eight;
another three bytes must not be admitted. Late known-result settlement changes no owner-oplog
record or terminal. Require no End/Cancelled while pending, one End for each actual entity root and
serialized blob call after completion, and no delivery/discard markers for either. Then perform
ordinary same-key repair and completed cold replay.
Incomplete effect-before-End repair may put the same object/content again without charging its
stock twice. Completed replay must perform no new put. Neither path promises external exactly-once
effects after process loss, an unknown provider outcome or a partial backend error.

Preserve the stored prefix and hydrated invocation/host-call payloads exactly. Validate each public
projection losslessly against those records. Public attribute vectors rendered from a HashMap have
no recorded sequence: compare their complete typed key/value maps, rejecting duplicate keys and
cardinality changes. Preserve all recorded sequence order elsewhere. Sorting arbitrary JSON or
dropping fields would weaken the oracle rather than account for that one unordered projection.

## Filesystem snapshots on APFS

For the macOS seed synchronization policy, see
[macOS seed synchronization](../../../../golem-worker-executor/worker-executor-walkthrough.html#macos-seed-synchronization).
Changes to macOS seed synchronization must cover `Apfs`, `Directory` and `Temporary`: plain
native `fsync` for each seeded regular file and the required scoped namespace `fsync` calls.
Each fresh macOS filesystem has one `seed_flusher`, an unlinked regular file that `NativeRoot`
creates and owns. A root on another platform has no `seed_flusher`. Check that admission pins the
root directory and its `seed_flusher` together, and every admitted batch attempts one final
`F_FULLFSYNC` through the pinned `seed_flusher`. Include empty, symlink-only and directory-only
batches, and batches that fail early. Verify that flush
finalization creates no per-batch directory entry, does not change the root's modification time,
works without a writable root, and cannot be redirected by pathname replacement. Distinguish
a setup or admission failure, which happens before admission and has no pinned `seed_flusher` to
flush.
Check that the namespace and batch finalizers attempt every required synchronization after an
admitted batch error while preserving the first error. Keep Linux seed
synchronization and explicit guest flush behavior unchanged. These are correctness checks,
not performance or workload-suitability claims.

The `build-apfs-test-bin` CI job compiles the executor library test binary on macOS. It selects
that executable from Cargo JSON output and uploads it for the same workflow run. The `apfs-tests`
job runs its `:tag:native_apfs` cases on macOS. The host script creates and attaches one 32MiB HFS+
image outside the test binary, then uses `--include-ignored` to run the constructor-refusal case.
The other native cases use the runner's existing volume. The script detaches and removes the
fixture after the run. A failed detach retains the fixture and fails the job. Worker tests spawn
no fixture processes. Native CI needs no Lima VM, WASM, built-in artifact or Redis dependency.

For broader local verification on macOS, use these commands. The first and the third also select
portable helpers that the ordinary unit suite covers. The second runs the only `local_filesystem`
case, `apfs_restart_from_a_snapshot_gives_the_tree_of_a_full_replay`. That case is macOS-only and
no CI job runs it, so run it locally after a change to APFS capture or restore. It needs the
`test-components/it_initial_file_system_release.wasm` fixture:

```shell
cargo test --locked -j 4 --profile dev-ci -p golem-worker-executor --features test-utils --lib -- sandbox_filesystem:: services::agent_filesystem::lifecycle:: services::agent_filesystem_snapshots:: --test-threads 1 --report-time
RUST_MIN_STACK=33554432 cargo test --locked -j 4 --profile dev-ci -p golem-worker-executor --features test-utils --test integration -- :tag:local_filesystem --test-threads 1 --report-time
cargo test --locked -j 4 --profile dev-ci -p golem --lib -- filesystem_config:: command_handler:: launch:: --test-threads 1 --report-time
```

The library module filters automatically select every portable and macOS seed helper test
in those modules, without a naming prefix or a per-test filter edit. They cover the clone test
in `sandbox_filesystem::apfs::tests`, the startup probe, conservative filename coordination,
the configuration round trip and the capture pressure check.
The portable seed tests also run their APFS branches. They check file attributes and temporary-file
cleanup, symlink times and cleanup, and merging an unreadable empty directory without a sync-only
child open. `macos_seed_syncs_directory_attributes_through_restrictive_permissions` checks directory
permissions and modification time through a descriptor opened before restrictive permissions.
`apfs_conservative_coordination_covers_both_edit_observation_orders` polls contenders under held
leases. It checks that observations run together, edits wait for observations, observations wait
for an active edit, and a different parent remains independent. It uses lease release, not sleeps.
`apfs_native_coordination_preserves_filename_spelling_and_lookup` creates a Unicode name through
the adapter. It checks the stored and requested spellings, native normalization aliases and distinct
files. It runs on the existing volume, so it observes only that volume's case mode.
The `apfs_alias_renamed_initial_file` cases rename a read-only initial file or an ancestor through
an intermediate path to a Unicode alias. They check that capture keeps the stored spelling and bytes,
leaves the declaration out of `left_out`, and restores the same tree. The case-only test runs that
history when the existing volume has case-insensitive lookup; it reports the observed case mode.
`apfs_capture_of_a_tree_with_hard_links_restores_the_link_groups` builds a tree through lifecycle
calls: a new file and a hard link to it, a hard link to a read-only initial file, a renamed file, a
deleted file, and a moved directory that holds the read-only initial file. It checks the two link
groups of the source, then captures the tree, restores it into a new agent on APFS and compares
both trees, link groups included.
The portable `capture_spelling_` cases check raw name comparisons, per-parent reuse, root-to-leaf
targets, error propagation and closing after a failed listing. The scripted opaque-facts test separates
a positive lookup proof from a canonical coordination label, even when the label matches the raw name.
`apfs_copy_contents_and_seed_share_extents_and_preserve_unflushed_writes` checks copy-on-write
copies, unflushed writes, exclusions and hard-link groups through the sandbox operations.
`apfs_a_volume_below_the_pressure_target_admits_no_periodic_upload` checks capture admission.
The native CI selection covers APFS cloning and lookup, the shared macOS seed effects, the
permission regression, capture pressure, and capture and restore with native spelling and with
hard links.
Compilation runs in a separate job, so a failed test does not prevent the successful build from
saving its dependency cache. The ordinary unit suite keeps portable decisions and configuration
coverage; the Linux runner keeps the shared managed and reflink XFS recovery cases.

The ordinary unit suite covers hard-link behavior and the fixed-seed restore property. A directory hard link
must give `not-permitted` even when the destination exists. The scripted collision test programs
`AlreadyExists` followed by a fresh directory observation. The preservation tests cover files,
symlinks, stale directory hints and nonterminal proof failures. A terminal proof failure invalidates
the generation and revokes later calls. The coordination test blocks the post-error source probe
and explicitly polls a source replacement before it releases the probe. It does not infer exclusion
from a sleep. The native minimized history runs the collision before and after capture, compares
the tree with full replay, and checks a successful later write. The independent model tests keep
ancestor and missing-source precedence and do not follow source symlinks.

The native APFS restart test needs the `test-components/it_initial_file_system_release.wasm` fixture.
It takes a periodic snapshot, drops the executor, starts a new one over the same storage and
compares the files with a full replay. It includes hard links. Build the fixture before a local run with the
`modifying-test-components` skill. This optional native integration uses the same recovery helper
as the managed and reflink XFS cases in the Linux managed-filesystem runner. CI keeps those cases
and the native backend proofs without compiling the duplicate macOS integration target.

The ordinary unit suite runs the `golem` library's configuration, command-handler and launch modules.
The focused wiring test parses the real `server run` command, forwards the argument or manifest
root through the production launch builder, and applies filesystem settings through the same
function as startup. It checks defaults, the root argument, TOML over that argument, and
environment over TOML. Two preparations check retained repository key reuse and Unix permissions.
These tests do not start services or provision built-in tools. Existing configuration cases also
cover optional TOML, validation before key effects and corrupt or concurrent retained keys.
The launch module checks that local blob storage remains at `<data_dir>/blobs`.
`Enabled` enables the snapshot service, but an agent still needs a snapshotting policy to save and
restore snapshots. The executor restart test supplies that policy.

## Anti-patterns

- A "restart" that only calls `resume` on a still-resident worker, or resets a cursor.
- Asserting only the caller's return value for an RPC or external effect.
- Sleeping until "probably replayed" instead of `wait_for_status` / awaiting the invocation.
- Loosening replay to make a flaky test green.
- Random inputs that mostly exercise argument validation.
