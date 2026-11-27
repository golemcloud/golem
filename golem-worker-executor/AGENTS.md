# Worker Executor

Load the `understanding-durable-execution` skill before changing replay, oplog, host-call,
RPC, durable-stream, tool/entity, snapshot, or worker-lifecycle code. This file states the rules; the skill explains the
mechanisms, timelines and tests behind them.

Keep `worker-executor-walkthrough.html` and the `understanding-durable-execution` skill aligned
with executor behavior changes in the same PR. Update affected explanations, diagrams and
interactive examples; preserve unrelated content and verify changed walkthrough sections in a browser.

## Axioms

1. **Replay is deterministic.** The guest is a deterministic function of its guest-observable
   host inputs, and every such input is recorded in the oplog. A replaying guest that asks for a
   host call different from the one recorded next has exposed an executor bug (missing recording,
   wrong reconstruction, wrong completion association). Fix the recording or reconstruction; never
   add tolerance (skipping, reordering, synthesized results, "stable IDs") to replay.
2. **The resident runtime is disposable.** `Store`, worker task, process, sockets, channels and
   subscriptions may vanish at any instruction boundary. Suspend, evict, reshard, restart and
   crash all reconstruct the instance from the oplog through the same path
   (`src/worker/invocation_loop/mod.rs::run` → `src/durable_host/recovery.rs::prepare_instance` →
   `replay`). Do not design feature-specific "safe to unload" gates; make every pending
   obligation durable or deterministically recomputable instead.
3. **Durable agent RPC is exactly-once per logical target invocation.** Caller attempts (replay,
   transport retry, individual call repair) share one idempotency key derived from the caller's
   invocation key and the durable call's own `Start` index (`derive_idempotency_key`);
   the target persists one invocation and one
   result. Attempts are not executions. This persistence rule applies to RPCs that execute: a
   settled read-only cache hit or coalesced follower returns the shared cached result without a
   separate target invocation, result, or alias.

## Durable versus resident state

Authoritative: oplog entries and payloads, `PendingAgentInvocation` / `AgentInvocationStarted` /
`AgentInvocationFinished` / `AgentInvocationSettled`, idempotency keys and recorded results,
`Start`/`End`/`Cancelled` and `CompletionDelivered`/`CompletionDiscarded` markers, update and
snapshot entries, and status folded from the oplog. Derived and disposable: `Store` memory,
in-memory queues, caches, event subscriptions, spawned store tasks, sockets. Never let
correctness after a restart depend on anything in the second list.

A host resource drop that has no guest-observable result and no remote effect must not open a
durable call: the guest may release the resource at different points in different runs (garbage
collection, a heap restored from a snapshot). Delete the resident state and record a hint for
observability, as `wasm-rpc` and `span` drops do with `SpanClosed`.

A drop that terminates a real durable effect must preserve the operation's terminal, but must
never wait for recorded history on the guest thread or in a cleanup drain. Defer replay waits
under the resident runtime's task ownership. Missing terminals may continue live only at the
natural replay tail. A changed disposal time does not permit changed guest reads, completed
effects, call identities, or terminal kinds.

## Invocation settlement

