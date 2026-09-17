# Durable host functions

Rules for code under `durable_host/`. Mechanisms, timelines and crash matrices are in the
`understanding-durable-execution` skill.

## Execute, append, claim, observe are four different steps

Vocabulary: a durable call is a `Start` entry plus its *terminal* (`End` or `Cancelled`), both
identified by the `Start` index. A *claim* is the replay step that matches the host call the
guest is making now against the next recorded `Start` (`ReplayState::claim_start`,
`replay_state/cursor.rs`; the identity is a `StartClaim` from `replay_state/claims.rs`: function
name, owner, optional request payload). *Observe* is the guest reading the result.

- **Execute** a live effect only when the durable context reports live
  (`store_is_live` / `is_live()` on the `DurableWorkerCtx`), never because the replay cursor is
  exhausted. Cursor exhaustion is an observation. The primary Store is live only after
  `ReplayState::switch_to_live` has settled every `HistoricalReconstruction` fence and published
  the shared flag; an entity Store is live only after its own `PendingReplayToLive::finish` set
  `local_live_tail` (a `ReplayingCompleted` entity Store is never live).
- **Append** `Start` eagerly when a durable call begins and `End`/`Cancelled` when it terminates,
  through `begin_durable_function` / `end_durable_function` (`durability.rs`) and a
  `DurableCallSession` (`concurrent/call.rs`). A trap leaves `Start` incomplete
  (`abandon_for_trap`); never write `Cancelled` for a trap.
- **Claim** during replay by identity (`StartClaim`, optional request-payload match). Scan-ahead
  claiming exists only to route concurrent completions to the right awaiter; a `Start` that does
  not exist in history is a divergence error, not something to skip past. Scopes are named
  `<scope:batched-write>` or `<scope:batched-write:DISCRIMINATOR>`; a discriminated claim never
  falls back to a plain-named sibling.
- **Observe**: the guest's consumption of a completion is recorded by `CompletionDelivered` /
  `CompletionDiscarded` as the tail operation of the host function
  (`supersede_prior_completion_delivery` hard-errors otherwise). This applies to accessor
  (concurrent) calls: replay releases each completion at its recorded boundary
  (`CompletionDelivery::AtMarker`); an accessor `End` without a marker is a
  crash-after-completion and is withheld until the natural replay tail (`AtReplayTail`), never
  re-executed and never delivered early. The serialized direct path records no markers
  (`DurableCallSession::replay` rejects `CompletionDelivered` for a non-accessor call) and
  delivers at the host return.

## Identity on the replay path

Any value replay must reproduce (idempotency keys, phantom ids, scope discriminators, span ids
that reach the guest) derives from oplog indices or recorded data — `derive_idempotency_key`,
`ephemeral_invocation_phantom_id` (UUIDv5 of the key). Fresh randomness or clock reads are
allowed only when appended and committed before the guest can observe them and read back on
replay (as `caller_attempt_id` does); unrecorded guest-observable randomness is a bug. Inside
atomic regions use the outermost region's logical counter (`current_idempotency_key_oplog_index`),
not the physical index: a rollback re-executes the region at new physical positions.

## Retry eligibility is part of the function type

`DurableFunctionType` decides two things with one predicate (`is_eligible_for_internal_retry`
== `can_reexecute_on_incomplete_replay`, `durability.rs`): whether a failed call may be retried
in-function under the same `Start` instead of trapping and reconstructing the worker, and whether
an incomplete `Start` may be re-executed on replay. `ReadLocal`/`ReadRemote`/`WriteLocal` always;
`WriteRemote` only under `assume_idempotence`; batched/transaction writes never. Choose the type
for re-execution safety, and test both the inline path and the fall-back-to-trap path
(`tests/in_function_retry/`).

## RPC

The key is derived from the durable call's `Start` index, or the outermost atomic region's
logical counter, and reused by every attempt. Streaming and non-streaming RPC follow the same
rule. Streaming session descriptors must exclude retry tracing and canonicalize environment-map
ordering without discarding execution-relevant configuration.
`InvocationFreshnessDisposition::MayExist` is the default; `KnownFresh` is allowed only for live,
ephemeral, non-`assume_idempotence` dispatch and must commit `DurableOnly` first
(`wasm_rpc/mod.rs`). Same-key retry attaches to the existing target invocation; it is never new
work. RPC preparation does not acquire target execution capacity. An execution-needing target
invocation durably enqueues its acceptance prefix before reporting acceptance, and caller
cancellation must not split that prefix. Settled read-only cache hits and coalesced followers do
not persist a separate invocation, result, or alias; only the miss owner follows normal durable
admission. Ephemeral targets are fail-stop; do not build resumption for them.

