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
   `resume_replay`). Do not design feature-specific "safe to unload" gates; make every pending
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
`AgentInvocationFinished`, idempotency keys and recorded results, `Start`/`End`/`Cancelled` and
`CompletionDelivered`/`CompletionDiscarded` markers, update and snapshot entries, and status folded
from the oplog. Derived and disposable: `Store` memory, in-memory queues, caches, event
subscriptions, spawned store tasks, sockets. Never let correctness after a restart depend on
anything in the second list.

## External effects

Tool isolation is limited to the fixed Wasm instruction-trap allowlist in
`worker/invocation.rs::permanent_tool_guest_trap`. A trapped tool records
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
replay claims (`src/durable_host/replay_state/claims.rs`), cut-point validation
(`src/worker/cut_point.rs`), status folding (`src/worker/status/`), public oplog rendering, and
a test that restarts a worker across the new entry.
