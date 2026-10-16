# Retries: individual call repair

Owner: `durable_host/durability.rs`, `durable_host/mod.rs::on_invocation_failure`,
`worker/invocation_loop.rs` and the status fold.

## Two paths, one budget

A retryable durable host call can retry inline under the same `Start`, or fall back to a trap
that lets the outer loop discard the Store, wait, reconstruct and repair the incomplete call.
Neither path reruns completed neighboring effects or deletes a guest workflow's history.
A deterministic guest trap fails the invocation; it is not an application retry mechanism.

Each semantic retry records an `Error { kind: Invocation, retry_from, retry_policy_state, .. }`
hint. The attempt count combines in-memory attempts with recorded errors for the same
`retry_from`. Reconstruction resets the in-memory count, not the recorded budget.

For a policy containing `TimeBox`, the durable state retains the first-decision timestamp and a
monotonic elapsed high-water mark. Evaluation uses `max(previous_elapsed, now - started_at)`;
restart and backward wall-clock steps cannot reset the budget. The first decision sees zero
elapsed, and `elapsed >= limit` gives up.

## Inline retry eligibility

`InFunctionRetryState::decide_retry_with_properties` permits inline retry only if:

1. The call type is `ReadLocal`, `ReadRemote`, `WriteLocal`, or `WriteRemote`.
   Batched/transactional children require their owner's recovery rules.
2. A named policy matches the available `RetryProperties` and returns `Retry(delay)`.
3. The delay is at most `max_in_function_retry_delay`. Longer waits fall back to reconstruction
   rather than holding an idle Store.

An exhausted or absent policy does not authorize an unsafe effect. The failure is persisted and
returned when the host path has no applicable retry. External peers must deduplicate ambiguous
remote writes; there is no guest idempotence mode.

## HTTP stays host-owned

HTTP uses Preview 3; Preview 2 HTTP is not supported. Rust applications use `golem_rust::http`.
Transport retries, opt-in `status-code` retries, request-body reconstruction and response-body
resumption remain host responsibilities. Status retries require a predicate that explicitly
references `status-code`; a catch-all policy does not retry HTTP responses.

An incomplete eligible send repairs its own recorded `Start` with the same derived idempotency
key. Body resumption preserves bytes already observed by the guest. A Range response must start
at the delivered offset, or a full response must match the delivered prefix before any new bytes
are exposed. A mismatched response ends with `HttpResponseIncomplete`, not a spliced body.

Loss of a response does not prove that the peer failed to apply the write; exactly-once requires
peer deduplication. The host retains request identity across retries.

## Infrastructure reconstruction

A temporary failure to read Golem's recovery data is not an HTTP result or a semantic retry.
`RecoveryRequired` leaves the affected call unfinished, retires the Store and filesystem window,
and uses infrastructure backoff. `Error { kind: Recovery, retry_policy_state: None, .. }` neither
advances nor resets semantic retry state. The accepted invocation and its key remain pending.
Missing or malformed payloads and replay divergence are permanent, not reasons to loop forever.
Interruption, quota suspension and shard loss retain their typed lifecycle meaning. Ephemeral
agents remain fail-stop.

## Guest workflows and handle loss

SDK sagas compensate successful steps in reverse order and loop in guest code for another
attempt. New calls append to the oplog; earlier attempts and compensations remain. After complete
rollback, infallible helpers resolve verb `saga`, noun the agent URN, with `attempt`/`error-kind`.
`next-retry-delay` uses the existing evaluator with an opaque binary continuation, durable clocks
and durable timers. When no rule matches, use the agent default. Unbounded policies return a typed error;
count/time/never exhaustion and selector/evaluator errors retain the original failure.
Compensation failure stops retries and must not be reported as complete rollback. Caught failures
in TS, Effect, Scala and Go compensate in reverse order; fallible helpers propagate the original
exception/panic only after complete rollback. Effect external cancellation stops retries while
retaining original and compensation evidence in Cause; self-interruption can be policy-retried.
Rust WASM panic aborts without compensation; Result failures, including infallible-helper
exhaustion and partial rollback, stay typed. MoonBit exposes the retry API without a saga helper.

A nonresumable handle reports typed loss after reconstruction. RDBMS result streams and
transactions return `ResourceLost`, not a reopened
query or transaction. Application code explicitly opens
and initializes another session and decides what pending traffic is safe to resend. Typed loss is
not evidence that an earlier write was undelivered.

External operators can revert/fork at validated cuts. Guests cannot disable recording or set an
execution cursor. `oplog-commit` remains a replication barrier, not an external transaction commit.

## Verification

Test inline success and fallback to reconstruction through the public host interface. Assert one
`Start`, the missing terminal appended under that same identity, preserved neighboring results,
and stable upstream keys. Count upstream effects as well as attempts. For nonresumable resources,
assert typed loss without a silent reconnect or resend. See `tests/in_function_retry/p3.rs`,
`tests/http/concurrent_recovery.rs`, `tests/retry_lifecycle.rs`, and `tests/retry_policies.rs`.