Pending durable RPC operations register suspendable waits when the operation starts. After
`rpc_suspend_after`, their await uses the shared voluntary-suspension predicate; if HTTP or other
live work makes the Store ineligible, it retries after `wait_suspend_check_interval`. Before
suspending it durably schedules a wakeup `rpc_resume_after` later (or the earliest wakeup among
mixed waits), then uses ordinary reconstruction with the same logical RPC key. This is proactive
scheduling only: never gate explicit interruption or arbitrary Store loss on this predicate, and
do not add a feature-specific recovery path or immediate restart.

## Durable streams

- The producer's oplog (`StreamRegistered`/`StreamItems`/`StreamEnd`/`StreamCancel`) and the
  consumer's `StreamSession` journal are the only authoritative stream state. Commit before
  `DurableLiveStreamBus::publish_committed`; the bus, readers, sockets and transports are
  resumable optimizations and must never be the only holder of an item or terminal.
- Consumers resume by `StreamOffsetV1` / `consumer_read_ordinal`, never by connection state.
- Stream liveness is bound to the durable `AgentFingerprint` of the producer; every place a stream
  identity crosses a boundary checks it (`validate_forwarded_mapping`, producer-side
  `CorruptHistory` checks). Reject mismatches rather than reattaching to a recreated agent.
- Every stream is finalized exactly once. Protocol terminals appended on invocation completion or
  failure fence later guest terminals; do not add a second finalization path.
- Reconstruct terminal outputs from committed records without requiring the finished consumer to
  reattach. Open agent-RPC outputs still require an active attachment before production.
- Stream entries are hints and must stay hints: they take part in no `Start`/terminal pairing and
  never satisfy a claim.
- The RPC result is completed (stripped of streams) before the stream-bearing value reaches the
  guest; do not assume an open stream implies an incomplete RPC `Start`.

## Entity bodies and tool invocations

An *entity* is a transient guest body that runs inside an agent's durable context but is not the
agent: a tool sidecar or a tool middleware (`AgentEntity::{Tool, ToolMiddleware}`,
`golem-common/src/model/entity.rs`). It gets its own Wasmtime `Store` and attaches to the
owner's `AgentFilesystem` generation (`OwnerRuntimeResources`, `worker/instance.rs`), so its
filesystem effects are the agent's; that is why completed bodies are re-executed on replay
rather than replaced by a recorded result.

- Entity bodies (`entity.rs`, `tool/`) record into the **owner's** oplog with
  `parent_start_index`; they have no oplog or cursor of their own. Entity Stores clone the owner's
  `ReplayState` and share its cursor.
- The entity `Start` index is the entity invocation id and its terminal is a plain `End` or
  `Cancelled`. Dropping before terminal selection leaves the `Start` incomplete
  (`LeaveIncompleteOnDrop`); a trap appends no terminal. There is no outer `call-tool` durable
  call around the entity invocation.
- Completed replay (`InvocationExecutionMode::ReplayingCompleted`) re-executes the body's guest
  export with its durable host calls replayed from the owner oplog, validates the claim, then
  releases the recorded terminal. Completed external effects are never performed again; the guest
  export itself does run again — unless the recorded terminal carries
  `body_execution: Skipped` (admission rejected before the body ran), in which case
  `execute_recorded_skipped_tool_call` returns it without a body. Incomplete replay completes
  under the original `Start` index.
- An entity Store's liveness is its own (`store_is_live`): `ReplayingCompleted` is never live;
  `ReplayingIncomplete`/`Live` become live when `PendingReplayToLive::finish` sets
  `local_live_tail`, which for incomplete tool entities first requires
  `activate_live_attachment_memory_accounting` to admit them. Completed replays use historical
  memory charges, not current pressure.

## Snapshotting

Snapshot save/load runs in snapshotting mode (`durability_is_suppressed` ==
`snapshotting_mode`): durable calls neither append nor consume entries (`DurableCallSession` with
`persisted: false`). Snapshot admission is governed by `SnapshotBoundaryConditions`; do not add
parallel boundary predicates.
Automatic snapshots are revision-scoped baselines chosen at instance creation. A deterministic load failure or
divergent replay suffix rejects that snapshot through its index and recreates the full instance
context from the authoritative manual-update baseline (never from pre-migration history). The
fingerprint-scoped `rejected_periodic_snapshot_through` watermark is persisted only after that
fallback succeeds and before readiness is published; payload-download failures use a temporary skip for the startup attempt.
Failure to load a manual-update snapshot is terminal and retains the underlying cause.

## Spawned store tasks

Tasks spawned on the store (`tail_work.rs`) may park across invocation settlement at guest-driven
waits or passive markerless-completion replay-tail waits after durable finalization. Cursor
operations and recorded-marker waits remain active. Any durable `Start`/`End` they can still
produce must land before `AgentInvocationFinished`. Work that must survive a restart belongs in
the oplog, not in a task.