The result of a successful constructor and of an agent method whose declared result type contains no stream is recorded
(`AgentInvocationFinished` or `AgentInvocationFailed`, with `awaits_settlement`) at the export's
return, when the guest calls `task.return`. A constructor error is not recorded at the return: its
background work drains first, within both `max_invocation_duration` and
`tail_work_settle_timeout`, and the error then takes the constructor's fatal path with no
`awaits_settlement` terminal. Tool children that already run, or need no admission,
settle before the result is recorded, and their failure fails the invocation. The background work
of that invocation (spawned guest tasks, active store tasks, retained durable calls) then drains,
bounded by neither `max_invocation_duration` nor `tail_work_settle_timeout`; capable tool children
staged until their parent ends are admitted at the end of the drain (`end_tool_parent`) and settle
before `AgentInvocationSettled`. The end of the drain records the hint
`AgentInvocationSettled { invocation_id, consumed_fuel }`, and only then does the next invocation
start (`worker/invocation.rs::record_at_return_and_drain`). Stream inputs do not delay this terminal.
Input reads after the return are background work. An input still open at the end of the drain
ends consumer-cancelled before `AgentInvocationSettled`; session completion waits for every
input terminal, including forwarded inputs. Methods whose declared result type contains a stream,
snapshot save and load (including a manual update's),
oplog-processor batches and external-tool invocations keep the settled path: their terminal comes after their work, and they write no
`AgentInvocationSettled`.

A method whose declared result type contains a stream records `AgentInvocationReturned` with
the value and its bindings in the same batch as its `StreamRegistered` entries. This includes
an `option<stream>` that returns `none`, with no bindings. Its successful terminal holds
`FinishedResult::Returned` and refers to that entry; the terminal of a scalar result holds
`FinishedResult::Payload`. The callee writes no session `RemoteResult`. Cancellation or
failure after an early return preserves the early value, but lookup reports completion only
after a terminal. RPC caller journals and entity-attributed tool sessions retain their
`RemoteResult` records. RPC and component-tool terminals refer to that journal entry when it
exists and carry an inline value otherwise. Replay resolves the reference; the public oplog
shows the stripped value. A tool session has a `Local` key but is not an agent invocation's
result; an agent invocation return must have no entity attribution. The outer native-tool
client session also has an unattributed `Local` key: its prepared target is `ExternalTool`,
not `AgentMethod`. It writes `RemoteResult` after the native entity terminal, so that terminal
stays inline.

- **Replay** consumes the terminal at the return, where the live run wrote it. The invocation's
  replay anchor stays open until its `AgentInvocationSettled` or the replay tail, so its
  `Start`/`End` entries may follow its terminal; abandoned-`Start` and closed-anchor checks run
  when the anchor closes.
  Background work can continue live at the replay tail once that terminal is read, before the
  result is recorded. The consumed terminal still decides the result: a Store that switched to
  live in between records no second terminal.
- **Authority ends at the return.** The final outcome hides the lent scope card from every host
  call made after the return: such a call can neither lend nor derive from a handle to that card
  or to a card derived from it. A call made before the return keeps its admitted authority across
  awaits and store tasks. Attribution (`invocation-id()`) stays the method's, and the card stays
  installed until the drain ends.
- **Failures after the result** never replace it. A trap during the drain is retried by
  reconstruction under the retry policy; an interrupt or suspension stops the agent as it does
  during the method, and the resumed agent reconstructs. Replay consumes the recorded terminal, so
  the method does not run again. Giving up fails the agent and the queued invocations but publishes
  no failure for the recorded one; an interrupt also fails the queued invocations. A give-up of the
  primary Store records an exhausted retry state, so a successful result that settles becomes
  `background-failed` (`InvocationEnd::BackgroundFailed`, folded from that `Error` or an
  `Exited`; no new oplog entry). A same-key retry or lookup returns the recorded result with its
  `background_failure`; RPC callers get the plain result. A recorded method failure stays
  `failed`, and a later `AgentInvocationSettled` of the invocation keeps `background-failed`. A rejection
  trap (`PermissionDenied`, `InvalidRequest`) after the result is an ordinary trap of the agent.
  A stop or capacity failure that lands before `AgentInvocationSettled` stops the agent even when
  no background work is left: a durable agent reconstructs to write `Settled`, and an ephemeral
  agent fails (`EphemeralCannotSuspend`) while keeping the recorded result.
- **Read-only strictness** of a read-only method lasts through its drain: background work that
  attempts a write side effect (`WriteLocal` or `WriteRemote`) traps with `ReadOnlyViolation`
  after the result, which fails the agent without replacing the recorded result.
- **Status** stays `Running` from a terminal with `awaits_settlement` until its
  `AgentInvocationSettled` (`AgentStatusRecord::settling_invocation`), so interrupts, crash
  recovery and shard assignment treat a draining agent as running.
- **The `Settled` commit** of a live durable invocation whose next work is a due queued
  invocation is left to the `AgentInvocationStarted` commit of that invocation, which stores both
  entries with one status fold (`Worker::defer_settlement_commit`). The invocation loop commits a
  deferred `Settled` on its own before other work, before it waits for a command or a permit, and
  when it exits; a failure of the next invocation before its start commits it before it reads the
  retry state. The start of the next invocation (payload hydration, scope card, instance lock)
  runs with `Settled` buffered and the status `Running`. `Settled` is committed on its own when
  nothing is queued, for ephemeral agents, at the replay tail, and before resident work, an idle
  turn or a pending update.

## External effects

Tool isolation is limited to the fixed Wasm instruction-trap allowlist and uncaught Wasm
exceptions in `worker/invocation.rs::permanent_tool_guest_trap`. A trapped tool records
`body_execution: Trapped` with `RemoteInternalError`; typed quotas, policy exhaustion,
`WorkerOutOfMemory`, infrastructure and lifecycle failures retain owner recovery even when
their retry decision is exhausted. Destroy the physical Wasmtime Store but retain its original
`Ctx` for cleanup. Independent supervisors settle failed parents and cancel their descendants
without replacing selected outcomes or cancelling unrelated siblings. Replay re-executes trapped
bodies and their recorded host calls to rebuild shared filesystem effects and partial stdout,
then validates the exact terminal and descendants; completed external effects are not repeated.
Stream completion is independent of the structured result: an already-completed stream stays
completed. Allocation-dependent `memory.grow` backing refusal returning -1 and leading to panic
is a separate, preexisting nondeterminism limitation, not a universal isolation guarantee or a
reason to introduce fork/snapshot recovery.

Recovery repairs an incomplete durable call at its own `Start`; it does not restart a group of
completed effects. SDK sagas compensate completed steps and loop in guest code, appending new
calls without deleting history. A deterministic guest trap fails the invocation; it is not a
saga retry signal. Nonresumable handles report typed loss after reconstruction, so the application
must explicitly reopen and initialize a session. Loss does not prove that an earlier write failed.
Host HTTP transport, status-code and body-resumption retries remain host responsibilities. HTTP
uses Preview 3; the Rust SDK provides `golem_rust::http`. Preview 2 HTTP is not supported.
Guests cannot disable durable recording, open atomic regions, or get/set an execution cursor.
External operators can revert or fork at validated cuts. `oplog-commit` remains a replication
barrier, not an external transaction commit or an undo operation.

Golem itself enforces exactly-once for durable agent RPC and for oplog-processor plugin delivery
(`services/oplog/plugin/mod.rs`: batch keys from `oplog_processor_idempotency_key`, checkpoints in
`AgentStatusRecord.oplog_processor_checkpoints`). Ephemeral targets are fail-stop (accepted work is
not re-executed, but completion is not guaranteed). For external peers the crash window between
the effect and its `End` entry is ambiguous, so exactly-once depends on the peer honouring an
idempotency key: outgoing HTTP requests carry an `idempotency-key` header derived from the call's
durable position (`http/policy.rs`, on by default unless the guest set its own), and guests can
mint keys with `golem:api/host.generate-idempotency-key`. Peers without such support get
at-least-once in that window. Choose the `DurableFunctionType` of a host call by whether
re-executing it after a crash is safe.

## Changing oplog shape

A new or changed `OplogEntry` requires updating, in the same change: `is_hint()` classification,
the WIT `oplog-entry` / `public-oplog-entry` variants in `wit/deps/golem-1.x/golem-oplog.wit`
(the `oplog_macro` binds `wit_raw_type` / `wit_public_type`; load `modifying-wit-interfaces`),
replay claims (`src/durable_host/replay_state/claims.rs`), the replay cursor's invocation
anchors when the entry can appear between an invocation's terminal and its settlement
(`src/durable_host/replay_state/cursor/`), cut-point validation
(`src/worker/cut_point.rs`), status folding (`src/worker/status/`), public oplog rendering, and
a test that restarts a worker across the new entry.
