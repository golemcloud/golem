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

Check memory-stop and same-cached-Worker continuation through execution APIs, accounting,
observable cleanup and original Started/Finished/results. Do not infer exact acceptance,
window identity, attempts, generations or permit counts from the dependency hold.
Compare the complete raw prefix after each stop and later
reconstruction. A repeated-key result is not enough; the next fresh Counter result must also prove
that completed replay did not repeat its effect.

### Live observations and incomplete history

Completed-replay tests first unload the worker, then invoke a fresh `noop` and check results,
peer effects and committed history. They do not count resident creations or observe a private window.

Reconnect history checks count End and CompletionDelivered separately. A live repair snapshot may
contain one End and no delivery yet; a Finished invocation still requires exactly one of each, in
that order. The completion-first controls validate a read-only prefix ending before delivery and
reject that prefix when a completed-history check requires the marker.

Snapshot and physical-pressure fixtures on reflink XFS without quotas or APFS use an explicit
unlimited per-agent disk policy. They exercise snapshot or volume-pressure recovery, not quota
enforcement. A finite limit on unaccounted storage must still fail production validation; turning
filesystem metering off does not disable that independent safety requirement.

Managed XFS CI restores native binaries and component WASMs from the same run. With
`GOLEM_MANAGED_XFS_REUSE_TEST_BINARIES=1`, the runner requires a nonempty restored initial-filesystem
WASM and skips its timestamp-based rebuild. A fresh checkout can be newer than that valid artifact.
Missing prebuilt input is a setup failure, not permission to rebuild the CLI in the VM. Local build
mode retains its source-timestamp check.

P3 `wait-for` records its outer Start before the host-internal clock Start and End. A live history
snapshot can end at either Start. The sleep-history helper waits for the clock input before checking
the original deadline; it still rejects duplicate calls and malformed completed history. Its prefix
regression uses read-only slices of real history, not a replay-cursor change or an injected runtime
pause.

Test observers use `test_observer_filter` at DEBUG with an accurate maximum-level hint. An unbounded
hint enables expensive dependency TRACE argument construction even when output filters discard it.
Keep the exact observer-stack bound and DEBUG-evidence regression; production output filters remain
independent.

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

### Replay-resume request acknowledgement

The internal replay-resume method acknowledges an enqueued wake, not the completion of replay. The
native regression holds the exact uncached input of an original Counter Started/Finished pair during
real Restart reconstruction. The request must acknowledge while that dependency is still held. Only
then release it and require the original Counter state plus the next fresh effect, exact keys and
execution counts, fingerprint, complete raw prefix and original record range without Error or Jump.
An enqueue acknowledgement is not a startup-ready or physical-cleanup receipt.

Main's coalesced wake and Boolean pending demand replace the unrelated per-command processing reply,
counter and replay-request vector. Monthly admission still runs before Store access/replay; dequeue
clears pending demand even when admission refuses. Permit-wait and delayed-retry body tests retain
the exact accepted interruption, establishment join, cancelled acquisition, deferred wake and actual
filesystem-limit reply/payload. Only replay-reply assertions retire with that removed protocol. The
duplicate-wake test exercises the actual capture/apply functions and leaves post-boundary work queued.
Earlier enqueue-bound, library, native, lint, build and browser results are historical.
Combined verification after instrumentation removal is pending.

### Teardown without a private pre-fence pause

Use real pending outcome appends to prove writer and FIFO-capacity ownership. The retained-Suspend
native case keeps its writer withheld while an accepted stop signals, requires the same account
contender to stay blocked, then releases the writer and joins actual cleanup before that contender
acquires. It does not invent a second teardown checkpoint after the writer returns.

Later-start provenance cases first join automatic suspension and physical cleanup, then occupy a
real account permit for the next startup. Check execution APIs, errors and lifecycle markers,
not execution-generation or acquisition counters. The before/after-fence snapshots and await-ready Pending projections retire
with the removed pause. They are not a reason to add another fence or to call append backpressure
the same checkpoint. The loaded-idle compute-admission case retains the original queued key without
guest execution until policy recovery, checks one Suspend and the complete raw prefix, then counts
the next fresh Counter effect. Admission-before-establishment release-order variants have no
surviving production protocol to reproduce.

