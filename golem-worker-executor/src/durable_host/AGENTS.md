# Durable host functions

Rules for code under `durable_host/`. Mechanisms, timelines and crash matrices are in the
`understanding-durable-execution` skill.

## Execute, append, claim, observe are four different steps

Vocabulary: a durable call is a `Start` entry plus its *terminal* (`End` or `Cancelled`), both
identified by the `Start` index. A *claim* is the replay step that matches the host call the
guest is making now against the next recorded `Start` (`ReplayState::claim_start`,
`replay_state/cursor/`; the identity is a `StartClaim` from `replay_state/claims.rs`: function
name, owner, optional request payload). *Observe* is the guest reading the result.

- **Execute** a live effect only when the durable context reports live
  (`store_is_live` / `is_live()` on the `DurableWorkerCtx`), never because the replay cursor is
  exhausted. Cursor exhaustion is an observation. The primary Store is live only after
  `ReplayState::switch_to_live` has settled every `HistoricalReconstruction` fence and published
  the shared flag; an entity Store is live only after its own `PendingReplayToLive::finish_at` set
  `local_live_tail` (a `ReplayingCompleted` entity Store is never live).
- **Append** `Start` eagerly when a durable call begins and `End`/`Cancelled` when it terminates,
  through `begin_durable_function` / `end_durable_function` (`durability/`) and a
  `DurableCallSession` (`concurrent/call/`). A trap leaves `Start` incomplete
  (`abandon_for_trap`); never write `Cancelled` for a trap.
- **Claim** during replay by identity (`StartClaim`, optional request-payload match). Scan-ahead
  claiming exists only to route concurrent completions to the right awaiter; a `Start` that does
  not exist in history is a divergence error, not something to skip past. Actual protocol
  parents retain their own identities, including `<scope:transaction>` for RDBMS; generic
  batched scopes do not exist. A claim never falls back to a differently named sibling.
- **Observe**: the guest's consumption of a completion is recorded by `CompletionDelivered` /
  `CompletionDiscarded` as the tail operation of the host function
  (`supersede_prior_completion_delivery` hard-errors otherwise). This applies to accessor
  (concurrent) calls whose results reach the guest: replay releases each completion at its recorded boundary
  (`CompletionDelivery::AtMarker`). Live and replay release recorded async events only at a
  blocked wait, at most one event per block, through the consuming Store's
  `RuntimeStore::await_guest_blocked`. Non-blocking polls and yields do not admit events; a
  busy guest must ask through recorded synchronous host calls. A guest-facing accessor `End` without a marker is a
  crash-after-completion and is withheld until the natural replay tail (`AtReplayTail`), never
  re-executed and never delivered early. `CompletionDiscarded` means the *guest* dropped the
  completion unread; an armed token or terminal guard torn by Store unload or executor shutdown
  (`TeardownProbe`) must leave the `End` markerless, otherwise replay withholds a completion the
  guest never saw. The serialized direct path records no markers
  (`DurableCallSession::replay` rejects `CompletionDelivered` for a non-accessor call) and
  delivers at the host return.
- **Host-internal results** retain durable Start/End and cancellation tracking, but create no
  guest-terminal observer, delivery token, delivery/discard marker, or replay-tail gate. Use
  `invoke_host_internal_access` with an outer call that owns cancellation until the host
  continuation finishes. A completed internal End returns its recorded result to the host.
  Never make a Store-holding operation wait for progress that requires that Store's event loop.

## Identity on the replay path

Any value replay must reproduce (idempotency keys, phantom ids, scope discriminators, span ids
that reach the guest) derives from oplog indices or recorded data — `derive_idempotency_key`,
`ephemeral_invocation_phantom_id` (UUIDv5 of the key). Fresh randomness or clock reads are
allowed only when appended and committed before the guest can observe them and read back on
replay (as `caller_attempt_id` does); unrecorded guest-observable randomness is a bug.

## Repair is owned by the call's protocol

`DurableFunctionType` has four cases: `ReadLocal`, `WriteLocal`, `ReadRemote`, and `WriteRemote`.
Nesting is explicit in `Start.parent_start_index`. Top-level incomplete calls repair in place.
Ambient entity lineage preserves ordinary call-local repair, but never authorizes an explicit
protocol child. An explicit child requires its owner's `AccessClaimOptions::incomplete_repair_authorized_by_scope`
authorization for repair or typed loss; an unauthorized child reaching generic repair is an
invariant error naming the function and Start index. `ScopeReplayRecovery` retains protocol
recovery/readiness decisions. Generic scope wrappers do not record additional Starts or Ends.
RDBMS protocol state prevents statement in-function retry; function types do not encode
transaction membership. Preserve both inline and trap-path tests (`tests/in_function_retry/`).

