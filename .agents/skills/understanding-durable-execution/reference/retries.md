# Retries: in-function versus trap-based

Owner: `golem-worker-executor/src/durable_host/durability.rs` (decision and inline loop),
`durable_host/mod.rs::on_invocation_failure` (post-trap recovery decision),
`worker/status.rs` (folds `OplogEntry::Error` into `current_retry_state`, keyed by `retry_from`).

## Two paths, one budget

A durable host call that fails with a retryable error can recover in two ways:

| | In-function (inline) retry | Trap-based retry |
|---|---|---|
| Where | Inside the same host function, under the **same** `Start` | The invocation traps; the worker is torn down and reconstructed |
| Oplog | Appends and commits one `OplogEntry::Error { kind: Invocation, retry_from, inside_atomic_region, retry_policy_state }` hint per attempt (`InFunctionRetryHost::append_retry_error_entry`) | Appends `OplogEntry::Error { kind: Invocation, .. }` in `on_invocation_failure`, then the outer loop replays to `retry_from` |
| Cost | A sleep and a second live action | Full `Store` teardown, instance creation, replay of all history since the last baseline |
| Decided by | `InFunctionRetryState::decide_retry_with_properties` → `AsyncRetryDecision::{Retry, FallBackToTrap, Exhausted}` | `try_trigger_host_trap_retry` attaches a `SemanticTrapRetryOverride`; `on_invocation_failure` turns it into a `RetryDecision` (`Immediate`, `Delayed`, `None`, …) |

The two paths share one retry budget: `retry_count` (in-memory attempts of this host call) plus
the number of recorded `Error` entries with the same `retry_from`
(`count_oplog_errors_for`, `current_retry_state_for`). After a trap and replay the in-memory
count resets but the oplog count does not, so a policy exhausted inline stays exhausted.

Elapsed-time budgets share the same durable sequence boundary. When the selected policy contains
a `TimeBox` (including below another decorator or combinator), `RetryPolicyState::TimeBox` wraps
the structural policy state with the first decision's wall-clock timestamp and the greatest
elapsed duration observed so far. Each later decision evaluates with
`max(previous_elapsed, now - started_at)`. The high-water mark makes elapsed monotonic if the wall
clock moves backward, and persisting the wrapper in each `Error` entry means reconstruction does
not restart the budget. The first decision observes zero elapsed; `RetryPolicy::step` keeps its
canonical inclusive boundary, so `elapsed >= limit` gives up. Policies without a `TimeBox` keep
their existing state shape.

Infrastructure reconstruction is separate from both application retry paths. For example, a p2
HTTP response-body resume may need an external oplog payload to count bytes already delivered to
the guest. A temporary payload-backend failure is typed as `RecoveryRequired`: the durable call is
deliberately abandoned, so its `Start` remains without `End` or `Cancelled`, and the invocation
loop appends `Error { kind: Recovery, retry_policy_state: None, .. }`. It then destroys the
affected `Store` and filesystem window and schedules reconstruction with infrastructure backoff.
The accepted invocation and its idempotency key remain pending; reconstruction replays the
recorded prefix and repairs the incomplete call. Repeated Recovery errors neither advance nor
reset `current_retry_state`, which belongs only to semantic application retries.

Do not infer retryability from “infrastructure-owned”. Missing or malformed recorded payloads,
impossible resource-table state, replay divergence, and invalid manual-update snapshot baselines
are permanent and must not enter an endless Recovery loop. Genuine HTTP/content failures remain
guest-visible HTTP errors and use ordinary Invocation failure policy. Typed lifecycle conditions
(quota suspension, explicit interruption, shard loss) keep priority over Recovery, and ephemeral
agents remain fail-stop because they cannot reconstruct accepted execution.

## When inline retry is allowed

`decide_retry_with_properties` returns `Retry(delay)` only when all of these hold:

1. The function type is eligible (`is_eligible_for_internal_retry`):
   - `ReadLocal`, `ReadRemote`, `WriteLocal` — always;
   - `WriteRemote` — only while the guest's idempotence mode is on (`assume_idempotence`);
   - `WriteRemoteBatched` / `WriteRemoteTransaction` — never (the enclosing scope owns recovery).
   This is deliberately the same predicate as `can_reexecute_on_incomplete_replay`: a call that
   may be re-run when its `End` is missing after a crash may also be re-run inline.