Completion-first cases do not need a virtual timer when real native completion, the required host
records and an entered invocation-outcome append already establish the scenario. Let the monitor
use its ordinary timer. Check accounting after physical cleanup. Exact capacity receiver counts are checked in the
colocated owned-window close test. A virtual clock's receiver
count is not an additional physical-release guarantee. The P2 input/splice/multi-poll and P3
promise/UDP completion cases retain their other oracles unchanged without those unused freezes.

The provider, transaction and quiet-stdin cases also use ordinary timers and actual
provider/session checkpoints. Applied capacity is observable at the dependency boundary;
that does not establish a monitor sleep poll, private window identity or native
filesystem metadata Pending. Check results, accounting and cleanup independently.

### Monitor cadence with the normal timer

Check exact deadlines on `MonthlyMemoryCheck::duration` and `wait` in a standard paused Tokio
runtime. The colocated tests poll the actual normal sleep before advancing time, reject early
completion and verify rearming after an applied update and a rejected proposal. They exercise the
existing queue function, not a copied monitor loop. This keeps two-period timing proofs independent
of native test setup time.

The colocated contention test holds the actual Interrupts lock across two ordinary virtual Tokio
periods, then releases it and accepts unchanged capacity on the next ordinary period.
Native tests check continuation across healthy capacity updates and interruption after
exhaustion, with accounting, terminal ownership, complete stored prefixes and same-key
recovery. Elapsed time alone does not prove a particular monitor check or window identity.

Join Registry refresh tasks without treating them as physical filesystem completion. A batch
publishes policy and queues the desired limit; the existing Worker can still owe its reply while an
invocation runs. Observe applied policy through the capacity dependency. Keep physical
unload and entered-work completion assertions separate from these batch joins.

### Periodic policy refresh while filesystem application is pending

Use `ResourceLimitsGrpc::new` with its ordinary configured timer, not overlapping manual batches.
Hold an existing filesystem-target reply, then change the Registry from `allowOverage` to exhausted
`hardLimit`. The later revision must reach both that account and another account before the reply
is released. Cover unchanged and changed filesystem limits. This reproduces the actual blocked
production await chain, not only first-snapshot publication.

The bounded latest-value dispatch keeps the existing serialized target callbacks and Worker quota
owner. Separate tests require coalescing to the newest limit, an unchanged desired cache through an
older completion, retry on the next ordinary snapshot after a failure, and no idle strong ownership
of the account entry. Explicit awaited filesystem-update tests retain their completion/error checks.

The native `input/periodic_refresh.rs` cases use only real periodic refreshes while a TCP peer stays
silent. Keep exact stop kind, durable/ephemeral terminals, original key, full committed prefix,
accounting, physical cleanup and normal recovery. Their configured short batch interval is a test
input to the production timer, not a new clock, scheduling sleep or forced tick.

Check prepaid compute and matching/nonmatching storage through applied capacity,
accounting, typed results, disabled-meter zeros and recovery. Precise cadence and
rearming belong to paused-Tokio tests of the actual MonthlyMemoryCheck,
AtomicResourceEntry and queue function. Do not use a usage producer or completed-check
event to reconstruct a private monthly window or count its checks.

A temporary capacity watch observes published capacity, not physical application or
monitor progress. Drop it before cleanup. Exact receiver ownership belongs to colocated
window tests. Managed-XFS cases require real synced allocation and mode-matching caps.
Keep actual-mode stored prefixes through interruption and reconstruction.

For P2 input and P3 UDP, establish prerequisites at the peer/dependency boundary and
check results, durable terminals and accounting. A receive Start does not prove binding
or native Pending. Endpoint reuse requires an independently known endpoint. Earlier
native, library, lint and browser results are historical; current combined verification
is pending.

### Mixed TCP input and filesystem output