Decide child authorization and protocol readiness before publishing live. Terminal refusal
abandons replay for failure (`authoritative_replay_failure`); it must not publish over sibling
history. A typed-loss `End` belongs to the original `Start`. Never rewind or discard another
task's history to make repair possible. HTTP without a safe resend reports
`http-response-incomplete`, RDBMS reports `resource-lost`, and TCP reports `connection-broken`.
UDP `ReadRemote` re-executes receive after rebind rather than manufacturing connection loss.

Preview 2 HTTP is removed. Preview 3 send keeps request-body intake running while awaiting
replay resolution: a streamed writer must reach its next durable call without deadlock.
Every method uses `Cancellable` guest drop, records `Cancelled`, and does not resend on replay.
Only crash-like loss leaves an incomplete send eligible for repair. A trap is never cancellation;
guest panic and deterministic traps are permanent failures, without changing infrastructure retries.

A P3 HTTP send uses `WriteRemote`. It repairs an incomplete `Start` in place and keeps the
operations recorded next to it. Nonresumable RDBMS handles complete incomplete calls with typed
loss under the original `Start`, without replacing the transaction or requerying the stream.
Transaction phase hints carry their owning scope's `Start` and are indexed before replay;
they must not be consumed positionally by another transaction.

## Saga attempts use semantic retry policies

After complete compensation, infallible SDK sagas resolve verb `saga` against the agent URN
with `attempt` and `error-kind` properties. The host's `next-retry-delay` uses the existing
retry evaluator and an opaque continuation; SDKs use durable clocks and timers. When no rule
matches, use the agent default. Reject unbounded saga policies; exhaustion and policy errors
return typed failures preserving the original failure. Each attempt and compensation appends
history; the policy bound keeps that history finite. Partial compensation stops retries.

TS, Effect, Scala and Go compensate catchable failures in reverse order. Propagate the original
exception or panic only after complete rollback; partial rollback retains both failures.
Effect external cancellation stops retries while retaining failure evidence in the Cause.
Rust WASM panics abort without compensation; Result failures, including exhaustion and partial
rollback from `infallible_transaction`, remain typed. MoonBit has no public saga helper.

## RPC

The key is derived from the durable call's `Start` index and reused by every attempt.
Streaming and non-streaming RPC follow the same
rule. Streaming session descriptors must exclude retry tracing and canonicalize environment-map
ordering without discarding execution-relevant configuration.
Only the first live dispatch to an ephemeral target may use
`InvocationFreshnessDisposition::KnownFresh`, after the caller durably commits its `Start`.
Every repair, retry, and replay dispatch uses `InvocationFreshnessDisposition::MayExist`.
Same-key retry attaches to the existing target invocation; it is never new work.
RPC preparation does not acquire target execution capacity. An execution-needing target
invocation durably enqueues its acceptance prefix before reporting acceptance, and caller
cancellation must not split that prefix. Settled read-only cache hits and coalesced followers do
not persist a separate invocation, result, or alias; only the miss owner follows normal durable
admission. Ephemeral targets are fail-stop; do not build resumption for them.

Owned asynchronous RPC operations register with the owner's suspension coordinator when the
operation starts, including before the guest consumes the result. Their passive remote waits
can qualify after `rpc_suspend_after`, but every owner participant must also be eligible and
runtime-blocked. Active work and borrowed synchronous RPC/preparation veto automatic suspension.
The coordinator rechecks after `wait_suspend_check_interval`, durably schedules a wakeup
`rpc_resume_after` later (or the earliest wakeup among mixed waits), then revalidates eligibility
before suspending. Ordinary reconstruction uses the same logical RPC key. This is proactive
scheduling only: never gate explicit interruption or arbitrary Store loss on eligibility, and do
not add a feature-specific recovery path or immediate restart.

## Durable streams

- The producer's oplog (`StreamRegistered`/`StreamItems`/`StreamEnd`/`StreamCancel`) and the
  consumer's `StreamSession` journal are the only authoritative stream state. Commit before
  `DurableLiveStreamBus::publish_committed`; the bus, readers, sockets and transports are
  resumable optimizations and must never be the only holder of an item or terminal.
- Consumers resume by `StreamOffset` / `consumer_read_ordinal`, never by connection state.
- Stream liveness is bound to the durable `AgentFingerprint` of the producer; every place a stream
  identity crosses a boundary checks it (`validate_forwarded_mapping`, producer-side
  `CorruptHistory` checks). Reject mismatches rather than reattaching to a recreated agent.
- Every stream is finalized exactly once. Protocol terminals appended on invocation completion or
  failure fence later guest terminals; do not add a second finalization path.
- Reconstruct terminal outputs from committed records without requiring the finished consumer to
  reattach. Open agent-RPC outputs still require an active attachment before production.
