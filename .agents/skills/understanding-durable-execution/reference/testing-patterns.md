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
    used in `tests/api.rs`).
  - `TestExecutorOverrides::wrap_shard_service` — fake ownership changes.
  - `executor.oplog_service_call_count(&worker_id, "read_exact")` — prove a path did not scan
    the oplog.
  - `probe_http_body_read_starts`, `external_end_payload_id`, the exact-payload download outage,
    and `probe_runtime_disposal` — target an uncached p2 response-body recovery payload and prove
    that each failed physical runtime is destroyed before replacement.
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

Copy: `tests/api.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart` — 32
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
repaired under the existing entry). A second physical `Start` for the same logical call appears
under atomic-region rollback (`Jump`, both attempts carry the same key) and inside a custom
durable invocation recovered incomplete (the nested calls are re-recorded with
`observational_owner`, and the block author owns their idempotency).

Copy: `tests/api.rs::lost_card_transfer_response_converges_after_source_and_target_restart` —
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

Copy: `tests/api.rs::p3_promise_suspend_survives_executor_restart`,
`tests/rpc.rs::ts_cancel_survives_executor_restart`,
`tests/durability.rs::snapshot_based_recovery_preserves_state_across_multiple_restarts`.

## Pattern 5: completed replay performs no new effect

Mistake caught: re-executing a recorded call during replay.

Do: stop or count the external dependency before restarting. With a wrapped RPC or storage
service, assert its call count is unchanged after a restart that replays a completed call. With
an HTTP test server, stop it before restart; replay must still succeed.

Copy: `tests/api.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart` — the
`read_exact` count is unchanged across the repeated-key lookup, proving the recorded result came
from the physical index rather than an oplog scan; the effect assertion is the `33` on the fresh
key. `tests/rpc.rs::durable_streaming_output_recovers_after_executor_restart` — after the restart
the consumer receives exactly `66 - observed` remaining items and each output stream ends once,
so no already-observed item is re-emitted.

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
`tests/api.rs::pending_self_card_transfer_recovery_does_not_deadlock`,
`lost_card_transfer_response_converges_after_source_and_target_restart`.

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
a setup or admission failure before admission, which has no pinned `seed_flusher` to flush.
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
fixture that the end of this section names:

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