Use the existing native input fixture with a real TCP stream and a file output stream. Keep the
accepted peer silent, apply the monthly memory policy and require the typed stop,
socket closure and observable cleanup. Peer entry is not a native Pending receipt.

Keep the original invocation key, exact terminals, actual-mode full committed prefix and ordinary
reconstruction. After capacity returns, send one byte and require the returned splice count, actual
file size and contents to agree. The ephemeral key stays terminal; only a fresh invocation may write
its new file. Existing filesystem observation/drain body tests separately prove entered-work and
generation-lease retention. This does not claim byte-transaction atomicity, output-backpressure
coverage or native managed-XFS measurement.

### Native host waits without virtual clocks

The monitor has no alternate test clock, Worker clock field or clock setter. Both builds use the same
Tokio sleep. Per-export host matrices apply a real exhausted policy after their native peer, promise
or RPC dependency establishes the wait. Storage tests first write and sync a real one-MiB allocation
through ManagedXfs, apply a healthy billing revision and require positive whole-unit usage at that
exact revision. They then exhaust only the matching storage cap and require acceptance at the next
exact revision. Immediate teardown can legitimately add no whole-unit usage at the exhausted
revision. After physical cleanup, deliver queued whole and fractional settlements through a newly
observed current-revision batch before checking that later batches add no usage. These cases run in
the privileged managed-XFS suite; memory and compute cases remain in the ordinary suite.

Keep exact original effects, keys, durable records, typed outcomes, refunds, disabled-meter zeros,
physical release and reconstruction assertions. Shared native metering cases above retain real
periodic/prepaid checks; do not repeat virtual cadence choreography for every host export. The
repeated-stop cases wait for ordinary ticks while their real outcome append remains held, within
their existing whole-test timeout. They still reject a second terminal and a changed-resource update.

RPC refund tests hold real target/caller dependencies and check fuel accounting and
caller continuation. Without an acceptance collector they do not prove a particular
stale proposal was rejected or establish private proposal/refund ordering. Exercise
that acceptance predicate separately on the existing colocated primitive.

A policy update can stop guest observation before a new allocation read finishes. Check that the
source was sampled after the existing cleanup join, not before stop acceptance. Keep the same
positive sampling assertion and the later no-resampling/no-accrual assertions. Exact short-guest
deadline reconstruction remains a separate test contract, not a quota detection deadline.

### Root completion and live tail ownership

Hold real HTTP response bytes and inspect the corresponding incomplete chunk history.
Check the invocation result, lifecycle hints, accounting, complete stored prefix and
ordinary recovery. A held chunk does not establish the instant the Rust root returned,
nor a positive or zero active-task count at settlement. Do not infer these facts from
missing events. The ordinary tail deadline, Notify and cleanup owners remain.

Scoped entity and blob-write tests use their actual provider dependencies, results and
accounting. They do not observe registration or cleanup through resource-producer
events or expose the private weak-flusher registry. Check precise tracker and ownership
properties on existing colocated primitives, separately from whole-Worker composition.

### Idle health with distinct success boundaries

Initial idle settlement precedes guest construction. Fault the existing weak usage flusher during
the monitor's real close-time flush, then require failed readiness for the actual startup attempt,
no guest execution, retained failed health and refusal to start again. A constructor Finished append
is too late to prove this initial boundary.

The later monitor case can hold the real selected Finished append, fault the existing usage
producer, then release the writer and require the original successful Counter result. This is a
pre-commit dependency hold, not post-success idle. Verify the complete raw prefix and exact invocation
records independently of the API result.

Actor loss has a different limit. At that earlier append hold it can fail the in-flight caller with
the actual lifecycle-actor error. That does not prove preservation of an already-published result.
The actor case therefore observes the successful result and real idle release, then starts a later
live RPC and withholds its actual Start commit through the existing oplog dependency. The real
execution window and monitor stay active while that dependency call remains pending. Releasing the
held positive growth append fails the actual lifecycle actor; the ordinary monitor observes its
closed receiver without a synthetic submission. Only then does the test release the RPC commit
and join disposal. It requires the original lost-actor error, physical deletion, retained failed
health, refusal to start, and an unchanged stored prefix. The original successful invocation records
remain exact; the later RPC adds one Started and must fail without a Finished. Do not
infer permit-acquisition counts or resident generation from those records. No idle-close gate
or replacement runtime hook is needed. A hold during startup replay is not equivalent: actor loss
there reaches the outer startup panic path rather than the live monitor-cleanup path under test.