- Stream entries are hints and must stay hints: they take part in no `Start`/terminal pairing and
  never satisfy a claim. Guest-consumed `ConsumerItemValue` and `ConsumerTerminal` records reserve
  their replay position until Wasmtime's per-read observer confirms guest consumption. Host
  projection records remain ordinary skippable hints.
- The RPC result is completed (stripped of streams) before the stream-bearing value reaches the
  guest; do not assume an open stream implies an incomplete RPC `Start`.
- Native guest reads append one consumer journal record at Wasmtime's per-read consumption
  observer, not at host take time. Its oplog position is the guest boundary; there is no extra
  durable call or delivery marker. After cursor/source readiness, guest observations wait for
  a fresh blocked wait and share the Store's one-event-per-block gate with accessor completions,
  live and replay alike. Replay first claims every earlier retained Start of the consuming
  invocation, including host-internal descendants; import issuance alone does not release a read.
  Host-side takes do not use this gate. Packed bytes record only the consumed portion. A crash before
  consumption leaves no record and recovery re-reads the source by offset. Host projections keep
  take-time journaling. A zero-progress native read cancellation retains the producer's receive
  and prepared observation for the next read; only dropping the readable end cancels the durable
  source. A retained receive keeps its original runtime activity binding; a later transfer stays
  unclassified until it completes. Finishing the original activity is not owner suspension and
  must not prevent the receive from returning. Native read cancellation does not yet reconstruct
  the inline-versus-blocked return disposition. If cancellation cannot retain a recorded guest
  delivery, replay fails with `UnexpectedOplogEntry` and poisons the shared cursor instead of
  discarding the value or waiting forever. The canonical cancellation policy is unchanged; do not
  force always-blocked returns.
- Retained history preserves logical consumer input, including its ordinal, source offset and nested
  reader identities. Re-executing a read whose physical boundary a cut deleted appends one
  consumer record with `reobserves` referencing the original record. Append and replay validate
  identical logical content. A re-observation reserves its new guest boundary but advances no
  consumer progress and introduces no nested readers; their identities retain the original index.
- The stdin of a tool call that a client started is read by the component body itself
  (`DurableToolStdinProducer`): each chunk and the terminal its guest takes is a guest observation
  whose parent is the entity `Start`, or the custom or batched scope that is open at the read. Do
  not put a host task or a buffer between that durable input and the guest: what a host takes ahead
  of the body has no place among the body's recorded calls, and a replay then hands it over too
  early or, after a restart, in another order. In completed replay a read past the recorded journal
  stays pending. A reader that is dropped cancels nothing; the end of the session cancels the unread
  input. The records keep the session's attribution, so `has_visible_scope_descendant` consults the
  cursor's `stream_observations` to see that such a body has started; do not send it through the
  empty-scope shortcut. Only a native body (`EntityActivationSource::Host`) gets an attachment that
  a host task fills.

## Entity bodies and tool invocations

An *entity* is a transient guest body that runs inside an agent's durable context but is not the
agent: a tool sidecar or a tool middleware (`AgentEntity::{Tool, ToolMiddleware}`,
`golem-common/src/model/entity.rs`). It gets its own Wasmtime `Store` and attaches to the
owner's `AgentFilesystem` generation (`OwnerRuntimeResources`, `worker/instance/mod.rs`), so its
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
  `ReplayingIncomplete`/`Live` become live when `PendingReplayToLive::finish_at` sets
  `local_live_tail`, which for incomplete tool entities first requires
  `activate_live_attachment_memory_accounting` to admit them. Completed replays use historical
  memory charges, not current pressure.

## Snapshotting

Snapshot save/load runs in snapshotting mode (`durability_is_suppressed` ==
`snapshotting_mode`): durable calls neither append nor consume entries (`DurableCallSession` with
`persisted: false`). Snapshot admission is governed by `SnapshotBoundaryConditions`; do not add
parallel boundary predicates.
Automatic snapshots are revision-scoped baselines chosen at instance creation
(`worker/snapshot_selection.rs`): the last usable entry, else the previous usable entry that the
status fold keeps. A deterministic load failure or divergent replay suffix rejects exactly that
entry and recreates the full instance context; the start then tries the previous usable entry,
and else the authoritative manual-update baseline (never pre-migration history). The
fingerprint-scoped set of rejected entries is persisted only after that fallback succeeds and
before readiness is published; payload-download and filesystem-restore failures skip only that
entry for the startup attempt.
Failure to load a manual-update snapshot is terminal and retains the underlying cause.

## Spawned store tasks

Tasks spawned on the store (`tail_work.rs`) may park across invocation settlement at guest-driven
waits or passive markerless-completion replay-tail waits after durable finalization. Cursor
operations and recorded-marker waits remain active. Any durable `Start`/`End` they can still
produce must land before `AgentInvocationFinished`. Work that must survive a restart belongs in
the oplog, not in a task.