2. The call is not inside an atomic region (`in_atomic_region()` → `FallBackToTrap`, because the
   region's rollback semantics must own the re-execution).
3. A `NamedRetryPolicy` resolves for the call's `RetryProperties` and its step verdict is
   `Retry(delay)` rather than `GiveUp` (→ `Exhausted`).
4. `delay <= max_in_function_retry_delay` (config); a longer wait falls back to the trap path so
   the worker can be unloaded instead of holding a resident `Store` idle.

Otherwise `FallBackToTrap` calls `try_trigger_host_trap_retry`; if that finds no applicable
policy, the failure is persisted as the call's result (`InternalRetryResult::Persist`).

HTTP response-body resumption has an additional ownership rule. A terminal P2 body read retires
the failed stream and parent `IncomingBody` in place before sending the Range request. Dropping
that old parent releases its connection-pool permits (or its unpooled request worker) while the
guest resource IDs and table parent/child relationship remain unchanged. On success, the
replacement `IncomingBody` retains the replacement response's worker, worker-error receiver, and
pool permits; swapping the replacement stream and body into the existing table slots therefore
preserves ordinary body lifetime and replay semantics. Sending the replacement before retiring
the old permit owner can self-wait until timeout when the per-host pool capacity is one.

Direct P2 resend header waits race only the native response readiness against a fresh
`create_interrupt_signal()`. This applies both to status-code retries in
`future_incoming_response::get` and to the shared resend used by response-body resumption and
awaiting-response recovery. Readiness wins when both are ready in the same poll; otherwise the
exact `InterruptKind` propagates without being classified as an HTTP failure or consuming another
semantic retry. The local `HostFutureIncomingResponse` remains the owner: returning on lifecycle
drops its pending request task, which releases transport permits asynchronously. Do not cancel the
whole retry operation, detach that owner, synthesize an HTTP cancellation result, or manually
release permits. Already committed retry decisions remain in the oplog, while the enclosing
durable call stays incomplete for ordinary reconstruction.

Spawned store tasks use the same decision but `FallBackToTrap` there means "stop inline retries
and let the invocation loop's trap path take over" (`durability.rs`, spawned-task section).

## What this means for a new durable function

- Pick the `DurableFunctionType` for its **re-execution safety**, not for what the peer is. A
  non-idempotent remote write must be `WriteRemote` with idempotence off (or batched /
  transactional) so it is neither retried inline nor re-executed on incomplete replay.
- If the function has a natural idempotency handle (HTTP `idempotency-key`, keyed writes), make
  it `WriteRemote` so the cheap inline path applies while `assume_idempotence` is on.
- Attach `RetryProperties` so named policies can distinguish, e.g., HTTP status classes.
- Add a test for both transitions: an inline retry that succeeds (oplog has `Error` hints under
  the same `retry_from`, one `Start`/`End`), and a fallback to trap (no extra inline hints, the
  worker was reconstructed).

## Tests

- `tests/in_function_retry/host_services.rs::{keyvalue_set_retries_inline_when_idempotent,
  in_function_retry_transitions_from_inline_to_trap_based}` — the second documents the shared
  budget arithmetic in its comments.
- `tests/in_function_retry/http_request.rs::{http_zone1_falls_back_to_trap_when_delay_exceeds_threshold,
  http_post_fails_permanently_when_idempotence_disabled,
  http_get_retried_inline_even_when_idempotence_disabled}` (and the p3 mirrors in
  `tests/in_function_retry/p3.rs`).
- `tests/in_function_retry/{http_servers.rs,http_streams.rs}` — streaming bodies and server
  failure modes, including response-body payload outages that physically retire multiple runtime
  generations while preserving the same invocation and semantic retry state; the withheld-header
  tests cover prompt lifecycle interruption, native response-owner cleanup, pool release, and
  reconstruction for status retries and body resumption.
- `tests/retry_lifecycle.rs::{interrupt_worker_during_delayed_recovery_retry,
  delete_worker_during_delayed_recovery_retry}` — trap-based `Delayed` retries interact with
  interruption and deletion.
- `tests/retry_policies.rs` — named policies are persisted to the oplog and survive restart.