The first positive guest growth can occur in construction. Arm its existing dependency interceptor
before bootstrap and observe its entry before registering a later Finished hold. The entered append
owns its captured gate while the same adapter is rearmed, so it needs no multi-interest registry.

Use a real Directory-backend root for independent deletion evidence. Require the root to exist at
the fault or dependency hold, join final cleanup and require exact metadata NotFound. The aggregate
unload-success projection includes monitor-close failure and cannot independently prove deletion.
Keep the exact retained error, readiness/startup refusal and invocation identity assertions.

### Native wait observations without Worker hooks

Use real peer or dependency boundaries, not a production tracing wrapper or collector.
For TCP and WebSockets, control accepted connections, Upgrade replies and frame bytes.
For RPC, hold the target's actual dependency and check its effects and caller result.
For UDP, use a fixture-controlled endpoint and verify delivery or reuse only when that
endpoint is independently known. An open receive Start alone does not establish binding.

Keep invocation keys, child Starts, durable terminals, effects, accounting and recovery
checks. Do not claim exact native Pending/Returned/Dropped, private window identity,
resource creation, permit counts or lock ordering from these boundaries. A peer request
proves peer entry; a withheld dependency proves that dependency has not completed.

Reader contention fixtures may create an own-agent promise and complete it through the
normal API after checking its recorded response and owner. Foreign promises remain
invalid. Promise completion and Start order do not prove which receive owns the lock or
that a timed reader has polled Pending. Test timeout and result behavior without adding
a gate or reconstructing private lock state from logs.

Keep two proof layers. Real Context integrations check effects, results, history,
accounting and cleanup through observable boundaries. Colocated tests poll the actual
production primitive with controlled inputs and inspect its resources directly.
`native_splice_interrupts_actual_pending_without_output_effect` checks writable output,
typed interruption, no output effect and real resource deletion.
`interrupted_prefix_preparation_keeps_original_real_http_resources` exercises the actual
prepare/install helper and checks stream/body ownership after cancellation. Neither
test establishes every whole-Worker interleaving.

Functional `NativeIoWait` cancellation and the operational permit-held eviction flag
remain. Existing release metrics and invocation-result, scheduler and entity-fence logs
retain their original purposes. There is no replacement telemetry or hook. Earlier
tracing-based validation is historical; combined removal verification is pending.
See [native wait verification boundaries](../../../../golem-worker-executor/native-wait-tracing.md).

### Monitor health at the usage-source seam

Monitor health tests use the existing owner-usage-flusher seam for panic faults, not a Worker panic
setter or an extra select arm. The test adapter signals actual unwind, keeps no Worker/Store, and
stays alive through cleanup. Apply an ordinary policy revision to wake capacity detection. Joining
the refresh is not a physical-cleanup receipt. Preserve initial/later
idle, peer-controlled I/O, exact retained error and observable cleanup assertions.

Actor-loss tests use the existing oplog append-failure fixture, not an orderly shutdown API or a
Worker stopper. Hold a real post-Started GrowMemory append, require its positive delta, then release
it into the provider's ordinary error path. Counter.allocate_memory supplies guest work at later
idle; the pending-TCP case uses its running invocation. The lifecycle job's fail-stop panic drops
the actor receiver, which the normal monitor observes. Test-side capture of the existing before-
fence event matches its exact target, OwnedAgentId, lifecycle kind and message. It unindexes before
delivery, keeps only active interests and does not claim that entity or filesystem cleanup completed.
The original health, cleanup and startup-refusal assertions decide success.
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
Running or Retrying. Check exact native outcomes without a private writer receipt.
The pure monthly mapper cannot replace a deadline or unrelated error with a queued stop.
Earlier establishment-restoration results are historical, not current combined verification
or full upstream SDK or managed-XFS evidence.

