# Retries: in-function versus trap-based

Owner: `golem-worker-executor/src/durable_host/durability.rs` (decision and inline loop),
`durable_host/mod.rs::on_invocation_failure` (post-trap recovery decision),
`worker/status.rs` (folds `OplogEntry::Error` into `current_retry_state`, keyed by `retry_from`).

## Two paths, one budget

A durable host call that fails with a retryable error can recover in two ways:

| | In-function (inline) retry | Trap-based retry |
|---|---|---|
| Where | Inside the same host function, under the **same** `Start` | The invocation traps; the worker is torn down and reconstructed |
| Oplog | Appends and commits one `OplogEntry::Error { retry_from, inside_atomic_region, retry_policy_state }` hint per attempt (`InFunctionRetryHost::append_retry_error_entry`) | Appends `OplogEntry::Error` in `on_invocation_failure`, then the outer loop replays to `retry_from` |
| Cost | A sleep and a second live action | Full `Store` teardown, instance creation, replay of all history since the last baseline |
| Decided by | `InFunctionRetryState::decide_retry_with_properties` → `AsyncRetryDecision::{Retry, FallBackToTrap, Exhausted}` | `try_trigger_host_trap_retry` attaches a `SemanticTrapRetryOverride`; `on_invocation_failure` turns it into a `RetryDecision` (`Immediate`, `Delayed`, `None`, …) |

The two paths share one retry budget: `retry_count` (in-memory attempts of this host call) plus
the number of recorded `Error` entries with the same `retry_from`
(`count_oplog_errors_for`, `current_retry_state_for`). After a trap and replay the in-memory
count resets but the oplog count does not, so a policy exhausted inline stays exhausted.

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
  failure modes.
- `tests/retry_lifecycle.rs::{interrupt_worker_during_delayed_recovery_retry,
  delete_worker_during_delayed_recovery_retry}` — trap-based `Delayed` retries interact with
  interruption and deletion.
- `tests/retry_policies.rs` — named policies are persisted to the oplog and survive restart.