### Retained stop markers and physical release

Assert the ordinary lifecycle hints and preserve the exact earlier prefix. An unloaded stop writes
its own Suspend even if an earlier callback has already committed a Suspend for that resident.
A new attempt stopped before establishing an owner also appends its own hint when the resident
counter still names the previous generation. Keep exact marker counts for each exercised path,
separate from invocation-result assertions: duplicate lifecycle hints must not replace committed
results or create results for unstarted work. The taken no-body demand still owns physical disposal;
an absent public receipt is not proof that disposal is complete.

For native cleanup, `tests/api/interruption/lifecycle/health.rs` uses an account limit of one and an
actual FIFO permit contender. It polls that contender Pending while native directory permissions
are changed while Started still holds the execution permit. After actual deletion fails, the same
contender acquires execution capacity while the runtime root still exists. Startup and readiness
still refuse the retained structural failure. Hold that lease through ordinary public Delete after
permissions are restored, then require verified root removal. Returned execution
capacity is not successful filesystem cleanup or an acquisition-count measurement.
Keep this real-Worker evidence separate from queue/predecessor and EntityFence body checks.
The post-success actor case uses an actual live RPC commit, not an idle-close control.

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

The all-meters-off late-stop test can use the scheduler's existing queue event for
its account and agent after the external FIFO contender acquires capacity. That
establishes enqueue, not an execution-lifetime close receipt. Check the later stop
through the execution API, result and history. Do not add a scheduler wait hook or
treat establishment as physical unload.

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
it observes ordinary status rather than invent a Worker receipt. Read-only status is not a pause
point or a separate lifecycle protocol.

### Stop freeze capture and native prerequisites

There is no freeze operation, frozen projection or immutable publication cause. Retire tests for
that protocol with its producers. Main may signal Restart before establishing a later terminal
request; do not restore the stronger single-elected-signal rule through another gate or coordinator.

Native completion and pending-Suspend cases hold real terminal persistence. Subscribe raw before
public `set_interrupting` and retain its exact returned Some receiver, not a private writer
projection. A stop can signal during that append. The callback and real teardown still retain the
Store and permit until required work finishes. Require one invocation outcome, unchanged key and
history, physical cleanup and recovery; do not accept extra quota errors. The pending-Suspend
case consumes the later stop at invocation exit and still requires one hint and one receipt.
A separate unloaded stop writes its own hint without deduplication against an earlier Suspend;
that is not a second invocation-failure callback or an invented result.

The shared retained-terminal helper's first automatic phase has no public Some promise. Its private
after-freeze receipt subscription is gone. At the already-held teardown checkpoint, retain the
actual selected result, held permit and already-polled real FIFO
lease. Create and poll the existing AwaitReady future before releasing that same teardown hold. Then await the same
lease, await readiness, and join accepted work and cleanup. Do not substitute a fresh lease, new barrier
or an early empty join before automatic acceptance. Observe the automatic cause from the actual
owner fence after joined cleanup, not a deleted writer slot. Exact owner cause and final
marker facts remain native assertions; generation and acquisition counts are not inferred.

Waiting retains its real append and public Some contract above, without instance queries or competing
stops while persistence owns the instance mutex. BeforeOwner/RestartFollower retain their held admission
before generation begin and actual driver join. Later pending demand, exact startup/receipt contracts
and one-versus-two Suspend provenance remain required. Neither proof layer alone establishes the old
controlled whole-Worker compound interleaving.

### Monthly acceptance guard and applied capacity

No monthly acceptance collector or replacement gate remains. Native tests check
applied capacity, execution state, typed results, accounting and committed history.
Neither silence nor a later result proves a particular proposal's acceptance boolean
or private window identity. Colocated tests exercise exact acceptance results and
literal target identity through the existing production guard.
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
live-window stale-message delivery. Cached-owner continuation tests check observable
interruption and recovery, not a traced window/attempt/generation transition.
Old-target rejection belongs to the body predicate, not a fabricated old completion or late false ack.
Retirement cases retain first-cause/history/cleanup facts without
claiming particular late actor-message processing. Interrupt-lock retry is tested colocated with the
actual lock and ordinary virtual Tokio periods, not a native Worker test hook.

The service publishes applied monthly capacity and queues desired filesystem limits without waiting
for physical application. A replay, terminal append or deletion hold can delay the existing Worker
reply, but cannot hold the batch or later refresh cadence. Subscribe and mark the capacity watch
seen before each controlled refresh; await a changed version and check exact capacity, revision,
period, fuel generation and exhaustion. Watch version is not policy revision
or refresh generation, and batch completion does not prove physical quota application. Drop the
temporary reader before physical cleanup. Exact monitor receiver counts are checked by the colocated
owned-window close test, separately from native cleanup observations.
Release real dependency holds and join physical cleanup separately. These are read-only dependency
observations, not a new Worker interface.

### Filesystem observation without a pre-poll gate

The keyed filesystem gate waited on its own semaphore before polling metadata. Remove its registry,
public type, stat/stat-at wrappers and synthetic native cells together. Do not rename that wait as
native metadata Pending. Real blob-provider Worker tests retain effects, original Start and key,
typed monthly stop and joined physical cleanup. Actual filesystem body tests separately retain a
started call and its generation lease after guest observation is interrupted, then require drain
before deletion. This factorized evidence does not claim a stalled native metadata syscall.

### Deletion dependency entry and consistent service composition

Use actual `OplogService::delete` entry for storage-removal backpressure, keyed by the real
`OwnedAgentId` and `AgentMode`. The test-owned service adapter waits before delegating storage
mutation. Its receipt is not native filesystem Pending or completed deletion, and sender Drop
releases it. `OwnedWorkJoined` has closed the resident oplog at this point. Read the authoritative
service in the actual mode, retain the complete committed prefix without Jump and require storage
absence after deletion. A durable-only helper or a read from the closed actor is not equivalent.

Apply the existing service decoration once at bootstrap construction, before worker metadata,
enumeration, schedulers and executing workers retain the service. Decorating only the late `All`
graph leaves already-constructed consumers holding the raw service. The construction decorator's
production default returns the same Arc; it adds no Worker state, per-lifecycle callback or protocol.
It replaces the late test-bootstrap override and forwarding shim rather than creating another path.

The quota-first case needs no extra ExecutionFenced pause after selecting its actual terminal. The
deleting-first case retains one fixed fence before any terminal is queued, with outer Deleting
around a matching Running resident. Interrupt-mutex contention would also reject acceptance and
could mask a missing production outer guard. This is the minimal deletion exception, with no
stage selector. Dropping its release sender also releases it.

Main's WorkerDeletionHook is wired by TestWorkerCtx, not production Context, which uses
NoAdditionalDeps and the default None hook. These quota cases deliberately use production Context
for real accounting. Installing the existing DeletionStageHook on the test graph cannot pause that
worker. Do not add a new Context bridge or change the test's context just to replace this one
boundary. Keep the exception explicit instead. All original first-cause, exact terminal/error/result,
key, receipt, identity, accounting and physical-cleanup oracles stay strict. The user-only case
retains its original resident history read.

For prepaid RPC caller-first cases, an applied account update wakes both Stores. The target can stop
and refund at invocation exit before the caller's proposal is validated. A terminal-append hold is
too late to prevent that refund. Use the existing real target GrowMemory-append dependency to keep
its lifecycle actor behind that append through caller cleanup. Assert the actual positive append
and its absence from committed history while held. After exact caller acceptance and physical
cleanup, require restored capacity, release the append and drain the target's lifecycle mailbox.
The target must remain live with no monthly stop. Complete its original promise, require the single
logical increment, then use ordinary idle eviction to close its window before the same two-Store
refund assertions. Registry refresh completion is separate from the target's physical filesystem
reply; preserve its normal completion and cleanup owner. Keep the separate
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
